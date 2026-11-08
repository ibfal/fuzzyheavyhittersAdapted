//! Location encodings: how a pickup `(lat, lon)` becomes the coordinates of a Mosaic client point.
//!
//! Mosaic compares points coordinate by coordinate (L∞, L1 or L2 against `delta`), so an encoding
//! must produce integer coordinates whose differences track physical distance. Every coordinate
//! must fit in `axis_bits` bits (the config's `h1`), and a location has `dims` coordinates (the
//! config's `d`, doubled when start and end points are both encoded).
//!
//! Unlike Mastic, bit interleaving does not matter here: the unknown-dictionary crawl already
//! takes one bit of each coordinate in turn.

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod cli;
pub mod geohash;
mod h3;
mod morton;
mod mosaic_grid;

pub use self::h3::H3;
pub use geohash::Geohash;
pub use morton::Morton;
pub use mosaic_grid::MosaicGrid;

/// Which encoding to use, with its parameters. Stored next to every generated dataset so the
/// heavy hitters can be decoded back to locations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Encoding {
    /// Mosaic's original RideAustin grid: 0.001 degree cells, 11 bits per axis, out-of-grid
    /// points clamped to the edge.
    MosaicGrid,

    /// A uniform grid over a bounding box, `2^axis_bits` cells per axis; points outside the box
    /// are dropped. The edges are inclusive at `min` and exclusive at `max`, in degrees.
    Morton {
        min_lat: f64,
        max_lat: f64,
        min_lon: f64,
        max_lon: f64,
        axis_bits: usize,
    },

    /// Standard geohash cells inside the cell `prefix` (base32, e.g. "9v6" for Austin), `bits`
    /// bits below it; points outside the prefix cell are dropped.
    Geohash { prefix: String, bits: usize },

    /// H3 cells at `resolution` as cube coordinates `[i, j, i - j]` (each shifted by
    /// `2^(axis_bits - 1)`) around the cell containing the origin; cells too far from it are
    /// dropped.
    H3 {
        resolution: u8,
        origin_lat: f64,
        origin_lon: f64,
        axis_bits: usize,
    },
}

impl Encoding {
    pub fn encoder(&self) -> Box<dyn LocationEncoder> {
        match self {
            Encoding::MosaicGrid => Box::new(MosaicGrid),
            Encoding::Morton {
                min_lat,
                max_lat,
                min_lon,
                max_lon,
                axis_bits,
            } => Box::new(Morton::new(
                *min_lat, *max_lat, *min_lon, *max_lon, *axis_bits,
            )),
            Encoding::Geohash { prefix, bits } => Box::new(Geohash::new(prefix, *bits)),
            Encoding::H3 {
                resolution,
                origin_lat,
                origin_lon,
                axis_bits,
            } => Box::new(H3::new(*resolution, *origin_lat, *origin_lon, *axis_bits)),
        }
    }

    /// Short name used in directory names and logs.
    pub fn name(&self) -> &'static str {
        match self {
            Encoding::MosaicGrid => "mosaic_grid",
            Encoding::Morton { .. } => "morton",
            Encoding::Geohash { .. } => "geohash",
            Encoding::H3 { .. } => "h3",
        }
    }
}

/// What happened to one location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoded {
    /// The location lies inside the encoded area.
    Inside(Vec<u128>),
    /// The location lies outside the area and was moved to the nearest edge cell.
    Clamped(Vec<u128>),
    /// The location lies outside the area; the ride is not sent.
    Dropped,
}

pub trait LocationEncoder {
    /// Number of coordinates per location.
    fn dims(&self) -> usize;

    /// Bits per coordinate. Every coordinate is below `2^axis_bits`.
    fn axis_bits(&self) -> usize;

    fn encode(&self, lat: f64, lon: f64) -> Encoded;

    /// Center `(lat, lon)` of the cell named by `coords`, or `None` if no location encodes to
    /// it (for example an H3 triple whose third coordinate is not `i - j`).
    fn decode(&self, coords: &[u128]) -> Option<(f64, f64)>;

    /// Human-readable name of the cell named by `coords`.
    fn label(&self, coords: &[u128]) -> String;
}

/// Which locations of a ride become the client point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointKind {
    /// The pickup only.
    Start,
    /// The pickup followed by the drop-off.
    StartEnd,
}

impl PointKind {
    /// `0` is the pickup only and `1` is pickup and drop-off, matching `--output-type`.
    pub fn from_output_type(output_type: u8) -> Result<Self, Box<dyn Error>> {
        match output_type {
            0 => Ok(PointKind::Start),
            1 => Ok(PointKind::StartEnd),
            _ => Err(format!("unsupported output_type: {}", output_type).into()),
        }
    }

