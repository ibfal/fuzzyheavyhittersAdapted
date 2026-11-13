//! The unknown-dictionary crawl in plaintext, level by level, for the crawl viewer.
//!
//! The servers start from the empty prefix and add one bit of one coordinate per level, taking
//! the coordinates in turn (coordinate 0, 1, ..., d - 1, then the next bit of coordinate 0). A
//! prefix stands for the box of points that extend it. It is kept, and split at the next level,
//! if at least `threshold` clients are within `delta` of some point of the box; otherwise it is
//! pruned. After `d * h1` levels the kept prefixes are full points: the fuzzy heavy hitters.
//!
//! This is what `mosaic_others ground-truth` computes, with the same distances, and what the
//! servers learn during the protocol: which prefixes are kept at every level.
//!
//! With a [`DerivedCoordinate`] (H3's `i - j`) the crawl skips that coordinate: it stays at the
//! empty prefix, which every client is near, while the others get their bits. After
//! `(d - 1) * h1` levels one more level gives every full point its only possible derived value
//! and checks it with all coordinates, as the servers do.

use std::time::Instant;

use rayon::prelude::*;

use crate::configs::cli_config::DerivedCoordinate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Linf,
    L1,
    L2,
}

impl Metric {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "Linf" => Ok(Metric::Linf),
            "L1" => Ok(Metric::L1),
            "L2" => Ok(Metric::L2),
            other => Err(format!("unsupported distance metric {:?}", other)),
        }
    }

    /// What a distance is compared with: `delta`, or `delta^2` for L2 (which uses squares).
    pub fn bound(&self, delta: u128) -> u128 {
        match self {
            Metric::L2 => delta * delta,
            _ => delta,
        }
    }

    /// Distance between two points (squared for L2), like `util::calculate_distance`.
    pub fn distance(&self, a: &[u128], b: &[u128]) -> u128 {
        let diffs = a.iter().zip(b).map(|(x, y)| if x > y { x - y } else { y - x });
        match self {
            Metric::Linf => diffs.max().unwrap_or(0),
            Metric::L1 => diffs.sum(),
            Metric::L2 => diffs.map(|d| d * d).sum(),
        }
    }
}

/// A prefix: the leading bits of every coordinate, as `(value, number of bits)`.
pub type Prefix = Vec<(u128, usize)>;

/// The inclusive range of values of each coordinate that extend `prefix`.
pub fn prefix_ranges(prefix: &Prefix, h1: usize) -> Vec<(u128, u128)> {
    prefix
        .iter()
        .map(|&(v, len)| {
            let free = h1 - len;
            (v << free, (v << free) | ((1u128 << free) - 1))
        })
        .collect()
}

/// Bits of one coordinate of a prefix, most significant first, e.g. "0110".
pub fn prefix_bits(value: u128, len: usize) -> String {
    (0..len)
        .rev()
        .map(|i| if (value >> i) & 1 == 1 { '1' } else { '0' })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub prefix: Prefix,
    /// Clients within `delta` of the closest point of the prefix's box; for a full point, the
    /// clients within `delta` of it.
    pub count: usize,
    /// `count >= threshold`: the prefix is kept.
    pub heavy: bool,
}

#[derive(Debug, Clone)]
pub struct Level {
    /// 1 to [`total_levels`].
    pub level: usize,
    /// The coordinate that got a bit at this level, or the derived coordinate at its check.
    pub dim: usize,
    pub candidates: Vec<Candidate>,
    pub seconds: f64,
}

/// Clients within `delta` of some point of the box of `prefix`.
fn count_near(points: &[Vec<u128>], ranges: &[(u128, u128)], metric: Metric, bound: u128) -> usize {
    points
        .par_iter()
        .filter(|p| {
            // The closest point of the box to p, coordinate by coordinate.
            let closest: Vec<u128> = ranges
                .iter()
                .zip(p.iter())
                .map(|(&(lo, hi), &v)| v.max(lo).min(hi))
                .collect();
            metric.distance(&closest, p) <= bound
        })
        .count()
}

