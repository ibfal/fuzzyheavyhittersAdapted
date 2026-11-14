//! Benchmark instrumentation (bench/BENCHMARK_MOSAIC.md, A3, A4, A7, B3).
//!
//! Measurement only: nothing here changes what the protocol computes or sends. Every function is
//! a no-op until a binary calls [`enable`] (`mosaic_server`, `mosaic_client` and `mosaic_dealer`
//! do so only with `--bench-out`), so runs without the bench options behave exactly as before.
//!
//! What is recorded, per process:
//! - **step times** ([`Timer`], [`add_time`]): wall-clock time per online step, summed over the
//!   crawl's iterations (B3: `sketch_verify`, `expand`, `check_threshold`, `reveal`,
//!   `derived_check`);
//! - **rounds** ([`turns`], [`TurnMax`], [`add_rounds`]): send-then-wait turns, see below;
//! - **levels** ([`record_levels`]): prefixes checked (= result bits opened to both servers) and
//!   kept per crawl level (A7);
//! - **counters** ([`count`]): e.g. crawl iterations.
//!
//! # Rounds
//!
//! A turn is one step in which a party sends and then has to wait for the other party's reply
//! (A4). The code calls [`turns`] where it sends: once per message exchange it drives itself
//! (Beaver openings, the opening of the verification values, masked-value exchanges, the
//! result-bit exchange, dealer requests), and with [`GC_TURNS`] / [`OT_TURNS`] at the calls into
//! swanky, whose messages are sent inside the library. The count goes into a thread-local
//! counter, because each server thread drives its own channel. A parallel section takes the
//! maximum over its threads ([`TurnMax`]): threads run concurrently, so the slowest chain of
//! turns is the critical path. Sections that run one after the other add up.
//!
//! Both servers run the same calls with the same chunking, so both count the same turns.

use serde_json::{json, Map, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Turns of one two-party garbled-circuit evaluation with swanky's semi-honest garbler and
/// evaluator (`fancy_garbling::twopac::semihonest`, OT = ALSZ extension over Chou-Orlandi base
/// OTs), as Mosaic calls it in `batch_equality_full.rs` and `greater_than_or_equal_threshold.rs`.
/// The messages, E = evaluator (server 0), G = garbler (server 1):
/// 1. E -> G: Chou-Orlandi point `s` (`Evaluator::new` -> ALSZ receiver init -> CO sender init);
/// 2. G -> E: 128 base-OT points `r` (`Garbler::new` -> ALSZ sender init -> CO receive);
/// 3. E -> G: base-OT ciphertexts;
/// 4. G -> E: the garbler's input wires (`encode_many`);
/// 5. E -> G: the OT-extension matrix `u` for the evaluator's inputs (`encode_many` -> ALSZ receive);
/// 6. G -> E: the OT ciphertexts, the garbled gates and the output decoding (`outputs`);
/// 7. E -> G: the one-byte acknowledgement Mosaic adds after the outputs.
///
/// Seven one-way flights: E waits for a reply three times (after 1, 3 and 5); the acknowledgement
/// (7) shares its flight with whatever E sends next. 3 turns.
pub const GC_TURNS: u64 = 3;

/// Turns of one batched boolean-to-arithmetic conversion with a fresh ALSZ OT
/// (`check_phase.rs` `batch_boolean_to_ring_share_modint`): E -> G base-OT point, G -> E base-OT
/// points, E -> G base-OT ciphertexts + extension matrix, G -> E ciphertexts. E waits twice. The
/// first flight shares its flight with the acknowledgement of the GC before it. 2 turns.
pub const OT_TURNS: u64 = 2;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State::new());

