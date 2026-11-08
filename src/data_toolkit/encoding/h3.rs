//! Uber's H3 hexagonal grid, as cube coordinates around an origin cell.

use std::convert::TryFrom;

use h3o::{CellIndex, CoordIJ, LatLng, LocalIJ, Resolution};

use super::{Encoded, LocationEncoder};

/// H3 cells at `resolution`, given by their local `(i, j)` coordinates relative to the cell that
/// contains `(origin_lat, origin_lon)` (H3's local IJ is measured in the origin's base cell, so
/// the origin's own `(i, j)` is subtracted). A point is `[i + o, j + o, i - j + o]` with
/// `o = 2^(axis_bits - 1)`, so every coordinate is non-negative and the origin is `[o, o, o]`.
///
/// The H3 index digits used by Mastic do not work here: neighbouring hexagons can have very
/// different indices. In local `(i, j)` the axes are 120 degrees apart, so the three
/// coordinates are the hexagon's cube coordinates and, for two cells,
///
/// - `Linf` is the H3 grid distance (number of hexagon steps), `max(|di|, |dj|, |di - dj|)`,
/// - `L1` is twice the grid distance,
/// - `L2` squared is `2 (di^2 + dj^2 - di dj)`: twice the squared distance between the cell
///   centers, in units of the center spacing.
///
/// The crawl also visits triples whose third coordinate is not `i - j`. No location encodes to
/// them; [`LocationEncoder::decode`] returns `None` for them so they can be left out of the
/// results. Cells whose coordinates do not fit in `axis_bits` bits, or that H3 cannot place
/// relative to the origin, are dropped. With `axis_bits = 11` every cell within 1023 steps of the
/// origin fits (about 140 km at resolution 10).
#[derive(Debug, Clone, PartialEq)]
pub struct H3 {
    origin: CellIndex,
    /// The origin's own local IJ coordinates.
    origin_ij: (i64, i64),
    axis_bits: usize,
    /// Center of the origin cell and the steps to the centers of its `+i` and `+j` neighbours,
    /// in degrees, for drawing regions too large to draw cell by cell.
    plane: [(f64, f64); 3],
}

/// Regions with at most this many cells are drawn as their hexagons.
const MAX_DRAWN_CELLS: i64 = 64;

impl H3 {
    pub fn new(resolution: u8, origin_lat: f64, origin_lon: f64, axis_bits: usize) -> Self {
        let resolution = Resolution::try_from(resolution)
            .unwrap_or_else(|e| panic!("invalid H3 resolution {}: {}", resolution, e));
        let origin = LatLng::new(origin_lat, origin_lon)
            .unwrap_or_else(|e| {
                panic!("invalid H3 origin ({}, {}): {}", origin_lat, origin_lon, e)
            })
            .to_cell(resolution);
        assert!(
            (2..=31).contains(&axis_bits),
            "H3 axis_bits must be between 2 and 31, got {}",
            axis_bits
        );
        let ij = origin
            .to_local_ij(origin)
            .unwrap_or_else(|e| panic!("no local IJ for the H3 origin {}: {}", origin, e));
        let mut h3 = Self {
            origin,
            origin_ij: (ij.coord.i as i64, ij.coord.j as i64),
            axis_bits,
            plane: [(0.0, 0.0); 3],
        };
        let center = |i, j| {
            let c = LatLng::from(h3.cell_at(i, j).expect("the origin's neighbours are cells"));
            (c.lat(), c.lng())
        };
        let (c0, ci, cj) = (center(0, 0), center(1, 0), center(0, 1));
        h3.plane = [c0, (ci.0 - c0.0, ci.1 - c0.1), (cj.0 - c0.0, cj.1 - c0.1)];
        h3
    }

    /// `(lat, lon)` of the point `(i, j)` of the plane, by the steps between the origin's
    /// neighbours. Close to the cell centers near the origin; only used for large regions.
    fn plane_point(&self, i: f64, j: f64) -> (f64, f64) {
        let [c0, di, dj] = self.plane;
        (c0.0 + i * di.0 + j * dj.0, c0.1 + i * di.1 + j * dj.1)
    }

    /// The cell at `(i, j)` relative to the origin.
    fn cell_at(&self, i: i64, j: i64) -> Option<CellIndex> {
        let (i, j) = (i + self.origin_ij.0, j + self.origin_ij.1);
        let coord = CoordIJ::new(i32::try_from(i).ok()?, i32::try_from(j).ok()?);
        CellIndex::try_from(LocalIJ::new(self.origin, coord)).ok()
    }

    fn offset(&self) -> i64 {
        1 << (self.axis_bits - 1)
    }

