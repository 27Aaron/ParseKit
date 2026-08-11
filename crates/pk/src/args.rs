//! CLI argument definitions.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "pk",
    version,
    about = "ParseKit CLI — resolve and download social media posts"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Prefer {
    /// Highest quality first (default).
    Best,
    /// Smallest / lowest quality first.
    Smallest,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Resolve share text or URL (print summary / JSON).
    Resolve {
        /// Share text or URL.
        input: String,
        /// Print full JSON instead of a short summary.
        #[arg(long)]
        json: bool,
    },
    /// Resolve and download a video source.
    Download {
        /// Share text or URL.
        input: String,
        /// Output directory (default: PARSE_KIT_OUTPUT_DIR or ./downloads).
        #[arg(short, long, env = "PARSE_KIT_OUTPUT_DIR")]
        output: Option<PathBuf>,
        /// Max bytes (default: PARSE_KIT_MAX_BYTES or 200 MiB).
        #[arg(long, env = "PARSE_KIT_MAX_BYTES")]
        max_bytes: Option<u64>,
        /// Which source to try first: best (default) or smallest.
        #[arg(long, value_enum, default_value_t = Prefer::Best)]
        prefer: Prefer,
        /// Download only this source index from `pk resolve` (0-based).
        #[arg(long)]
        source: Option<usize>,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List registered platforms for this build.
    Platforms,
}
