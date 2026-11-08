//! Save the unknown-dictionary crawl of one config for the crawl viewer in `viz/`.
//!
//! Reads a Mosaic config (`data_file`, `delta`, `match_threshold`, `h1`, `d`, `distance_metric`)
//! and the `.encoding.json` next to its data file, runs the crawl in plaintext (the same search
//! as `mosaic_others ground-truth`) and writes every level, with the map region of every
//! candidate prefix, to `--output`.

use clap::Parser;
use mosaic::data_toolkit::{
    crawl::{crawl, prefix_bits, prefix_ranges, Metric},
    encoding::{metadata_path, read_metadata, Encoding, EncodingMetadata, PointKind},
};
use serde_json::{json, Value};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// A Mosaic config, e.g. configs/encodings/h3/busiest_day/linf_fss_config.json.
    #[arg(long)]
    config: String,
    /// The dataset's `.encoding.json` (default: next to the config's `data_file`).
    #[arg(long)]
    metadata: Option<String>,
    /// Where to write the crawl, e.g. viz/results/busiest_day-linf-h3.json.
    #[arg(long)]
    output: String,
}

/// Round to 5 decimals (about 1 m) to keep the files small.
fn round(v: f64) -> f64 {
    (v * 1e5).round() / 1e5
}

fn polygons(rings: Vec<Vec<(f64, f64)>>) -> Value {
    Value::Array(
        rings
            .into_iter()
            .map(|ring| json!(ring.into_iter().map(|(a, b)| [round(a), round(b)]).collect::<Vec<_>>()))
            .collect(),
    )
}

/// The metadata of the config's dataset. The original grid datasets from Zenodo have none; a
/// 2-coordinate, 11-bit config without metadata is taken to be the original grid.
fn load_metadata(
    args: &Args,
    data_file: &str,
    d: usize,
    h1: usize,
    points: usize,
) -> Result<EncodingMetadata, Box<dyn Error>> {
    let path = args
        .metadata
        .clone()
        .unwrap_or_else(|| metadata_path(data_file).to_string_lossy().into_owned());
    if Path::new(&path).exists() {
        return read_metadata(&path);
    }
    if d == 2 && h1 == 11 {
        eprintln!("{} not found; assuming the original Mosaic grid", path);
        return Ok(EncodingMetadata {
            encoding: Encoding::MosaicGrid,
            point_kind: PointKind::Start,
            d,
            h1,
            total_rides: points,
            kept_rides: points,
            clamped_rides: 0,
            dropped_rides: 0,
        });
    }
    Err(format!(
        "{} not found: generate the dataset with ride_austin_json_generator, which writes it",
        path
    )
    .into())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let start = Instant::now();
    let config: Value = serde_json::from_str(&fs::read_to_string(&args.config)?)?;
    let protocol = &config["protocol"];
    let field = |name: &str| -> Result<u64, Box<dyn Error>> {
        protocol[name]
            .as_u64()
            .ok_or_else(|| format!("config has no protocol.{}", name).into())
    };
    let data_file = config["data_file"]
        .as_str()
        .ok_or("config has no data_file")?
        .to_string();
    let metric_name = protocol["distance_metric"]
        .as_str()
        .ok_or("config has no protocol.distance_metric")?;
    let metric = Metric::parse(metric_name)?;
    let (delta, threshold) = (field("delta")? as u128, field("match_threshold")? as usize);
    let (d, h1) = (field("d")? as usize, field("h1")? as usize);

    let points: Vec<Vec<u128>> = serde_json::from_str(&fs::read_to_string(&data_file)?)?;
    let metadata = load_metadata(&args, &data_file, d, h1, points.len())?;
    if metadata.point_kind != PointKind::Start || metadata.d != d || metadata.h1 != h1 {
        return Err(format!(
            "the viewer draws pickup points only, and the config (d = {}, h1 = {}) must match \
             the dataset ({:?}, d = {}, h1 = {})",
            d, h1, metadata.point_kind, metadata.d, metadata.h1
        )
        .into());
    }
    let encoder = metadata.encoding.encoder();

    println!(
        "Crawling {} ({} clients, {}, delta {}, threshold {})",
        data_file,
        points.len(),
        metric_name,
        delta,
        threshold
    );
    let levels = crawl(&points, d, h1, metric, delta, threshold);

    let mut heavy_hitters = Vec::new();
    let mut non_cells = 0;
    if let Some(last) = levels.last().filter(|l| l.level == d * h1) {
        for c in last.candidates.iter().filter(|c| c.heavy) {
            let point: Vec<u128> = c.prefix.iter().map(|&(v, _)| v).collect();
            match encoder.decode(&point) {
                Some((lat, lon)) => heavy_hitters.push(json!({
                    "point": point.iter().map(u128::to_string).collect::<Vec<_>>(),
                    "label": encoder.label(&point),
                    "center": [round(lat), round(lon)],
                    "count": c.count,
                })),
                None => non_cells += 1,
            }
        }
    }

    let levels_json: Vec<Value> = levels
        .iter()
        .map(|level| {
            let candidates: Vec<Value> = level
                .candidates
                .iter()
                .map(|c| {
                    json!({
                        "prefix": c.prefix.iter().map(|&(v, len)| prefix_bits(v, len)).collect::<Vec<_>>(),
                        "count": c.count,
                        "heavy": c.heavy,
                        "region": polygons(encoder.region(&prefix_ranges(&c.prefix, h1))),
                    })
                })
                .collect();
            json!({
                "level": level.level,
                "dim": level.dim,
                "seconds": level.seconds,
                "candidates": candidates,
            })
        })
        .collect();

    let dataset = Path::new(&data_file)
        .file_stem()
        .map(|s| s.to_string_lossy().trim_start_matches("start_").to_string())
        .unwrap_or_default();
    let prefixes: usize = levels.iter().map(|l| l.candidates.len()).sum();
    let out = json!({
        "format": "mosaic-crawl/1",
        "dataset": dataset,
        "config": args.config,
        "encoding": metadata.encoding,
        "encoding_name": metadata.encoding.name(),
        "metric": metric_name,
        "delta": delta as u64,
        "threshold": threshold,
        "num_clients": points.len(),
        "d": d,
        "h1": h1,
        "rides": {
            "total": metadata.total_rides,
            "clamped": metadata.clamped_rides,
            "dropped": metadata.dropped_rides,
        },
        "prefixes_checked": prefixes,
        "non_cell_heavy_hitters": non_cells,
        "heavy_hitters": heavy_hitters,
        "levels": levels_json,
        "total_seconds": start.elapsed().as_secs_f64(),
    });
    if let Some(dir) = Path::new(&args.output).parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&args.output, serde_json::to_string(&out)?)?;
    println!(
        "{} levels, {} prefixes checked, {} heavy hitters ({} points that are not cells) -> {}",
        levels.len(),
        prefixes,
        heavy_hitters.len(),
        non_cells,
        args.output
    );
    Ok(())
}