    /// The cell `(lat, lon)` falls in, if it is a valid location.
    pub fn cell(&self, lat: f64, lon: f64) -> Option<CellIndex> {
        Some(LatLng::new(lat, lon).ok()?.to_cell(self.origin.resolution()))
    }

    /// `(i, j)` of a cell relative to the origin, which is `(0, 0)`.
    pub fn local_ij(&self, cell: CellIndex) -> Option<(i64, i64)> {
        let ij = cell.to_local_ij(self.origin).ok()?;
        Some((
            ij.coord.i as i64 - self.origin_ij.0,
            ij.coord.j as i64 - self.origin_ij.1,
        ))
    }

    /// The cell named by `coords`, if they are a consistent `[i, j, i - j]` triple.
    fn cell_of(&self, coords: &[u128]) -> Option<CellIndex> {
        let max = 1u128 << self.axis_bits;
        let [a, b, c] = match coords {
            [a, b, c] if *a < max && *b < max && *c < max => [*a as i64, *b as i64, *c as i64],
            _ => return None,
        };
        let o = self.offset();
        let (i, j) = (a - o, b - o);
        if c - o != i - j {
            return None;
        }
        self.cell_at(i, j)
    }
}

/// Clip a convex polygon to the half-plane `f(p) >= 0`, where `f` is affine.
fn clip(polygon: &[(f64, f64)], f: impl Fn((f64, f64)) -> f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for (k, &p) in polygon.iter().enumerate() {
        let q = polygon[(k + 1) % polygon.len()];
        let (fp, fq) = (f(p), f(q));
        if fp >= 0.0 {
            out.push(p);
        }
        if (fp >= 0.0) != (fq >= 0.0) {
            let t = fp / (fp - fq);
            out.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
        }
    }
    out
}

impl LocationEncoder for H3 {
    fn dims(&self) -> usize {
        3
    }

    fn axis_bits(&self) -> usize {
        self.axis_bits
    }

    fn encode(&self, lat: f64, lon: f64) -> Encoded {
        let ij = self.cell(lat, lon).and_then(|cell| self.local_ij(cell));
        let (i, j) = match ij {
            Some(ij) => ij,
            None => return Encoded::Dropped,
        };
        let o = self.offset();
        let coords = [i + o, j + o, i - j + o];
        if coords.iter().all(|&v| v >= 0 && v < 2 * o) {
            Encoded::Inside(coords.iter().map(|&v| v as u128).collect())
        } else {
            Encoded::Dropped
        }
    }

    fn decode(&self, coords: &[u128]) -> Option<(f64, f64)> {
        let center = LatLng::from(self.cell_of(coords)?);
        Some((center.lat(), center.lng()))
    }

    fn label(&self, coords: &[u128]) -> String {
        match self.cell_of(coords) {
            Some(cell) => cell.to_string(),
            None => format!("not a cell ({} {} {})", coords[0], coords[1], coords[2]),
        }
    }

    fn region(&self, ranges: &[(u128, u128)]) -> Vec<Vec<(f64, f64)>> {
        let o = self.offset();
        let shift = |(lo, hi): (u128, u128)| (lo as i64 - o, hi as i64 - o);
        let ((i0, i1), (j0, j1), (k0, k1)) = (shift(ranges[0]), shift(ranges[1]), shift(ranges[2]));

        // The cells are the (i, j) in the box with i - j in [k0, k1].
        let j_range = |i: i64| (j0.max(i - k1), j1.min(i - k0));
        let mut cells = 0;
        for i in i0..=i1 {
            let (lo, hi) = j_range(i);
            cells += (hi - lo + 1).max(0);
            if cells > MAX_DRAWN_CELLS {
                break;
            }
        }
        if cells == 0 {
            return vec![];
        }
        if cells <= MAX_DRAWN_CELLS {
            let mut hexagons = Vec::new();
            for i in i0..=i1 {
                let (lo, hi) = j_range(i);
                for j in lo..=hi {
                    if let Some(cell) = self.cell_at(i, j) {
                        hexagons.push(cell.boundary().iter().map(|v| (v.lat(), v.lng())).collect());
                    }
                }
            }
            return hexagons;
        }

        // Too many cells to draw: the box in the (i, j) plane, half a cell wider on every side,
        // cut by the two limits on i - j.
        let (a0, a1, b0, b1) = (i0 as f64 - 0.5, i1 as f64 + 0.5, j0 as f64 - 0.5, j1 as f64 + 0.5);
        let mut polygon = vec![(a0, b0), (a1, b0), (a1, b1), (a0, b1)];
        polygon = clip(&polygon, |(i, j)| (i - j) - (k0 as f64 - 0.5));
        polygon = clip(&polygon, |(i, j)| (k1 as f64 + 0.5) - (i - j));
        vec![polygon.into_iter().map(|(i, j)| self.plane_point(i, j)).collect()]
    }
}