thread_local! {
    /// Turns counted on this thread since the last [`take_turns`].
    static TURNS: Cell<u64> = const { Cell::new(0) };
    /// While set, step times and rounds recorded on this thread go to this name instead
    /// (see [`scope`]).
    static SCOPE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

struct State {
    /// (step, milliseconds), in the order the steps first appear.
    steps: Vec<(String, f64)>,
    /// (section, critical-path turns, turns summed over all threads).
    rounds: Vec<(String, u64, u64)>,
    /// Index = level - 1: (prefixes checked, prefixes kept).
    levels: Vec<(u64, u64)>,
    counters: Vec<(String, u64)>,
    /// (event, [`unix_ns`] when it first happened).
    marks: Vec<(String, u128)>,
}

impl State {
    const fn new() -> Self {
        State {
            steps: Vec::new(),
            rounds: Vec::new(),
            levels: Vec::new(),
            counters: Vec::new(),
            marks: Vec::new(),
        }
    }
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Turn the instrumentation on for this process.
pub fn enable() {
    ENABLED.store(true, Ordering::SeqCst);
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Wall-clock time in nanoseconds since the Unix epoch. All parties run on one machine, so their
/// timestamps can be compared (Windows: GetSystemTimePreciseAsFileTime, sub-microsecond).
pub fn unix_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn scoped(name: &'static str) -> &'static str {
    SCOPE.with(|s| s.get()).unwrap_or(name)
}

/// Add `d` to step `name` (or to the current [`scope`]).
pub fn add_time(name: &'static str, d: Duration) {
    if !enabled() {
        return;
    }
    let name = scoped(name);
    let ms = d.as_secs_f64() * 1e3;
    let mut st = state();
    match st.steps.iter_mut().find(|(n, _)| n == name) {
        Some((_, total)) => *total += ms,
        None => st.steps.push((name.to_string(), ms)),
    }
}

/// A running step timer; `None` inside when the instrumentation is off.
pub struct Timer(Option<Instant>);

/// Start a step timer.
pub fn timer() -> Timer {
    Timer(if enabled() { Some(Instant::now()) } else { None })
}

impl Timer {
    /// Add the time since the start to step `name`, and restart.
    pub fn lap(&mut self, name: &'static str) {
        if let Some(start) = self.0 {
            let now = Instant::now();
            add_time(name, now - start);
            self.0 = Some(now);
        }
    }

    /// Add the time since the start to step `name`.
    pub fn stop(mut self, name: &'static str) {
        self.lap(name);
    }
}

/// Redirect the step times and rounds recorded on this thread to `name` until the guard is
/// dropped. The H3 derived-coordinate check uses it, so that its `batch_check` counts as
/// `derived_check` and not as one more crawl iteration.
pub fn scope(name: &'static str) -> ScopeGuard {
    let previous = SCOPE.with(|s| s.replace(if enabled() { Some(name) } else { None }));
    ScopeGuard(previous)
}

pub struct ScopeGuard(Option<&'static str>);

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let previous = self.0;
        SCOPE.with(|s| s.set(previous));
    }
}

/// Count `n` send-then-wait turns on this thread (called where the messages are sent).
#[inline]
pub fn turns(n: u64) {
    if enabled() {
        TURNS.with(|t| t.set(t.get() + n));
    }
}

/// The turns counted on this thread since the last call, and reset the counter.
#[inline]
pub fn take_turns() -> u64 {
    TURNS.with(|t| t.replace(0))
}

/// Collects the turns of the threads of one parallel section.
pub struct TurnMax {
    max: AtomicU64,
    sum: AtomicU64,
}

impl TurnMax {
    pub fn new() -> Self {
        TurnMax { max: AtomicU64::new(0), sum: AtomicU64::new(0) }
    }

    /// Call at the start of a thread's work: drops turns left on the thread by other code.
    pub fn begin(&self) {
        take_turns();
    }

    /// Call at the end of a thread's work: adds this thread's turns.
    pub fn end(&self) {
        let n = take_turns();
        self.max.fetch_max(n, Ordering::Relaxed);
        self.sum.fetch_add(n, Ordering::Relaxed);
    }

    pub fn max(&self) -> u64 {
        self.max.load(Ordering::Relaxed)
    }

    pub fn sum(&self) -> u64 {
        self.sum.load(Ordering::Relaxed)
    }
}

impl Default for TurnMax {
    fn default() -> Self {
        Self::new()
    }
}

/// Add the critical-path turns of parallel sections that ran one after the other to section
/// `name` (or to the current [`scope`]). `sum` is the total over all threads, kept for checks.
pub fn add_rounds(name: &'static str, sections: &[&TurnMax]) {
    if !enabled() {
        return;
    }
    let name = scoped(name);
    let critical: u64 = sections.iter().map(|s| s.max()).sum();
    let sum: u64 = sections.iter().map(|s| s.sum()).sum();
    let mut st = state();
    match st.rounds.iter_mut().find(|(n, _, _)| n == name) {
        Some((_, c, s)) => {
            *c += critical;
            *s += sum;
        }
        None => st.rounds.push((name.to_string(), critical, sum)),
    }
}

/// Record checked prefixes: one `(level, kept)` per prefix, levels counted from 1.
pub fn record_levels(prefixes: &[(usize, bool)]) {
    if !enabled() {
        return;
    }
    let mut st = state();
    for &(level, kept) in prefixes {
        let level = level.max(1);
        if st.levels.len() < level {
            st.levels.resize(level, (0, 0));
        }
        st.levels[level - 1].0 += 1;
        st.levels[level - 1].1 += u64::from(kept);
    }
}

/// Add `n` to counter `name`.
pub fn count(name: &'static str, n: u64) {
    if !enabled() {
        return;
    }
    let mut st = state();
    match st.counters.iter_mut().find(|(c, _)| c == name) {
        Some((_, total)) => *total += n,
        None => st.counters.push((name.to_string(), n)),
    }
}

/// Remember when event `name` first happened ([`unix_ns`]).
pub fn mark(name: &'static str) {
    if !enabled() {
        return;
    }
    let now = unix_ns();
    let mut st = state();
    if !st.marks.iter().any(|(n, _)| n == name) {
        st.marks.push((name.to_string(), now));
    }
}

/// Everything recorded so far, as JSON:
/// `{"steps": [{"name", "ms"}], "rounds": {name: {"critical", "sum_over_threads"}},
///   "candidates_per_level": [..], "frontier_per_level": [..], "counters": {name: n},
///   "marks": {name: unix_ns}}`.
pub fn snapshot() -> Value {
    let st = state();
    let steps: Vec<Value> =
        st.steps.iter().map(|(name, ms)| json!({"name": name, "ms": ms})).collect();
    let mut rounds = Map::new();
    for (name, critical, sum) in &st.rounds {
        rounds.insert(name.clone(), json!({"critical": critical, "sum_over_threads": sum}));
    }
    let mut counters = Map::new();
    for (name, n) in &st.counters {
        counters.insert(name.clone(), json!(n));
    }
    let mut marks = Map::new();
    for (name, ns) in &st.marks {
        marks.insert(name.clone(), json!(*ns as u64));
    }
    json!({
        "steps": steps,
        "rounds": rounds,
        "candidates_per_level": st.levels.iter().map(|l| l.0).collect::<Vec<_>>(),
        "frontier_per_level": st.levels.iter().map(|l| l.1).collect::<Vec<_>>(),
        "counters": counters,
        "marks": marks,
    })
}

// --- Files shared by the parties -----------------------------------------------------------------
//
// A run has four processes. Server 0 writes the A9 line; the client, server 1 and the dealer
// write their measurements to part files next to the `--bench-out` file, which server 0 reads.

/// `<out>.<party>.part.json`, where this party's measurements go.
pub fn part_path(out: &str, party: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}.part.json", out, party))
}

/// Write `value` to `path` in one step (a temporary file, then a rename), so that a reader
/// never sees half a file.
pub fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("cannot rename {} to {}: {}", tmp.display(), path.display(), e))
}

