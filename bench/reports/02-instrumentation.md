# 02: Instrumentation (Mosaic)

## Summary

The Mosaic code is instrumented, and the client micro-benchmark and the runner are written. The full benchmark was not started.
- **Result lines:** with `--bench-out`, the four processes (dealer, both servers, client simulator) record timings, rounds, bytes, leakage counters and the output. Server 0 merges them into one A9 line per run.
- **Default behaviour:** without the new options the binaries behave exactly as before. HEAD with only the buffer change and the instrumented build give the same output and the same bytes on every link.
- **Correctness:** all 4 smoke runs and all 9 full-day runs reproduce Mosaic's plaintext answer exactly, including the prefixes opened per level. All 21 result lines pass the schema check.
- **Rounds:** counted where the messages are sent. A temporary channel-level count, since removed, gives exactly the same numbers.
- **Cost on this laptop:** one full-day run takes 5–6 min for grid, geohash and Morton, and 15 min for H3.
  - The 72 main runs take about 6.4 h: one night.
  - All 72 runs at τ = 0.005 would take about 22 h, and H3 at τ = 0.005 does not fit in memory. I recommend `mosaic_grid` only at τ = 0.005: 18 runs, 3.9 h.
  - The authors' original 33-level H3 crawl takes 49 min per full-day run; its 18 runs would take about 10 h.
- **Channel buffers:** 64 MiB vs 256 MiB makes no real difference to the online time (341 s vs 347 s back to back). A first 256 MiB run took 505 s, but the slowdown hit every step, including purely local work, so it was the machine's state, not the buffer.

**Needs your decision** (Deviations 6–8):
- B6 for H3 `day_3of3`: 15 min is over the 10-minute line, so 3 measured reps instead of 5;
- which optional runs to do;
- setting up the laptop as A8 asks before the full run (power plan, closed applications).

## Files

Created:
- `src/bench.rs`: the measurement recorder. Step timers, send-then-wait turn counters (thread-local, maximum over threads per parallel section), prefixes per level, counters, event timestamps, part-file and JSONL helpers. Every function is a no-op until a binary calls `bench::enable()`.
- `src/bin/benchmarks/client_report_bench.rs` (binary `mosaic_client_bench`): the A6 micro-benchmark.
- `bench/scripts/run_bench.py`: the runner.
- `bench/reports/02-instrumentation.md`: this report.

