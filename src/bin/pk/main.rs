//! CLI entrypoint.

mod args;
mod commands;
mod config;
mod output;
mod sources;

use std::process::ExitCode;

use clap::Parser;

use self::args::{Cli, Commands};

fn main() -> ExitCode {
    config::load_dotenv();
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("error: failed to start runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async move {
        match cli.command {
            Commands::Resolve { input, json } => commands::resolve(&input, json).await,
            Commands::Download {
                input,
                output,
                max_bytes,
                prefer,
                source,
                json,
            } => commands::download(&input, output, max_bytes, prefer, source, json).await,
            Commands::Platforms => commands::platforms(),
        }
    });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
