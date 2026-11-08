//! Standard geohash, as integer cell indices inside a fixed prefix cell.

use super::{Encoded, LocationEncoder};

const BASE32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// Geohash halves the world alternately by longitude and latitude, longitude first, so its cells
/// form a uniform grid. Every ride must lie in the cell named by `prefix`; the next `bits` bits
/// are split by position into a latitude and a longitude index, and a point is
/// `[lat_idx, lon_idx]`. Rides outside the prefix cell are dropped.
///
/// With prefix `9v6` (15 bits) and 21 more bits, the indices have 11 latitude and 10 longitude
/// bits, and the cells are 0.000687 by 0.001373 degrees (about 76 m by 132 m in Austin), the
/// closest geohash area to Mosaic's grid cells. They are not square, and Mosaic uses one `delta`
/// for both coordinates, so the neighbourhood is about 1.7 times wider east-west than
/// north-south.
#[derive(Debug, Clone, PartialEq)]
pub struct Geohash {
    prefix: Vec<bool>,
    bits: usize,
}

impl Geohash {
    pub fn new(prefix: &str, bits: usize) -> Self {
        let prefix = base32_to_bits(prefix);
        let geohash = Self { prefix, bits };
        assert!(
            (1..=31).contains(&geohash.lat_bits()) && (1..=31).contains(&geohash.lon_bits()),
            "geohash bits after the prefix must give 1 to 31 bits per axis, got {}",
            bits
        );
        geohash
    }

    /// Whether the bit at position `i` of the full geohash halves the longitude range.
    fn is_lon_bit(i: usize) -> bool {
        i % 2 == 0
    }

    fn suffix_positions(&self) -> std::ops::Range<usize> {
        self.prefix.len()..self.prefix.len() + self.bits
    }

    fn lon_bits(&self) -> usize {
        self.suffix_positions().filter(|&i| Self::is_lon_bit(i)).count()
    }

    fn lat_bits(&self) -> usize {
        self.bits - self.lon_bits()
    }

    /// The full geohash bits (prefix included) of the cell `[lat_idx, lon_idx]`.
    fn full_bits(&self, lat_idx: u128, lon_idx: u128) -> Vec<bool> {
        let (mut lat_left, mut lon_left) = (self.lat_bits(), self.lon_bits());
        let mut bits = self.prefix.clone();
        for i in self.suffix_positions() {
            if Self::is_lon_bit(i) {
                lon_left -= 1;
                bits.push((lon_idx >> lon_left) & 1 == 1);
            } else {
                lat_left -= 1;
                bits.push((lat_idx >> lat_left) & 1 == 1);
            }
        }
        bits
    }
}

/// The first `n` geohash bits of a point.
pub fn point_to_bits(lat: f64, lon: f64, n: usize) -> Vec<bool> {
    let (mut lat_range, mut lon_range) = ((-90.0, 90.0), (-180.0, 180.0));
    (0..n)
        .map(|i| {
            let (range, v): (&mut (f64, f64), f64) = if Geohash::is_lon_bit(i) {
                (&mut lon_range, lon)
            } else {
                (&mut lat_range, lat)
            };
            let mid = (range.0 + range.1) / 2.0;
            if v >= mid {
                range.0 = mid;
                true
            } else {
                range.1 = mid;
                false
            }
        })
        .collect()
}

/// Bounding box `((min_lat, max_lat), (min_lon, max_lon))` of a geohash bit string.
pub fn bits_to_box(bits: &[bool]) -> ((f64, f64), (f64, f64)) {
    let (mut lat_range, mut lon_range) = ((-90.0, 90.0), (-180.0, 180.0));
    for (i, &bit) in bits.iter().enumerate() {
        let range: &mut (f64, f64) = if Geohash::is_lon_bit(i) {
            &mut lon_range
        } else {
            &mut lat_range
        };
        let mid = (range.0 + range.1) / 2.0;
        if bit {
            range.0 = mid;
        } else {
            range.1 = mid;
        }
    }
    (lat_range, lon_range)
}

pub fn base32_to_bits(s: &str) -> Vec<bool> {
    s.bytes()
        .flat_map(|c| {
            let v = BASE32
                .iter()
                .position(|&b| b == c)
                .unwrap_or_else(|| panic!("invalid geohash character {:?}", c as char));
            (0..5).rev().map(move |i| (v >> i) & 1 == 1)
        })
        .collect()
}

/// Base32 string of the whole 5-bit groups in `bits`; leftover bits are shown in brackets.
pub fn bits_to_base32(bits: &[bool]) -> String {
    let chunks = bits.chunks_exact(5);
    let rest: String = chunks
        .remainder()
        .iter()
        .map(|&b| if b { '1' } else { '0' })
        .collect();
    let mut s: String = chunks
        .map(|c| BASE32[c.iter().fold(0, |v, &b| (v << 1) | b as usize)] as char)
        .collect();
    if !rest.is_empty() {
        s.push_str(&format!("[{}]", rest));
    }
    s
}

impl LocationEncoder for Geohash {
    fn dims(&self) -> usize {
        2
    }

    fn axis_bits(&self) -> usize {
        self.lat_bits().max(self.lon_bits())
    }

    fn encode(&self, lat: f64, lon: f64) -> Encoded {
        if !(lat.is_finite() && lon.is_finite()) {
            return Encoded::Dropped;
        }
        let full = point_to_bits(lat, lon, self.prefix.len() + self.bits);
        if !full.starts_with(&self.prefix) {
            return Encoded::Dropped;
        }
        let (mut lat_idx, mut lon_idx) = (0u128, 0u128);
        for i in self.suffix_positions() {
            if Self::is_lon_bit(i) {
                lon_idx = (lon_idx << 1) | full[i] as u128;
            } else {
                lat_idx = (lat_idx << 1) | full[i] as u128;
            }
        }
        Encoded::Inside(vec![lat_idx, lon_idx])
    }

    fn decode(&self, coords: &[u128]) -> Option<(f64, f64)> {
        match coords {
            [lat, lon] if *lat >> self.lat_bits() == 0 && *lon >> self.lon_bits() == 0 => {
                let (lat, lon) = bits_to_box(&self.full_bits(*lat, *lon));
                Some(((lat.0 + lat.1) / 2.0, (lon.0 + lon.1) / 2.0))
            }
            _ => None,
        }
    }

    fn label(&self, coords: &[u128]) -> String {
        bits_to_base32(&self.full_bits(coords[0], coords[1]))
    }
}
