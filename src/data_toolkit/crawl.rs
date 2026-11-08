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

use std::time::Instant;

use rayon::prelude::*;

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
    /// 1 to `d * h1`.
    pub level: usize,
    /// The coordinate that got a bit at this level.
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

/// Run the crawl and return every level, from the first bit to the full points.
pub fn crawl(
    points: &[Vec<u128>],
    d: usize,
    h1: usize,
    metric: Metric,
    delta: u128,
    threshold: usize,
) -> Vec<Level> {
    let bound = metric.bound(delta);
    let mut frontier: Vec<Prefix> = vec![vec![(0, 0); d]];
    let mut levels = Vec::new();
    for level in 1..=d * h1 {
        let start = Instant::now();
        let dim = (level - 1) % d;
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
            break;
        }
    }
    levels
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
}