/// Number of levels of a crawl that is not cut short: a bit of every crawled coordinate per
/// level, plus the check of the derived coordinate if there is one.
pub fn total_levels(d: usize, h1: usize, derived: Option<&DerivedCoordinate>) -> usize {
    DerivedCoordinate::crawl_dims(derived, d).len() * h1 + usize::from(derived.is_some())
}

/// Run the crawl and return every level, from the first bit to the full points.
pub fn crawl(
    points: &[Vec<u128>],
    d: usize,
    h1: usize,
    metric: Metric,
    delta: u128,
    threshold: usize,
) -> Vec<Level> {
    crawl_with_derived(points, d, h1, metric, delta, threshold, None)
}

/// [`crawl`], skipping the `derived` coordinate and checking it at the full points.
pub fn crawl_with_derived(
    points: &[Vec<u128>],
    d: usize,
    h1: usize,
    metric: Metric,
    delta: u128,
    threshold: usize,
    derived: Option<&DerivedCoordinate>,
) -> Vec<Level> {
    let bound = metric.bound(delta);
    let dims = DerivedCoordinate::crawl_dims(derived, d);
    let mut frontier: Vec<Prefix> = vec![vec![(0, 0); d]];
    let mut levels = Vec::new();
    for level in 1..=dims.len() * h1 {
        let start = Instant::now();
        let dim = dims[(level - 1) % dims.len()];
        let children: Vec<Prefix> = frontier
            .iter()
            .flat_map(|prefix| {
                (0..2u128).map(move |bit| {
                    let mut child = prefix.clone();
                    let (v, len) = child[dim];
                    child[dim] = ((v << 1) | bit, len + 1);
                    child
                })
            })
            .collect();
        let candidates: Vec<Candidate> = children
            .into_iter()
            .map(|prefix| {
                let count = count_near(points, &prefix_ranges(&prefix, h1), metric, bound);
                Candidate {
                    prefix,
                    count,
                    heavy: count >= threshold,
                }
            })
            .collect();
        frontier = candidates
            .iter()
            .filter(|c| c.heavy)
            .map(|c| c.prefix.clone())
            .collect();
        levels.push(Level {
            level,
            dim,
            candidates,
            seconds: start.elapsed().as_secs_f64(),
        });
        if frontier.is_empty() {
            return levels;
        }
    }
    if let Some(derived) = derived {
        let start = Instant::now();
        let candidates = frontier
            .into_iter()
            .filter_map(|mut prefix| {
                let point: Vec<u128> = prefix.iter().map(|&(v, _)| v).collect();
                prefix[derived.index] = (derived.value(&point, h1)?, h1);
                let count = count_near(points, &prefix_ranges(&prefix, h1), metric, bound);
                Some(Candidate {
                    prefix,
                    count,
                    heavy: count >= threshold,
                })
            })
            .collect();
        levels.push(Level {
            level: dims.len() * h1 + 1,
            dim: derived.index,
            candidates,
            seconds: start.elapsed().as_secs_f64(),
        });
    }
    levels
}

