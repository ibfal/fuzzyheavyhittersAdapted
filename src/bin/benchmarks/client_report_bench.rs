//! Client micro-benchmark (bench/BENCHMARK_MOSAIC.md A6): the time one client needs to turn its
//! location into its reports for both servers, on one thread.
//!
//! Per client, exactly what `mosaic_client` does per client (`client.rs:69-80,146-160`):
//! 1. encode the pickup with the dataset's encoding (`LocationEncoder::encode`);
//! 2. one interval-FSS key pair per coordinate (`SharePhase::share_range`);
//! 3. d sketch-data pairs, from a fresh random PRG seed (`SketchPhase::get_sketch_data`);
//! 4. serialization of both servers' reports (`SharedRange::to_bytes`, `SketchData::to_bytes`).
//!
//! The first K rows of the CSV; one untimed warm-up pass over them, then every client is timed
//! on its own. Writes one A9 client line (mean, median, p95 nearest rank, report bytes per
//! server) to `--out`; the header comes from `--bench-meta`.
//!
//!     mosaic_client_bench --config <run config.json> --csv bench/data/day_3of3.csv
//!         --metadata bench/data/encodings/<enc>/day_3of3.encoding.json
//!         [--points bench/data/encodings/<enc>/day_3of3.json] --clients 2000
//!         --out <file.jsonl> --bench-meta @<meta.json>

use clap::Parser;
use mosaic::{
    configs::cli_config::CliConfig,
    data_toolkit::encoding::{read_metadata, Encoded},
    fuzzy_match::{share_phase::SharePhase, sketch_phase::SketchPhase},
    randomness::prg::PRG,
};
use rand::RngExt;
use serde_json::{json, Value};
use std::hint::black_box;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(about = "Mosaic client report generation, per client, on one thread (A6)")]
struct Args {
    /// A run config (protocol parameters: d, h1, h2, delta, sketch modulus, ...).
    #[arg(long)]
    config: String,
    /// Ride CSV; the first --clients rows are used.
    #[arg(long)]
    csv: String,
    /// The dataset's `.encoding.json` (which encoding, with its parameters).
    #[arg(long)]
    metadata: String,
    /// Optional: the converted points of the same CSV; the encoded points must equal them.
    #[arg(long)]
    points: Option<String>,
    #[arg(long, default_value_t = 2000)]
    clients: usize,
    /// JSONL file the result line is appended to.
    #[arg(long)]
    out: String,
    /// The line's header (system, encoding, dataset, subset, rep, warmup, timestamp, params,
    /// env), as `@file`, a file path or inline JSON.
    #[arg(long)]
    bench_meta: String,
}

/// Time spent in each part of one client's report generation.
struct Parts {
    encode: Duration,
    share: Duration,
    sketch: Duration,
    serialize: Duration,
}

/// One client's reports: (bytes for server 0, bytes for server 1, encoded point, parts).
fn generate(
    encoder: &dyn mosaic::data_toolkit::encoding::LocationEncoder,
    share_phase: &SharePhase,
    sketch_phase: &SketchPhase,
    d: usize,
    delta: u128,
    lat: f64,
    lon: f64,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u128>, Parts), String> {
    let t0 = Instant::now();
    let point = match encoder.encode(lat, lon) {
        Encoded::Inside(c) | Encoded::Clamped(c) => c,
        Encoded::Dropped => return Err(format!("({}, {}) lies outside the encoded area", lat, lon)),
    };
    let t1 = Instant::now();
    let (share0, share1) = share_phase
        .share_range(&point, delta)
        .map_err(|e| format!("share_range: {:?}", e))?;
    let t2 = Instant::now();
    let mut prg = PRG::new(Some(&rand::rng().random::<[u8; 16]>()), 0);
    let mut sketches = Vec::with_capacity(d);
    for _ in 0..d {
        sketches.push(sketch_phase.get_sketch_data(&mut prg).map_err(|e| e.to_string())?);
    }
    let t3 = Instant::now();
    let mut bytes0 = share0.to_bytes();
    let mut bytes1 = share1.to_bytes();
    for (s0, s1) in &sketches {
        bytes0.extend_from_slice(&s0.to_bytes());
        bytes1.extend_from_slice(&s1.to_bytes());
    }
    let t4 = Instant::now();
    let parts = Parts { encode: t1 - t0, share: t2 - t1, sketch: t3 - t2, serialize: t4 - t3 };
    Ok((bytes0, bytes1, point, parts))
}

/// Mean, median and p95 (nearest rank) in milliseconds (as Mastic's keygen_bench).
fn summary_ms(durations: &[Duration]) -> (f64, f64, f64) {
    let mut ms: Vec<f64> = durations.iter().map(|d| d.as_secs_f64() * 1e3).collect();
    ms.sort_by(f64::total_cmp);
    let n = ms.len();
    let median = if n % 2 == 1 { ms[n / 2] } else { (ms[n / 2 - 1] + ms[n / 2]) / 2.0 };
    let p95 = ms[((0.95 * n as f64).ceil() as usize).max(1) - 1];
    (ms.iter().sum::<f64>() / n as f64, median, p95)
}

fn mean_ms(durations: impl Iterator<Item = Duration>) -> f64 {
    let (sum, n) = durations.fold((0.0, 0usize), |(s, n), d| (s + d.as_secs_f64(), n + 1));
    sum / n as f64 * 1e3
}