    pub fn locations(&self) -> usize {
        match self {
            PointKind::Start => 1,
            PointKind::StartEnd => 2,
        }
    }
}

/// Everything needed to set up a config for, and decode the results of, an encoded dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodingMetadata {
    pub encoding: Encoding,
    pub point_kind: PointKind,
    /// Coordinates per client point (the config's `d`).
    pub d: usize,
    /// Bits per coordinate (the config's `h1`).
    pub h1: usize,
    /// Rides in the CSV.
    pub total_rides: usize,
    /// Rides written to the dataset (the config's `num_clients`).
    pub kept_rides: usize,
    /// Kept rides with at least one location moved to the edge of the area.
    pub clamped_rides: usize,
    /// Rides with a location outside the area; they are not in the dataset.
    pub dropped_rides: usize,
}

pub struct EncodedDataset {
    pub points: Vec<Vec<u128>>,
    pub metadata: EncodingMetadata,
}

/// The columns of a RideAustin CSV row that are used. Columns are matched by header name.
#[derive(Debug, Deserialize)]
struct Ride {
    start_location_lat: f64,
    start_location_long: f64,
    end_location_lat: f64,
    end_location_long: f64,
}

/// Read a RideAustin CSV and encode every ride, in file order. Rides with a location outside the
/// area are left out and counted in the metadata.
pub fn encode_csv<P: AsRef<Path>>(
    path: P,
    point_kind: PointKind,
    encoding: &Encoding,
) -> Result<EncodedDataset, Box<dyn Error>> {
    let encoder = encoding.encoder();
    let mut reader = csv::Reader::from_path(path)?;
    let mut points = Vec::new();
    let (mut total, mut clamped, mut dropped) = (0, 0, 0);

    for ride in reader.deserialize() {
        let ride: Ride = ride?;
        total += 1;
        let mut locations = vec![(ride.start_location_lat, ride.start_location_long)];
        if point_kind == PointKind::StartEnd {
            locations.push((ride.end_location_lat, ride.end_location_long));
        }

        let mut point = Vec::with_capacity(encoder.dims() * locations.len());
        let mut was_clamped = false;
        let mut was_dropped = false;
        for (lat, lon) in locations {
            match encoder.encode(lat, lon) {
                Encoded::Inside(coords) => point.extend(coords),
                Encoded::Clamped(coords) => {
                    was_clamped = true;
                    point.extend(coords);
                }
                Encoded::Dropped => {
                    was_dropped = true;
                    break;
                }
            }
        }

        if was_dropped {
            dropped += 1;
        } else {
            clamped += was_clamped as usize;
            points.push(point);
        }
    }

    let metadata = EncodingMetadata {
        encoding: encoding.clone(),
        point_kind,
        d: encoder.dims() * point_kind.locations(),
        h1: encoder.axis_bits(),
        total_rides: total,
        kept_rides: points.len(),
        clamped_rides: clamped,
        dropped_rides: dropped,
    };
    Ok(EncodedDataset { points, metadata })
}

/// Where the metadata of the dataset at `points_path` is stored: `start_busiest_day.json` gets
/// `start_busiest_day.encoding.json` next to it.
pub fn metadata_path<P: AsRef<Path>>(points_path: P) -> PathBuf {
    let points_path = points_path.as_ref();
    let stem = points_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    points_path.with_file_name(format!("{}.encoding.json", stem))
}

pub fn write_metadata<P: AsRef<Path>>(
    points_path: P,
    metadata: &EncodingMetadata,
) -> Result<PathBuf, Box<dyn Error>> {
    let path = metadata_path(points_path);
    let writer = BufWriter::new(File::create(&path)?);
    serde_json::to_writer_pretty(writer, metadata)?;
    Ok(path)
}

pub fn read_metadata<P: AsRef<Path>>(path: P) -> Result<EncodingMetadata, Box<dyn Error>> {
    let file = File::open(path)?;
    Ok(serde_json::from_reader(file)?)
}

/// Print how many rides were kept, clamped and dropped.
pub fn print_summary(metadata: &EncodingMetadata) {
    println!("Encoding: {:?}", metadata.encoding);
    println!(
        "  d = {}, h1 = {} (coordinates per point, bits per coordinate)",
        metadata.d, metadata.h1
    );
    println!(
        "  rides: {} total, {} kept, {} clamped to the edge, {} dropped",
        metadata.total_rides, metadata.kept_rides, metadata.clamped_rides, metadata.dropped_rides
    );
}

/// Split a client point into the coordinates of each of its locations.
pub fn split_locations<'a>(
    metadata: &EncodingMetadata,
    point: &'a [u128],
) -> impl Iterator<Item = &'a [u128]> {
    let dims = metadata.d / metadata.point_kind.locations();
    point.chunks(dims)
}

#[cfg(test)]
mod tests;
