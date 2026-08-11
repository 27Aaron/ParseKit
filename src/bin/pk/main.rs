//! CLI entry point.

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
    init_tracing(cli.verbose);

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
                prefer,
                source,
                first_only,
                json,
            } => commands::download(&input, output, prefer, source, first_only, json).await,
            Commands::Platforms { check } => commands::platforms(check),
            Commands::Doctor => commands::doctor(),
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

fn init_tracing(verbose: bool) {
    if !verbose && std::env::var_os("RUST_LOG").is_none() {
        return;
    }
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| {
        if verbose {
            "info,parse_kit=debug,pk=debug".into()
        } else {
            "info".into()
        }
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