/// Wait until `path` exists, then read it as JSON. Gives up after `timeout`.
pub fn wait_for_json(path: &Path, timeout: Duration) -> Result<Value, String> {
    let start = Instant::now();
    loop {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            return serde_json::from_str(&text)
                .map_err(|e| format!("cannot parse {}: {}", path.display(), e));
        }
        if start.elapsed() > timeout {
            return Err(format!("{} did not appear within {:?}", path.display(), timeout));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The `--bench-meta` argument as JSON: `@file`, a path to a file, or inline JSON (PowerShell
/// strips the quotes of inline JSON, so the runner always passes a file).
pub fn read_meta(arg: &str) -> Result<Value, String> {
    let text = if let Some(path) = arg.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path, e))?
    } else if arg.trim_start().starts_with('{') {
        arg.to_string()
    } else {
        std::fs::read_to_string(arg).map_err(|e| format!("cannot read {}: {}", arg, e))?
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("--bench-meta is not JSON: {}", e))?;
    if !value.is_object() {
        return Err("--bench-meta must be a JSON object".to_string());
    }
    Ok(value)
}

/// Append one compact JSON line to a JSONL file.
pub fn append_jsonl(path: &str, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("cannot open {}: {}", path, e))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

/// Milliseconds between two [`unix_ns`] timestamps (negative if `to` is earlier).
pub fn ms_between(from: u64, to: u64) -> f64 {
    (to as f64 - from as f64) / 1e6
}

/// `x` rounded to `digits` decimals (keeps the JSON lines short).
pub fn round_to(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_only_when_enabled() {
        // Disabled: nothing is counted.
        turns(5);
        assert_eq!(take_turns(), 0);
        enable();
        let tm = TurnMax::new();
        tm.begin();
        turns(GC_TURNS);
        turns(OT_TURNS);
        tm.end();
        assert_eq!(tm.max(), 5);
        {
            let _g = scope("derived_check");
            add_rounds("crawl", &[&tm]);
        }
        add_rounds("crawl", &[&tm]);
        record_levels(&[(1, true), (1, false), (3, true)]);
        let snap = snapshot();
        assert_eq!(snap["rounds"]["crawl"]["critical"], 5);
        assert_eq!(snap["rounds"]["derived_check"]["critical"], 5);
        assert_eq!(snap["candidates_per_level"], json!([2, 0, 1]));
        assert_eq!(snap["frontier_per_level"], json!([1, 0, 1]));
    }
}
