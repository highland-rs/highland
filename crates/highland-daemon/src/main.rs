// Rust guideline compliant 2026-09-27

//! The `highland-daemon` entry point.
//!
//! Argument parsing lives in `highland-cli`; this binary is what
//! `highland run` executes.

use std::path::PathBuf;
use std::process::ExitCode;

use highland_daemon::Options;

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let config_path = arguments
        .windows(2)
        .find(|pair| pair[0] == "--config")
        .map_or_else(
            || PathBuf::from("/etc/highland/config.toml"),
            |pair| PathBuf::from(&pair[1]),
        );
    let allow_insecure_config = arguments
        .iter()
        .any(|argument| argument == "--allow-insecure-config");

    let mut options = Options::with_config(config_path);
    options.allow_insecure_config = allow_insecure_config;

    match highland_daemon::run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("highland-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}