Modified (Mosaic code; the protocol's computation and messages are unchanged):
- `src/channel.rs`: the listening side's buffer size comes from `MOSAIC_CHANNEL_BUFFER_BYTES`. Unset, it is the original 1 GiB (`channel.rs:93-127`). This is the only change to this file.
- `src/lib.rs`: `pub mod bench;`.
- `src/fuzzy_match/protocol.rs`: measurement calls only.
  - Turn counting per parallel section: sketch verification (`:181-283`), `batch_check`'s check-and-threshold section and reveal section (`:919-1123`).
  - One turn at the opening of the verification values (`:256`), at the result-bit exchange (`:1114`), and at the three dealer requests (`:1357,1395,1431`).
  - Step timers: `expand` (`:591-600,613-703,711-753`), `check_threshold`, `reveal`, and `derived_check` (`:803-874`, which sends its `batch_check` to `derived_check`).
  - Prefixes checked and kept per level (`:711-720,857-861`), and the crawl iteration count (`:614`).
- `src/fuzzy_match/check_phase.rs`, `threshold_phase.rs`, `sketch_phase.rs`: one `bench::turns(..)` call after each exchange.
  - check phase: GC check 3 + OT 2 turns (`check_phase.rs:173,181,333,342`); the FSS masked-value exchanges, 1 each (`:277,411`).
  - threshold phase: GC threshold, 3 turns (`threshold_phase.rs:66`); the FSS exchange, 1 (`:146`).
  - sketch phase: each Beaver opening, 1 turn (`sketch_phase.rs:1021`).
- `src/fuzzy_match/client.rs`: one timestamp just before the client's first byte is written (`client.rs:103`).
- `src/fuzzy_match/dealer.rs`: the key pre-generation that each channel thread ran inline is now a method, `pregenerate_keys` (`:382`), and `run_dealer_parallel_with_keys` (`:432`) can take keys generated beforehand.
  - `run_dealer_parallel` calls it with `None`, which does what it did before.
  - Needed for the dealer decision; see Deviations 1.
- `src/bin/mosaic_end_to_end/mosaic_server.rs`: `--bench-out`, `--bench-meta`.
  - Timestamps: reports stored (`:277`) and output ready (`:396`).
  - Byte snapshots of the inter-server channels; the `sketch_verify` timer.
  - Server 1 writes its part file; server 0 builds and appends the A9 line (`mod bench_line`, `:448-`).
- `src/bin/mosaic_end_to_end/mosaic_client.rs`: `--bench-out`. Writes the client's part: first-byte time, sketch-generation time, bytes per server.
- `src/bin/mosaic_end_to_end/mosaic_dealer.rs`: `--bench-out`. Pre-generates the keys before connecting and writes a part file when done.
- `Cargo.toml`: the `mosaic_client_bench` binary.

Modified (harness):
- `bench/scripts/bench_common.py`: added the following; nothing existing changed.
  - smoke constants (`SMOKE_DATASET`, `SMOKE_SUBSET`, `SMOKE_CSV`, `SMOKE_THRESHOLD`);
  - `H3_CRAWLS`, `CHANNEL_BUFFER_ENV`, `CHANNEL_BUFFER_BYTES` (64 MiB);
  - `PROTOCOL_BASE` (h2 = 1, h3 = 15, the sketch modulus, as in the repo's day configs);
  - `CLIENT_BENCH_ROWS`, `gt_variant()`;
  - `self_check()`, copied unchanged from Mastic's `bench_common.py`.
- `bench/README.md`: a "Running the benchmark" section and the results row in "What is where".
- `.gitignore`: `bench/.run/` (the runner's configs, logs, part files and per-run records).

Not touched: everything else under `src/` (the GC/OT code, FSS, sketch logic, encodings, `util.rs`), `configs/`, `evaluate.py`, `test_evaluate.py`, `ground_truth.py`, the data, the ground truth and the reference.

## How to run

```powershell
# Smoke runs, all four encodings (builds the binaries first; about 2 minutes):
.\.venv\Scripts\python.exe bench\scripts\run_bench.py --smoke

# One full-day timing run (as used for the checks below):
.\.venv\Scripts\python.exe bench\scripts\run_bench.py --only-encoding mosaic_grid --only-subset 3of3 --reps 1 --no-warmup --skip-keygen --results-dir bench\.run\timing
.\.venv\Scripts\python.exe bench\scripts\run_bench.py --only-encoding h3 --h3-crawl original --only-subset 3of3 --reps 1 --no-warmup --skip-keygen --results-dir bench\.run\timing
.\.venv\Scripts\python.exe bench\scripts\run_bench.py --only-encoding mosaic_grid --tau 0.005 --only-subset 3of3 --reps 1 --no-warmup --skip-keygen --results-dir bench\.run\timing
.\.venv\Scripts\python.exe bench\scripts\run_bench.py --only-encoding mosaic_grid --only-subset 3of3 --reps 1 --no-warmup --skip-keygen --results-dir bench\.run\buffercheck --channel-buffer 268435456

# Main benchmark (not started):
.\.venv\Scripts\python.exe bench\scripts\run_bench.py
```

`bench\README.md` lists every flag and the optional runs.

## Checks

All runs: commit df685ec plus this prompt's uncommitted changes (`dirty: true`); 5 threads per server and dealer; 64 MiB channel buffers unless stated; i7-1255U, Windows 11, Balanced power plan, on mains power.

### 1. Smoke runs (B7): `ten_locations.csv`, threshold 0.1 (N = 165, T = 16)

| Encoding | Outputs (ground truth) | Prefixes checked = bits opened | Levels | Online rounds (sketch + crawl) | Online | server→server sent, s0 / s1 (MiB) | client→server per server (B) |
|---|---|---|---|---|---|---|---|
| `mosaic_grid` | 484 (484) | 1,334 | 22 | 1,577 (5 + 1,572) | 16.8 s | 22.3 / 58.0 | 752,440 |
| `geohash` | 484 (484) | 1,384 | 22 | 1,632 (5 + 1,627) | 17.3 s | 23.1 / 60.2 | 752,440 |
| `h3` (δ 5, derived) | 359 (359) | 1,821 | 23 | 2,076 (5 + 2,071) | 24.6 s | 34.7 / 101.9 | 1,126,825 |
| `morton` | 484 (484) | 1,254 | 20 | 1,475 (5 + 1,470) | 18.2 s | 21.0 / 54.6 | 691,720 |

- Self-check against `ground_truth.py`: precision 1, recall 1, every centre equal, for all four encodings. **Pass.**
- Prefixes checked and kept per level (`leakage.revealed_aggregates_per_level`, `frontier_per_level`) equal the ground truth's `candidates_per_level` / `frontier_per_level` on every level. **Pass.**
- `client_validity_bits` = 165, `revealed_kind` = `"bit"`, all counts null, 0 clients rejected.
- The runner checks all of this on every run and refuses to append a line that fails.

### 2. Behaviour without the new options

| Comparison (H3 smoke config, 64 MiB) | Outputs | server0 sent / received (B) | client sent per server (B) | dealer (B) |
|---|---|---|---|---|
| HEAD + only the `channel.rs` change (built in the scratchpad from `git archive HEAD`) | 359 | 36,357,002 / 106,882,026 | 1,126,825 | 90 |
| this prompt's build, no bench options | 359 | 36,357,002 / 106,882,026 | 1,126,825 | 90 |

- Identical on both servers. These are also the numbers the instrumented run recorded in its A9 line (smoke H3: 34.7 / 101.9 MiB).
- With `MOSAIC_CHANNEL_BUFFER_BYTES` unset, unmodified HEAD and the new build both fail in the same way on this laptop. Server 1 aborts with "memory allocation of 1073741824 bytes failed" while it sets up its channels: two 1 GiB buffers per listening channel, and server 0 has 11 listening channels. So the default is unchanged too, but it cannot run on this machine (see Deviations 9).
- Every recording call is gated by `bench::enabled()`. The dealer pre-generates its keys before connecting only with `--bench-out`.

### 3. Schema check (A9)

- `bench_common.py --self-test`: all cases pass.
- All 21 lines written in this prompt pass `validate_result` (smoke lines with the subset swapped, as in Mastic's runner):
  - 4 smoke lines and 4 client lines;
  - 7 timing lines and 2 buffer-check lines;
  - 4 lines from the round validation.

**Pass.**

### 4. Rounds: how they are counted, and the check

**Definition (A4, prompt-1 decision).** A turn is one step in which a server sends and then waits for the other server's reply.
- Each server thread drives its own channel. The code counts the turns of a thread where its messages are sent.
- A parallel section (all threads) counts the maximum over its threads: the slowest chain is the critical path.
- Sections that run one after the other add up.
- `rounds.online` = sketch verification + crawl (+ H3's derived check). Both servers count the same; server 0 writes it.

**Turns per message exchange:**

| Exchange | Turns | Where counted |
|---|---|---|
| Beaver opening (4 per thread in sketch verification) | 1 each | `sketch_phase.rs:1021` |
| Opening of the verification values | 1 | `protocol.rs:256` |
| GC equality test (swanky semi-honest garbler/evaluator, ALSZ OT over Chou-Orlandi) | 3 | `check_phase.rs:173` |
| OT boolean-to-arithmetic conversion (fresh ALSZ OT) | 2 | `check_phase.rs:181` |
| GC ≥ T (one per thread per iteration) | 3 | `threshold_phase.rs:66` |
| Result-bit exchange | 1 | `protocol.rs:1114` |
| Dealer request (FSS methods only; never with gc_gc) | 1 | `protocol.rs:1357,1395,1431` |

- **Why GC = 3:** the swanky calls send 7 one-way messages: base-OT point, base-OT points, base-OT ciphertexts, garbler wires, OT-extension matrix, ciphertexts + garbled gates + output decoding, and Mosaic's acknowledgement. The evaluator waits three times (`bench.rs`, `GC_TURNS`).
- **Why OT = 2:** the OT that follows sends its first message in the same direction as that acknowledgement, so it adds 2 turns, not 2.5.
- So each checked prefix costs 5 turns, and each crawl iteration costs 5 × ⌈candidates/threads⌉ + 4 (GC ≥ T 3 + reveal 1).
- Example, full day `mosaic_grid`: 76 iterations, crawl 2,974 + sketch 5 = **2,979 online rounds**.
- `rounds.upload` = 1: the client sends both messages to both servers without waiting. `rounds.extra` = 0.
- In the line, `extra.rounds_sketch_verify` and `extra.rounds_crawl` give the split (H3 also `extra.rounds_derived_check`).

**Check against the channel.** I temporarily added a write→read transition counter to `CommTrackingChannel` and ran the four smoke runs, then restored `channel.rs` and `mosaic_server.rs` from saved copies (SHA-256 checked) and rebuilt.

| Encoding | Channel transitions, summed over the 5 inter-server channels (both servers) | Call-site turns summed over threads | Busiest channel | Critical path |
|---|---|---|---|---|
| `mosaic_grid` | 7,562 | 7,567 | 1,576 | 1,577 |
| `geohash` | 7,824 | 7,829 | 1,631 | 1,632 |
| `h3` | 10,033 | 10,038 | 2,075 | 2,076 |
| `morton` | 7,106 | 7,111 | 1,474 | 1,475 |

The difference is always exactly 5, one per channel: a channel's first read has no write before it. **Pass:** the call-site counts are exact.

### 5. Against the thesis and the authors' README (B7)

- **Thesis** (`main papers/mosaic.pdf`, read with `pdftotext`):
  - no round or byte counts in the text;
  - runtimes only in figures, at δ = 3, on c4.8xlarge servers in us-east-1/us-east-2;
  - it gives 984 explored tree nodes for the day at δ = 3.
  - Nothing is directly comparable with δ = 5.
- **README** (day, L∞, FSS, gc_gc, 32 threads on c5a.24xlarge; `README.md:76-163,339-347`):

| | README | Here (full day, `mosaic_grid`, τ 0.02) |
|---|---|---|
| server 0 sends | 2.74 GB | 2.80 GB (2.61 GiB) |
| server 1 sends | 11.6 GB | 11.89 GB (11.07 GiB) |
| all clients send | 192 MB | 196.8 MB (187.7 MiB) = 21,581 × 9,120 B + 80 B of headers |
| client time (all clients) | 2.91 s | 21,581 × 0.121 ms = 2.6 s (A6) |
| sketching | 6.86 s (32 threads) | 107 s (5 threads) |
| total | 32.4 s | 321 s online |

- **Bytes agree within 2.5%.** The README's threads only change the number of GC ≥ T instances, a small share; base OT and per-prefix GCs are the same.
- **Time:** the laptop (5 threads; 2 performance + 8 efficiency cores) is about 10× slower. Per client and thread, sketching costs about 25 ms here and about 10 ms on the authors' server.
- The README gives no rounds. **Plausible.**

### 6. Channel buffers: 64 MiB vs 256 MiB (full day, `mosaic_grid`, τ 0.02)

| Run (time) | Buffers | Online | sketch_verify / expand / check_threshold / reveal (s) | Upload | Peak memory s0 / s1 (MiB) |
|---|---|---|---|---|---|
| 18:10 | 64 MiB | 321.0 s | 107.3 / 111.9 / 97.0 / 0.4 | 1.25 s | 1,533 / 1,158 |
| 18:43 | 256 MiB | 505.0 s | 157.4 / 167.1 / 173.0 / 1.8 | 4.10 s | 2,590 / 1,288 |
| 19:59 | 64 MiB | 341.4 s | 113.9 / 118.9 / 103.5 / 0.4 | 1.26 s | 1,385 / 1,093 |
| 20:05 | 256 MiB | 346.7 s | 107.4 / 110.3 / 123.5 / 1.2 | 1.25 s | 2,385 / 1,082 |

- **Result:** back to back, 256 MiB is 1.6% slower (346.7 vs 341.4 s). Bytes and rounds are identical in all four runs. **No noticeable change.** The only real difference is server 0's memory: about +1 GB.
- **The 505 s run** slowed every step by 1.5–1.8×, including `sketch_verify`, which is local computation that never touches the buffers. So it was the machine's state, not the buffer size.
  - At the time the laptop had 2.9 GB of physical memory free, Windows' memory compression was active, VS Code used about 1.8 GB, and the power plan was Balanced.
  - I repeated the pair once, alternating.
- **Run-to-run variation** on this laptop with the current setup: 6% between the two 64 MiB runs. The same `sketch_verify` work also took 165 s in one H3 run and 224 s in another. See Deviations 8.

### 7. Full-day timing runs (`day_3of3`, N = 21,581)

| Run | Wall-clock | Online | Upload | sketch_verify / expand / check_threshold / reveal / derived_check (s) | Online rounds (sketch + crawl) | Iterations | server→server sent s0 / s1 (GB) | Peak memory s0 / s1 / client / dealer (MiB) | Self-check |
|---|---|---|---|---|---|---|---|---|---|
| `mosaic_grid` τ 0.02 | 5.5 min | 321.0 s | 1.25 s | 107.3 / 111.9 / 97.0 / 0.4 | 2,979 (5 + 2,974) | 76 | 2.80 / 11.89 | 1,533 / 1,158 / 702 / 53 | 916 cells, P 1 R 1, levels = GT |
| `geohash` τ 0.02 | 5.5 min | 320.5 s | 1.33 s | 100.0 / 115.6 / 100.0 / 0.3 | 2,954 (5 + 2,949) | 76 | 2.78 / 11.81 | 1,483 / 1,136 / 702 / 53 | 891 cells, P 1 R 1, levels = GT |
| `h3` τ 0.02 (δ 5, derived) | 15.1 min | 894.0 s | 1.81 s | 164.7 / 187.1 / 170.6 / 0.4 / 354.6 | 4,192 (5 + 4,187) | 82 | 5.40 / 23.90 | 4,463 / 3,923 / 1,076 / 79 | 721 cells, P 1 R 1, levels = GT |
| `morton` τ 0.02 | 4.9 min | 281.1 s | 1.17 s | 58.4 / 120.0 / 97.9 / 0.2 | 3,002 (5 + 2,997) | 78 | 2.82 / 11.97 | 1,415 / 1,078 / 647 / 53 | 916 cells, P 1 R 1, levels = GT |
| `h3` τ 0.02, `--h3-crawl original` | 49.4 min | 2,945.3 s | 3.70 s | 224.4 / 1,476.5 / 1,179.9 / 3.2 | 28,328 (5 + 28,323) | 657 | 35.70 / 159.59 | 2,143 / 1,788 / 1,076 / 79 | 721 cells = GT; all 9,755 points and 33 levels = plaintext 3-coordinate crawl |
| `mosaic_grid` τ 0.005 | 17.7 min | 1,048.9 s | 1.31 s | 103.7 / 510.8 / 413.4 / 0.9 | 13,461 (5 + 13,456) | 314 | 12.76 / 54.79 | 1,551 / 1,206 / 702 / 53 | 4,870 cells, P 1 R 1, levels = GT |

Notes:
- **Wall-clock** = from starting the dealer until all four processes have exited. It includes about 9 s of set-up (connection retries, client key generation, dealer pre-generation 0.2–0.35 s).
- **Upload bytes** are the same in every run of an encoding: 98.4 MB per server (H3 147.4 MB, Morton 90.5 MB). They equal N × the micro-benchmark's report size plus 40 B of headers per server.
- **Dealer:** 90 B in total (one 9-byte shutdown message per channel and server); in `extra.dealer_bytes`.
- **Online steps** (server 0's) add up to 98–99% of the online time (97.9% for the original H3 crawl). The rest is the gap between the two servers' timestamps, the conversion of the output and the dealer shutdown.
- **`expand`** includes the authors' `print_memory_usage()` in every crawl iteration (`protocol.rs:670`, `util.rs:332-344`). It builds `sysinfo::System::new_all()`, about 85 ms per call here, which makes about 6.5 s of the grid's 321 s. See Deviations 5.
- **H3's derived check** (the fork's crawl) is one `batch_check` of all 994 surviving (i, j) points: 199 per thread, 999 rounds, 355 s.
  - It also explains the memory: the crawl keeps every surviving point's per-client evaluations until that check (about 3 MB per point at N = 21,581).
- **The original H3 crawl** checks 25,620 prefixes instead of 3,826, which costs 6.7× the rounds and 6.7× the bytes. It outputs 9,034 non-cells: in `output` with lat/lon null, labelled `noncell (i, j, k)`, and counted in `extra.h3_noncell_outputs`. That equals the ground truth's `mosaic_others_without_derived.non_cells`.
- **In the WAN model** (A4), the grid's rounds add 2,979 × 50 ms = 149 s, but the byte term is larger: server 1's 11.9 GB at 100 Mbit/s = 951 s.

### 8. Estimated durations and recommendation (B6)

**Model**, per encoding: sketch ∝ N; `expand` and `check_threshold` = a·P + b·P·N, where P is the configuration's prefixes checked (from the ground truth); set-up r0 + r1·N.
- Fitted on the smoke run and the `day_3of3` run.
- It reproduces the four τ = 0.02 runs within 2%, and predicts the τ = 0.005 grid run at 1,084 s (measured 1,059 s), at 4.6× the prefixes.
- The original H3 crawl uses its own measured run, with P from the plaintext 3-coordinate crawl per subset (26,230 / 25,842 / 25,620).

| Set | Runs | Per run (1of3 / 2of3 / 3of3) | Total |
|---|---|---|---|
| **Main, τ 0.02** | 72 | grid 2.2 / 3.9 / 5.4 min; geohash 2.2 / 3.8 / 5.4; h3 5.5 / 10.2 / 14.9; morton 2.1 / 3.4 / 4.8 | **6.4 h** |
| Main with B6 (`--reps-override h3:3of3=3`) | 70 | | 5.9 h |
| Optional τ 0.005, all encodings | 72 | grid 7.8 / 13.0 / 18.1; geohash 8.2 / 13.7 / 19.0; h3 19.0 / 34.9 / 49.9; morton 7.9 / 12.9 / 17.6 | **22.2 h** |
| Optional τ 0.005, `mosaic_grid` only | 18 | 7.8 / 13.0 / 18.1 | **3.9 h** |
| Optional H3 δ = 4 (τ 0.02) | 18 | 4.1 / 7.6 / 11.0 | **2.3 h** |
| Optional H3 original crawl (τ 0.02, δ 5) | 18 | 19.6 / 33.5 / 47.3 (measured 49.4) | **10.0 h** |

These assume the machine is in the same state as during these runs. A high-performance power plan should make them shorter, not longer.

**Recommendation:**
1. **Main runs:** all 72 in one night, 6.4 h.
   - B6 says to cut a `day_3of3` configuration to 3 measured reps if one run takes more than about 10 min. That applies to H3 (15 min): `--reps-override h3:3of3=3`, 70 runs, 5.9 h.
   - H3 `day_2of3` (10.2 min) is on the line; I would keep it at 5.
   - Your decision.
2. **τ = 0.005:** the full set (22 h) does not fit one night, and H3 does not fit in memory.
   - The derived crawl's final check would hold 4,399 points × 21,581 clients, about 13 GB per server; 2of3 about 9 GB.
   - Per B6: only `mosaic_grid` at τ = 0.005, 18 runs, 3.9 h, as the single matched data point.
3. **H3 δ = 4** (2.3 h) and **grid τ = 0.005** (3.9 h) fit together in a second night (6.2 h).
4. **H3 original crawl:** 10 h for 18 runs is a night of its own.
   - As a comparison point, I would run only `day_3of3`, with 1 warm-up + 3 measured: about 3.3 h. This needs `--only-subset 3of3 --reps 3 --h3-crawl original --only-encoding h3`.
   - Or skip it, and report the one measured run (49 min, 28,328 rounds, 195 GB between the servers) as a single data point.

### 9. Client micro-benchmark (A6): first 2,000 rows of `day_3of3.csv`, 1 thread

| Encoding | mean / median / p95 (ms) | report per server (B) | upload per client (B) | mean parts: encode / FSS keys / sketch data / serialize (ms) |
|---|---|---|---|---|
| `mosaic_grid` | 0.121 / 0.110 / 0.197 | 4,560 | 9,120 | 0.0001 / 0.049 / 0.009 / 0.061 |
| `geohash` | 0.115 / 0.111 / 0.140 | 4,560 | 9,120 | 0.0003 / 0.047 / 0.008 / 0.057 |
| `h3` | 0.172 / 0.165 / 0.260 | 6,829 | 13,658 | 0.0006 / 0.071 / 0.011 / 0.085 |
| `morton` | 0.109 / 0.103 / 0.152 | 4,192 | 8,384 | 0.0001 / 0.044 / 0.008 / 0.054 |

- Per client: the same steps as `mosaic_client` (`client.rs:69-80,146-160`), timed one client at a time after a warm-up pass.
- The encoded points equal `bench/data/encodings/<enc>/day_3of3.json` for all 2,000 rows.
- The sizes equal the per-client bytes of the full runs exactly.
- Written to `mosaic_client.jsonl` (`n_clients` = k = 2,000 and `threshold_abs` = 0, as Mastic's line). The smoke run wrote them to `bench\.run\smoke\`.

### 10. B7 checklist

| Item | Status |
|---|---|
| `verify_subsets.py` passes | done in prompt 1; the runner re-checks every subset's SHA-256 against the manifest before planning |
| Encoding parameters equal A2 | done (prompt 1); run configs are written from `bench_common.ENCODINGS` |
| Input conversion reproduces `start_busiest_day.json` | done (prompt 1) |
| Reference: 916 squares, 4 areas | done (prompt 1) |
| Smoke run equals the plaintext for every encoding | **done** (Checks 1) |
| Every result line passes the schema check | **done** (Checks 3) |
| Rounds and bytes plausible against the thesis | **done**: bytes match the README within 2.5%; no rounds published (Checks 5) |
| Self-check 1.0 / 1.0 for every run | **done** for every run so far; the runner enforces it |
| One `day_3of3` run per encoding timed | **done** (Checks 7) |

## Deviations and open questions

1. **Dealer pre-generation needed a dealer change.** The decision was that the runner waits for the dealer's key pre-generation before starting the servers and the client.
   - In the code that is impossible. The dealer generates its keys only after it has connected to both servers (`dealer.rs:403-418` at HEAD), and the servers accept the dealer only after the client has connected (`mosaic_server.rs:58-106`).
   - So in the original order, the key generation (0.2–0.35 s on 5 threads) runs during the upload.
   - **Resolution:** with `--bench-out` only, the dealer generates the same keys before connecting (`pregenerate_keys`) and writes a part file. The runner waits for that file. Without the option the order is the original one.
   - With gc_gc the dealer never sends keys; it only receives the shutdown signal (90 B).
2. **Upload** = first byte written by the client (timestamp in `client.rs:103`, after the shares for server 0 are serialized) until both servers have stored the sketch data, minus the client's sketch-data generation between its two sends (B3; about 0.2 s).
   - Timestamps from different processes are compared as wall-clock time on the same machine (sub-microsecond on Windows).
   - Online = from the later of the two "reports stored" times to the later of the two "output ready" times.
3. **`online_steps` are server 0's;** server 1 records the same steps in its part file.
   - `expand` includes the bookkeeping between batches (the per-level counting runs only with `--bench-out` and is negligible).
   - `derived_check` is the whole final H3 check, its `batch_check` included, so it is not counted in `check_threshold`/`reveal` as well.
4. **The GC and OT turns are constants from reading swanky's code**, because their messages are sent inside the library and the channel code may only get the buffer change.
   - They were checked against the channel (Checks 4).
   - They hold for swanky rev 553ede0. If swanky changes, re-check them with a channel-level count.
5. **The authors' per-iteration `print_memory_usage()`** costs about 85 ms per crawl iteration on Windows. That is 2% of the online time at τ 0.02 and 3% at τ 0.005; in the original H3 crawl it is about 56 s.
   - I left it: it is the authors' code, and removing it would change the measured system.
   - If you prefer to exclude it, it can be gated behind the bench flag, but that is a change to the authors' code path; your decision.
6. **B6 for H3 `day_3of3` (15 min).** I added `--reps-override ENC:SUBSET=N` so the reduction can be applied without changing the loop order. The default is unchanged (5 for everything). Your decision whether to use `h3:3of3=3` (Checks 8).
7. **Optional runs:** see the recommendation in Checks 8. H3 at τ = 0.005 is not feasible on this laptop (memory) for 2of3 and 3of3.
8. **Machine state (A8).** During these checks the power plan was Balanced, not high-performance, and VS Code (about 1.8 GB) and other applications were open. Free memory was 2.9–3.7 GB.
   - Run-to-run variation was 6% between two identical runs, with one 1.6× outlier.
   - Before the full benchmark: set the high-performance plan, close VS Code and other heavy applications, and keep the laptop plugged in.
   - The runner does not change system settings.
9. **Mosaic's default 1 GiB channel buffers cannot run on this laptop** (allocation failure; Checks 2). Every benchmark run therefore needs `MOSAIC_CHANNEL_BUFFER_BYTES`. The runner sets 64 MiB and records it in `extra.channel_buffer_bytes`.
10. **A9 field choices:**
    - `server_offline_ms` = 0 (B3);
    - `collector_to_server` / `server_to_collector` = [0, 0] (no collector);
    - `server_to_server` = [sent by server 0, sent by server 1] over the whole online phase, which also includes the sketch verification (`extra.bytes_sketch_verify` gives that part);
    - `client_validity_bits` = N;
    - `revealed_kind` = `"bit"`.
    - `extra` holds: `channel_buffer_bytes`, `dealer_bytes`, `rounds_sketch_verify`, `rounds_crawl` (+ `rounds_derived_check` for the derived H3 crawl), `crawl_iterations`, `bytes_sketch_verify`, `client_sketch_gen_ms`, `clients_rejected`, and for H3 `h3_noncell_outputs`.
    - H3's `params.encoding_params` gets `h3_crawl` (`derived` or `original`). With `original`, `derived_coordinate` is left out, as in the config.
11. **Run configs use h3 = 15 bits for the count ring at every N** (the repo's day configs). 2^15 > 21,581, and the runner checks N < 2^h3.
12. **The client line's `keygen_ms_*` is the whole report generation** (encode + FSS keys + sketch data + serialization), as A3 defines client report generation. The micro-benchmark uses the run's δ.
13. **For prompt 3:**
    - the copied `evaluate.py` also uses `bc.BASE_CONFIG` and `bc.checker_module`, which are Mastic-specific and not in Mosaic's `bench_common.py` yet;
    - Mastic's `bench_common.py` still has to accept the v3.1 additions (open since report 01).
14. **Default `--port-base` is 18300** (Mastic: 18100). Each run uses 17 ports, rotating over 4 blocks (18300–18379), so no run waits for the previous run's sockets.
15. **Timing and diagnostic lines** are in `bench\.run\{timing,buffercheck,smoke,turncheck}\` (ignored by Git), not in `bench\results\raw\`. The full benchmark writes there.

## What to commit

```
git add .gitignore Cargo.toml bench/README.md bench/scripts/bench_common.py bench/scripts/run_bench.py bench/reports/02-instrumentation.md src/bench.rs src/bin/benchmarks/client_report_bench.rs src/bin/mosaic_end_to_end/mosaic_client.rs src/bin/mosaic_end_to_end/mosaic_dealer.rs src/bin/mosaic_end_to_end/mosaic_server.rs src/channel.rs src/fuzzy_match/check_phase.rs src/fuzzy_match/client.rs src/fuzzy_match/dealer.rs src/fuzzy_match/protocol.rs src/fuzzy_match/sketch_phase.rs src/fuzzy_match/threshold_phase.rs src/lib.rs
```

Commit message: `Instrument Mosaic for the benchmark (A9 lines, rounds, bytes, leakage), add the client micro-benchmark and run_bench.py; add report 02`

Left out: `bench/.run/` (runner output), `target/`, `.venv/`.
