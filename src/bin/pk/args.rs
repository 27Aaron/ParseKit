//! Command-line interface definitions.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "pk",
    version,
    about = "ParseKit CLI — resolve and download social media posts"
)]
pub struct Cli {
    /// Write tracing logs to stderr; respect RUST_LOG when set.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Prefer {
    /// Try the highest-quality source first.
    Best,
    /// Try the smallest source first.
    Smallest,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Resolve a share URL or pasted share text.
    Resolve {
        /// Share URL or pasted share text.
        input: String,
        /// Emit the complete result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve and download a playable video or image set.
    Download {
        /// Share URL or pasted share text.
        input: String,
        /// Output directory; defaults to PARSE_KIT_OUTPUT_DIR or ./downloads.
        #[arg(short, long, env = "PARSE_KIT_OUTPUT_DIR")]
        output: Option<PathBuf>,
        /// Order for automatic source selection.
        #[arg(long, value_enum, default_value_t = Prefer::Best)]
        prefer: Prefer,
        /// Download one source by its zero-based index from pk resolve.
        #[arg(long)]
        source: Option<usize>,
        /// Download only the first image in an image set.
        #[arg(long)]
        first_only: bool,
        /// Re-download even when a complete local file already exists.
        #[arg(long)]
        force: bool,
        /// Emit the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List the platform resolvers included in this build.
    Platforms {
        /// Include credential and capability status.
        #[arg(long)]
        check: bool,
    },
    /// Check local credentials and registered platforms.
    Doctor,
    /// Bilibili account helpers (cookie / QR login).
    Bilibili {
        #[command(subcommand)]
        command: BilibiliCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum BilibiliCmd {
    /// Scan a QR code with the Bilibili app; save cookie to `.env.local`.
    Login,
    /// Remove `BILIBILI_COOKIE` from `.env.local` and the current process.
    Logout,
    /// Show whether a Bilibili cookie is loaded (local shape only).
    Status,
}