/// The fuzzy heavy hitters: the kept points of the last level, if the crawl got that far.
pub fn heavy_hitters(levels: &[Level], total_levels: usize) -> Vec<&Candidate> {
    levels
        .last()
        .filter(|l| l.level == total_levels)
        .map(|l| l.candidates.iter().filter(|c| c.heavy).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_the_extensions_of_a_prefix() {
        assert_eq!(prefix_ranges(&vec![(0b10, 2), (0, 0)], 4), vec![(8, 11), (0, 15)]);
        assert_eq!(prefix_bits(0b0110, 4), "0110");
    }

    #[test]
    fn crawl_finds_the_cluster() {
        // Ten clients around (5, 5) and one far away; delta 1, threshold 5, 4 bits.
        let mut points: Vec<Vec<u128>> = (0..10).map(|i| vec![5 + i % 2, 5]).collect();
        points.push(vec![14, 14]);
        let levels = crawl(&points, 2, 4, Metric::Linf, 1, 5);
        assert_eq!(levels.len(), 8);
        let last = levels.last().unwrap();
        let mut heavy: Vec<Vec<u128>> = last
            .candidates
            .iter()
            .filter(|c| c.heavy)
            .map(|c| c.prefix.iter().map(|&(v, _)| v).collect())
            .collect();
        heavy.sort();
        // Within 1 of (5, 5) or (6, 5) for at least 5 clients: x in 4..=7, y in 4..=6.
        let expected: Vec<Vec<u128>> = (4..=7u128)
            .flat_map(|x| (4..=6u128).map(move |y| vec![x, y]))
            .collect();
        assert_eq!(heavy, expected);
    }

    /// H3-style points `[i + o, j + o, i - j + o]` from a few clusters of hexagons.
    fn cube_points(h1: usize) -> (Vec<Vec<u128>>, DerivedCoordinate) {
        let o = 1i64 << (h1 - 1);
        let derived = DerivedCoordinate { index: 2, plus: 0, minus: 1, offset: o as u128 };
        let mut points = Vec::new();
        for &(ci, cj, n) in &[(0i64, 0i64, 40usize), (5, -3, 25), (-6, 4, 12)] {
            for k in 0..n {
                // Spread each cluster over its cell and the six neighbours.
                let (di, dj) = [(0, 0), (1, 0), (0, 1), (1, 1), (-1, 0), (0, -1), (-1, -1)][k % 7];
                let (i, j) = (ci + di, cj + dj);
                points.push(vec![(i + o) as u128, (j + o) as u128, (i - j + o) as u128]);
            }
        }
        (points, derived)
    }

    #[test]
    fn derived_crawl_finds_exactly_the_cells_of_the_full_crawl() {
        let h1 = 6;
        let (points, derived) = cube_points(h1);
        for (metric, delta) in [(Metric::Linf, 2), (Metric::L1, 4), (Metric::L2, 3)] {
            for threshold in [5, 15, 30] {
                let full = crawl(&points, 3, h1, metric, delta, threshold);
                let mut expected: Vec<Vec<u128>> = heavy_hitters(&full, 3 * h1)
                    .iter()
                    .map(|c| c.prefix.iter().map(|&(v, _)| v).collect::<Vec<u128>>())
                    .filter(|p| derived.value(p, h1) == Some(p[2]))
                    .collect();
                let skipped = crawl_with_derived(&points, 3, h1, metric, delta, threshold, Some(&derived));
                assert_eq!(total_levels(3, h1, Some(&derived)), 2 * h1 + 1);
                let mut got: Vec<Vec<u128>> = heavy_hitters(&skipped, 2 * h1 + 1)
                    .iter()
                    .map(|c| c.prefix.iter().map(|&(v, _)| v).collect())
                    .collect();
                expected.sort();
                got.sort();
                assert!(!expected.is_empty() || threshold == 30, "{:?} {}", metric, threshold);
                assert_eq!(got, expected, "{:?}, delta {}, threshold {}", metric, delta, threshold);
            }
        }
    }

    #[test]
    fn derived_value_is_i_minus_j() {
        let derived = DerivedCoordinate { index: 2, plus: 0, minus: 1, offset: 1024 };
        assert_eq!(derived.value(&[1030, 1020, 0], 11), Some(1034));
        assert_eq!(derived.value(&[0, 1500, 0], 11), None);
        assert_eq!(derived.value(&[2047, 0, 0], 11), None);
        assert_eq!(DerivedCoordinate::crawl_dims(Some(&derived), 3), vec![0, 1]);
        assert_eq!(DerivedCoordinate::crawl_dims(None, 3), vec![0, 1, 2]);
        assert!(derived.validate(3).is_ok());
        assert!(derived.validate(2).is_err());
    }
}
