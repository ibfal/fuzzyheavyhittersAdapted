//! Mosaic's original RideAustin grid.

use super::{rect, Encoded, LocationEncoder};
use crate::data_toolkit::{geo_to_grid, grid_to_geo, LAT_BITS, LAT_GRID_SIZE, LON_GRID_SIZE};
use crate::data_toolkit::{AUSTIN_MIN_LAT, AUSTIN_MIN_LON, DECIMAL_SCALE};

/// 0.001 degree cells, 2000 per axis, with the origin one degree south-west of downtown Austin.
/// A point is `[lat_grid, lon_grid]`. Points outside the grid are clamped to its edge, exactly as
/// [`geo_to_grid`] has always done, so the datasets are unchanged.
pub struct MosaicGrid;

impl MosaicGrid {
    /// Whether `geo_to_grid` has to clamp this location.
    fn is_outside(lat: f64, lon: f64) -> bool {
        let lat_cell = ((lat - AUSTIN_MIN_LAT) * DECIMAL_SCALE as f64).round();
        let lon_cell = ((lon - AUSTIN_MIN_LON) * DECIMAL_SCALE as f64).round();
        !(0.0..LAT_GRID_SIZE as f64).contains(&lat_cell)
            || !(0.0..LON_GRID_SIZE as f64).contains(&lon_cell)
    }
}

impl LocationEncoder for MosaicGrid {
    fn dims(&self) -> usize {
        2
    }

    fn axis_bits(&self) -> usize {
        LAT_BITS
    }

    fn encode(&self, lat: f64, lon: f64) -> Encoded {
        let (lat_grid, lon_grid) = geo_to_grid(lat, lon);
        let coords = vec![lat_grid as u128, lon_grid as u128];
        if Self::is_outside(lat, lon) {
            Encoded::Clamped(coords)
        } else {
            Encoded::Inside(coords)
        }
    }

    fn decode(&self, coords: &[u128]) -> Option<(f64, f64)> {
        match coords {
            [lat, lon] if *lat < LAT_GRID_SIZE as u128 && *lon < LON_GRID_SIZE as u128 => {
                Some(grid_to_geo(*lat as u32, *lon as u32))
            }
            _ => None,
        }
    }

    fn label(&self, coords: &[u128]) -> String {
        format!("grid ({}, {})", coords[0], coords[1])
    }

    fn region(&self, ranges: &[(u128, u128)]) -> Vec<Vec<(f64, f64)>> {
        // Cell i holds the points that round to i, so it spans i - 0.5 to i + 0.5. Indices past
        // the last cell are never produced.
        let (lat_hi, lon_hi) = (
            ranges[0].1.min(LAT_GRID_SIZE as u128 - 1),
            ranges[1].1.min(LON_GRID_SIZE as u128 - 1),
        );
        if ranges[0].0 > lat_hi || ranges[1].0 > lon_hi {
            return vec![];
        }
        let edge = |min: f64, i: f64| min + i / DECIMAL_SCALE as f64;
        vec![rect(
            (
                edge(AUSTIN_MIN_LAT, ranges[0].0 as f64 - 0.5),
                edge(AUSTIN_MIN_LAT, lat_hi as f64 + 0.5),
            ),
            (
                edge(AUSTIN_MIN_LON, ranges[1].0 as f64 - 0.5),
                edge(AUSTIN_MIN_LON, lon_hi as f64 + 0.5),
            ),
        )]
    }
}
