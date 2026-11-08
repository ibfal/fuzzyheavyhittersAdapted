//! Command-line flags that choose an encoding, shared by the data generation binaries.

use clap::{Args, ValueEnum};

use super::Encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EncodingName {
    /// Mosaic's original 0.001 degree grid (the default).
    MosaicGrid,
    /// A uniform grid over the `--morton-*` box; points outside are dropped.
    Morton,
}

#[derive(Debug, Clone, Args)]
pub struct EncodingArgs {
    /// How each location becomes integer coordinates.
    #[arg(long, value_enum, default_value_t = EncodingName::MosaicGrid)]
    pub encoding: EncodingName,

    /// Morton box, south edge (inclusive). The default box gives exactly Mosaic's grid cells.
    #[arg(long, default_value_t = 29.9005, allow_negative_numbers = true)]
    pub morton_min_lat: f64,
    /// Morton box, north edge (exclusive).
    #[arg(long, default_value_t = 30.9245, allow_negative_numbers = true)]
    pub morton_max_lat: f64,
    /// Morton box, west edge (inclusive).
    #[arg(long, default_value_t = -98.0505, allow_negative_numbers = true)]
    pub morton_min_lon: f64,
    /// Morton box, east edge (exclusive).
    #[arg(long, default_value_t = -97.0265, allow_negative_numbers = true)]
    pub morton_max_lon: f64,
    /// Morton cells per axis, as a power of two (the config's h1).
    #[arg(long, default_value_t = 10)]
    pub morton_axis_bits: usize,
}

impl EncodingArgs {
    pub fn to_encoding(&self) -> Encoding {
        match self.encoding {
            EncodingName::MosaicGrid => Encoding::MosaicGrid,
            EncodingName::Morton => Encoding::Morton {
                min_lat: self.morton_min_lat,
                max_lat: self.morton_max_lat,
                min_lon: self.morton_min_lon,
                max_lon: self.morton_max_lon,
                axis_bits: self.morton_axis_bits,
            },
        }
    }
}
