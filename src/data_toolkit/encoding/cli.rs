//! Command-line flags that choose an encoding, shared by the data generation binaries.

use clap::{Args, ValueEnum};

use super::Encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EncodingName {
    /// Mosaic's original 0.001 degree grid (the default).
    MosaicGrid,
}

#[derive(Debug, Clone, Args)]
pub struct EncodingArgs {
    /// How each location becomes integer coordinates.
    #[arg(long, value_enum, default_value_t = EncodingName::MosaicGrid)]
    pub encoding: EncodingName,
}

impl EncodingArgs {
    pub fn to_encoding(&self) -> Encoding {
        match self.encoding {
            EncodingName::MosaicGrid => Encoding::MosaicGrid,
        }
    }
}
