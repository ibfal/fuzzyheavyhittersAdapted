//! Morton (Z-order) grid over a configurable bounding box.

use super::{Encoded, LocationEncoder};

/// Each axis of the box `[min, max)` is split into `2^axis_bits` equal cells, and a point is
/// `[lat_idx, lon_idx]`. Mastic interleaves the two indices into one Morton code; Mosaic's crawl
/// already alternates between the coordinates, so the indices are sent as they are. Points
/// outside the box are dropped.
///
/// The default box, 29.9005 to 30.9245 by -98.0505 to -97.0265 with 10 bits per axis, gives
/// 0.001 degree cells whose edges sit half a step off the 3-decimal RideAustin coordinates: the
/// cells are exactly Mosaic's grid cells, shifted by (634, 693), with one bit less per axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Morton {
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    axis_bits: usize,
}

impl Morton {
    pub fn new(min_lat: f64, max_lat: f64, min_lon: f64, max_lon: f64, axis_bits: usize) -> Self {
        assert!(
            [min_lat, max_lat, min_lon, max_lon]
                .iter()
                .all(|v| v.is_finite())
                && min_lat < max_lat
                && min_lon < max_lon,
            "invalid Morton bounding box: lat [{}, {}), lon [{}, {})",
            min_lat,
            max_lat,
            min_lon,
            max_lon
        );
        assert!(
            (1..=31).contains(&axis_bits),
            "Morton axis_bits must be between 1 and 31, got {}",
            axis_bits
        );
        Self {
            min_lat,
            max_lat,
            min_lon,
            max_lon,
            axis_bits,
        }
    }

    /// Index of the cell containing `v` on the axis `[min, max)`, or `None` outside it
    /// (including NaN). Same expression as Mastic's encoder, so both give the same cells.
    fn axis_cell(&self, v: f64, min: f64, max: f64) -> Option<u32> {
        if !(min <= v && v < max) {
            return None;
        }
        let cells = (1u64 << self.axis_bits) as f64;
        let idx = ((v - min) / (max - min) * cells).floor() as u64;
        // Rounding can put a point just below `max` at index `cells`.
        Some(idx.min((1u64 << self.axis_bits) - 1) as u32)
    }

    /// Center of cell `idx` on the axis `[min, max)`.
    fn axis_center(&self, idx: u128, min: f64, max: f64) -> f64 {
        let cells = (1u64 << self.axis_bits) as f64;
        min + (max - min) * (idx as f64 + 0.5) / cells
    }
}

impl LocationEncoder for Morton {
    fn dims(&self) -> usize {
        2
    }

    fn axis_bits(&self) -> usize {
        self.axis_bits
    }

    fn encode(&self, lat: f64, lon: f64) -> Encoded {
        match (
            self.axis_cell(lat, self.min_lat, self.max_lat),
            self.axis_cell(lon, self.min_lon, self.max_lon),
        ) {
            (Some(lat_idx), Some(lon_idx)) => {
                Encoded::Inside(vec![lat_idx as u128, lon_idx as u128])
            }
            _ => Encoded::Dropped,
        }
    }

    fn decode(&self, coords: &[u128]) -> Option<(f64, f64)> {
        match coords {
            [lat, lon] if *lat >> self.axis_bits == 0 && *lon >> self.axis_bits == 0 => Some((
                self.axis_center(*lat, self.min_lat, self.max_lat),
                self.axis_center(*lon, self.min_lon, self.max_lon),
            )),
            _ => None,
        }
    }

    fn label(&self, coords: &[u128]) -> String {
        format!("morton ({} {})", coords[0], coords[1])
    }
}
