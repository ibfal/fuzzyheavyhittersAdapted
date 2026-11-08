//! Turn heavy hitters (integer points) back into locations.
//!
//! Reads either the output file of `mosaic_others ground-truth` (its `heavy_hitters` field, or
//! `query_point`s marked `is_heavy_hitter` for the known dictionary) or a plain JSON list of
//! points, plus the `.encoding.json` written next to the dataset, and prints or writes a CSV of
//! the cell label and center of every heavy hitter. Points that no location encodes to (an H3
//! triple whose third coordinate is not `i - j`) are counted and left out.

use clap::Parser;
use mosaic::data_toolkit::encoding::{read_metadata, split_locations};
use serde_json::Value;
use std::error::Error;
use std::fs;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Results JSON from `mosaic_others ground-truth`, or a JSON list of points.
    #[arg(long)]
    input: String,
    /// The `.encoding.json` written next to the dataset by `ride_austin_json_generator`.
    #[arg(long)]
    metadata: String,
    /// Write a CSV here instead of printing.
    #[arg(long)]
    output: Option<String>,
}

/// A coordinate stored as a JSON number or as a string of digits.
fn to_u128(v: &Value) -> Result<u128, Box<dyn Error>> {
    match v {
        Value::String(s) => Ok(s.parse()?),
        Value::Number(n) => n
            .as_u64()
            .map(u128::from)
            .ok_or_else(|| format!("not a coordinate: {}", n).into()),
        _ => Err(format!("not a coordinate: {}", v).into()),
    }
}

fn to_point(v: &Value) -> Result<Vec<u128>, Box<dyn Error>> {
    v.as_array()
        .ok_or_else(|| format!("not a point: {}", v))?
        .iter()
        .map(to_u128)
        .collect()
}

fn load_points(input: &str) -> Result<Vec<Vec<u128>>, Box<dyn Error>> {
    let json: Value = serde_json::from_str(&fs::read_to_string(input)?)?;
    let points: Vec<&Value> = if let Some(list) = json.as_array() {
        list.iter().collect()
    } else if let Some(list) = json.get("heavy_hitters").and_then(Value::as_array) {
        list.iter().collect()
    } else if let Some(list) = json.get("results").and_then(Value::as_array) {
        list.iter()
            .filter(|r| r.get("is_heavy_hitter").and_then(Value::as_bool) == Some(true))
            .filter_map(|r| r.get("query_point"))
            .collect()
    } else {
        return Err(
            "expected a list of points or a ground-truth output with `heavy_hitters` or `results`"
                .into(),
        );
    };
    points.into_iter().map(to_point).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let metadata = read_metadata(&args.metadata)?;
    let encoder = metadata.encoding.encoder();
    let points = load_points(&args.input)?;

    let mut rows = Vec::new();
    let mut invalid = 0;
    for point in &points {
        if point.len() != metadata.d {
            return Err(format!(
                "point {:?} has {} coordinates but the encoding has d = {}",
                point,
                point.len(),
                metadata.d
            )
            .into());
        }
        let decoded: Option<Vec<(String, f64, f64)>> = split_locations(&metadata, point)
            .map(|coords| {
                encoder
                    .decode(coords)
                    .map(|(lat, lon)| (encoder.label(coords), lat, lon))
            })
            .collect();
        match decoded {
            Some(locations) => rows.push((point.clone(), locations)),
            None => invalid += 1,
        }
    }

    let mut csv = String::from("point");
    for i in 0..metadata.point_kind.locations() {
        csv.push_str(&format!(",cell_{i},lat_{i},lon_{i}"));
    }
    csv.push('\n');
    for (point, locations) in &rows {
        let coords: Vec<String> = point.iter().map(u128::to_string).collect();
        csv.push_str(&coords.join(" "));
        for (label, lat, lon) in locations {
            csv.push_str(&format!(",\"{label}\",{lat:.6},{lon:.6}"));
        }
        csv.push('\n');
    }

    match &args.output {
        Some(path) => {
            fs::write(path, &csv)?;
            println!("Wrote {} heavy hitters to {}", rows.len(), path);
        }
        None => print!("{csv}"),
    }
    eprintln!(
        "{} heavy hitters decoded, {} points left out because no location encodes to them",
        rows.len(),
        invalid
    );
    Ok(())
}
