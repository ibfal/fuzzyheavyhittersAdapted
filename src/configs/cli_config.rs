//! CLI Configuration for Fuzzy Heavy Hitters Protocol
//!
//! This module defines configuration structures for the CLI application

use crate::fuzzy_match::{
    share_phase_types::{DictionaryType, DistanceMetric, ShareConfig, ShareMethod},
};
use serde::{Deserialize, Serialize};

/// CLI configuration that combines all protocol parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// Path to the synthetic data file (clusters.json)
    pub data_file: String,
    /// Path to server query points file  
    pub query_file: String,
    /// Protocol parameters
    pub protocol: ProtocolParameters,
    /// Logging and output configuration
    pub output: OutputConfig,
}

/// Protocol-specific parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolParameters {
    /// L-infinity distance for fuzzy matching
    pub delta: u128,
    /// Threshold for heavy hitters detection
    pub match_threshold: u128,
    /// Whether to enable sketching verification
    pub enable_sketch: bool,
    /// Input bit length (for coordinates)
    pub h1: usize,
    /// Output bit length (for ring operations)
    pub h2: usize,
    /// Check phase output bit length (for aggregation)
    pub h3: usize,
    /// Number of dimensions
    pub d: usize,
    /// Share phase method ("OKVS" or "IntervalFSS")
    pub share_method: String,
    /// Dictionary type ("Known" or "Unknown")
    pub dictionary_type: String,
    pub check_property: String, // ("Equality", "MuBounded")
    /// Distance metric ("Linf", "L1", "L2", "L3")
    pub distance_metric: String,
    /// Number of clients participating in the protocol
    pub num_clients: usize,
    /// Prime modulo for arithmetic sketching
    pub sketch_modulus: u128,
    /// A coordinate that is a fixed function of the others (H3's `i - j`). The unknown-dictionary
    /// crawl skips its bits and checks it once per full point instead. Absent for the other
    /// encodings, which crawl every coordinate as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_coordinate: Option<DerivedCoordinate>,
}

/// Coordinate `index` of every client point is `point[plus] - point[minus] + offset`.
///
/// For H3 points `[i + o, j + o, i - j + o]` this is `{index: 2, plus: 0, minus: 1, offset: o}`.
/// The crawl then only branches on the other coordinates. Leaving the derived coordinate at the
/// empty prefix counts every client as inside on it (distance 0), so while crawling the count of
/// a prefix can only be too high: no heavy hitter is pruned. At a full point the derived
/// coordinate has a single possible value, and the final check uses all coordinates exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedCoordinate {
    pub index: usize,
    pub plus: usize,
    pub minus: usize,
    pub offset: u128,
}

impl DerivedCoordinate {
    /// Check the coordinates against the dimension `d`.
    pub fn validate(&self, d: usize) -> Result<(), String> {
        if self.index >= d || self.plus >= d || self.minus >= d {
            return Err(format!("derived_coordinate {:?} does not fit d = {}", self, d));
        }
        if self.index == self.plus || self.index == self.minus || self.plus == self.minus {
            return Err(format!(
                "derived_coordinate {:?} must name three different coordinates",
                self
            ));
        }
        Ok(())
    }

    /// The coordinates the crawl branches on, in crawl order.
    pub fn crawl_dims(derived: Option<&Self>, d: usize) -> Vec<usize> {
        (0..d).filter(|&k| derived.map_or(true, |c| c.index != k)).collect()
    }

    /// The derived coordinate of `point` (its own entry is ignored), or `None` if it does not fit
    /// in `h1` bits, in which case no client point has these other coordinates.
    pub fn value(&self, point: &[u128], h1: usize) -> Option<u128> {
        let v = (point[self.plus] + self.offset).checked_sub(point[self.minus])?;
        (v < 1u128 << h1).then_some(v)
    }
}

/// Output and logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Whether to enable verbose output
    pub verbose: bool,
    /// Whether to show intermediate results (for debugging)
    pub show_intermediate: bool,
    /// Output file for results (optional)
    pub output_file: Option<String>,
}

impl CliConfig {
    /// Convert CLI config to protocol config for a specific server
    /// Convert CLI config to share config for client
    pub fn to_share_config(&self) -> Result<ShareConfig, String> {
        // Convert share method and data
        let share_method = match self.protocol.share_method.as_str() {
            "OKVS" => ShareMethod::OKVS,
            "FSS" => ShareMethod::FSS,
            other => return Err(format!("Unsupported share method: {}", other)),
        };

        // Convert dictionary type
        let dictionary_type = match self.protocol.dictionary_type.as_str() {
            "Known" => DictionaryType::Known,
            "Unknown" => DictionaryType::Unknown,
            other => return Err(format!("Unsupported dictionary type: {}", other)),
        };

        // Convert distance metric based on the explicit distance_metric field
        let distance_metric = match self.protocol.distance_metric.as_str() {
            "Linf" => DistanceMetric::LInfinity,
            "L1" => DistanceMetric::Lp { p: 1 },
            "L2" => DistanceMetric::Lp { p: 2 },
            "L3" => DistanceMetric::Lp { p: 3 },
            other => return Err(format!("Unsupported distance metric: {}", other)),
        };

        Ok(ShareConfig {
            method: share_method,
            dictionary_type,
            metric: distance_metric,
            h1: self.protocol.h1,
            h2: self.protocol.h2,
            d: self.protocol.d,
            sketch_modulus: self.protocol.sketch_modulus,
            delta: self.protocol.delta,
        })
    }

    /// Load configuration from a JSON file
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {}: {}", path, e))?;

        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config file {}: {}", path, e))
    }

    /// Save configuration to a JSON file
    pub fn to_file(&self, path: &str) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write config file {}: {}", path, e))
    }
}

/// Generate a sample configuration file
pub fn generate_config(output_path: &str) -> Result<(), String> {
    let sample_config = CliConfig {
        data_file: "data/synthetic/client_points.json".to_string(),
        query_file: "data/synthetic/server_points.json".to_string(),
        protocol: ProtocolParameters {
            delta: 5,
            match_threshold: 3,
            enable_sketch: false,
            h1: 10,
            h2: 16,
            h3: 20, // output_bit_length + 4 for aggregation
            d: 2,
            share_method: "OKVS".to_string(), // Can also be "IntervalFSS"
            dictionary_type: "Known".to_string(), // Can also be "Unknown"
            check_property: "Equality".to_string(), // Can also be "MuBounded"
            distance_metric: "Linf".to_string(), // Can also be "L1", "L2", "L3"
            num_clients: 100,                 // Number of clients participating in the protocol
            sketch_modulus: 1362378130168812918549609490751, // A prime number of 100 bits
            derived_coordinate: None,
        },
        output: OutputConfig {
            verbose: true,
            show_intermediate: false,
            output_file: Some("results.json".to_string()),
        },
    };

    sample_config.to_file(output_path)?;
    println!("Sample configuration written to {}", output_path);
    Ok(())
}