/// Client times are small: keep 0.1 µs.
fn round_ms(ms: f64) -> f64 {
    (ms * 1e4).round() / 1e4
}

/// (lat, lon) of the pickup of the first `k` rows.
fn read_pickups(path: &str, k: usize) -> Result<Vec<(f64, f64)>, String> {
    let mut reader = csv::Reader::from_path(path).map_err(|e| format!("{}: {}", path, e))?;
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let col = |name: &str| {
        headers.iter().position(|h| h == name).ok_or_else(|| format!("{}: no column {}", path, name))
    };
    let (lat_col, lon_col) = (col("start_location_lat")?, col("start_location_long")?);
    let mut out = Vec::with_capacity(k);
    for record in reader.records().take(k) {
        let record = record.map_err(|e| e.to_string())?;
        let parse = |i: usize| record[i].parse::<f64>().map_err(|e| format!("{}: {}", path, e));
        out.push((parse(lat_col)?, parse(lon_col)?));
    }
    if out.len() < k {
        return Err(format!("{} has only {} rows, {} needed", path, out.len(), k));
    }
    Ok(out)
}

fn run(args: Args) -> Result<(), String> {
    // A6: one thread. The client code is sequential; this also fixes any Rayon use to 1 thread.
    rayon::ThreadPoolBuilder::new().num_threads(1).build_global().map_err(|e| e.to_string())?;

    let cli = CliConfig::from_file(&args.config)?;
    let share_config = cli.to_share_config()?;
    let (d, delta) = (share_config.d, share_config.delta);
    let share_phase = SharePhase::new(share_config.clone());
    let sketch_phase = SketchPhase::new(share_config);
    let metadata = read_metadata(&args.metadata).map_err(|e| format!("{}: {}", args.metadata, e))?;
    if metadata.d != d {
        return Err(format!("{} has d = {}, the config d = {}", args.metadata, metadata.d, d));
    }
    let encoder = metadata.encoding.encoder();
    let pickups = read_pickups(&args.csv, args.clients)?;
    let k = pickups.len();

    // Warm-up pass, untimed; it also checks the points against the converted dataset.
    let expected: Option<Vec<Vec<u128>>> = match &args.points {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
            Some(serde_json::from_str(&text).map_err(|e| format!("{}: {}", path, e))?)
        }
        None => None,
    };
    for (i, &(lat, lon)) in pickups.iter().enumerate() {
        let (b0, b1, point, _) = generate(encoder.as_ref(), &share_phase, &sketch_phase, d, delta, lat, lon)?;
        if let Some(exp) = &expected {
            if exp.get(i) != Some(&point) {
                return Err(format!("row {}: encoded {:?}, the dataset has {:?}", i, point, exp.get(i)));
            }
        }
        black_box((b0, b1));
    }

    // Timed pass: one client at a time.
    let mut totals = Vec::with_capacity(k);
    let mut parts = Vec::with_capacity(k);
    let mut sizes = Vec::with_capacity(k);
    for &(lat, lon) in &pickups {
        let start = Instant::now();
        let (b0, b1, point, p) =
            generate(encoder.as_ref(), &share_phase, &sketch_phase, d, delta, lat, lon)?;
        totals.push(start.elapsed());
        sizes.push([b0.len() as u64, b1.len() as u64]);
        black_box((b0, b1, point));
        parts.push(p);
    }
    let (mean, median, p95) = summary_ms(&totals);
    if sizes.iter().any(|s| *s != sizes[0]) {
        return Err("report sizes differ between clients".to_string());
    }
    let report = sizes[0];

    println!(
        "client_report_bench: {:?}, first {} rows of {}, 1 thread",
        metadata.encoding, k, args.csv
    );
    println!("  per client (ms): mean {:.4}, median {:.4}, p95 {:.4}", mean, median, p95);
    println!(
        "    means: encode {:.4} | share_range {:.4} | sketch data {:.4} | serialize {:.4}",
        mean_ms(parts.iter().map(|p| p.encode)),
        mean_ms(parts.iter().map(|p| p.share)),
        mean_ms(parts.iter().map(|p| p.sketch)),
        mean_ms(parts.iter().map(|p| p.serialize)),
    );
    println!("  report per server: {} / {} B", report[0], report[1]);

    let meta = mosaic::bench::read_meta(&args.bench_meta)?;
    let mut line = meta.clone();
    let obj = line.as_object_mut().unwrap();
    obj.insert("n_clients".into(), json!(k));
    obj.insert(
        "client".into(),
        json!({
            "k": k,
            "keygen_ms_mean": round_ms(mean),
            "keygen_ms_median": round_ms(median),
            "keygen_ms_p95": round_ms(p95),
            "report_bytes_per_server": report,
            "upload_bytes_per_client": report[0] + report[1],
        }),
    );
    let mut extra = meta.get("extra").and_then(Value::as_object).cloned().unwrap_or_default();
    extra.insert(
        "client_ms_mean_parts".into(),
        json!({
            "encode": round_ms(mean_ms(parts.iter().map(|p| p.encode))),
            "share_range": round_ms(mean_ms(parts.iter().map(|p| p.share))),
            "sketch_data": round_ms(mean_ms(parts.iter().map(|p| p.sketch))),
            "serialize": round_ms(mean_ms(parts.iter().map(|p| p.serialize))),
        }),
    );
    obj.insert("extra".into(), Value::Object(extra));
    mosaic::bench::append_jsonl(&args.out, &line)?;
    println!("Appended the result to {}", args.out);
    Ok(())
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
