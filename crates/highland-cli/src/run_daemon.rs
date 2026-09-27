// Rust guideline compliant 2026-09-27

//! Delegating `highland run` to the daemon binary.

use std::path::Path;
use std::process::Command;

use anyhow::Context as _;

/// Runs the daemon in the foreground by executing the daemon binary.
///
/// The CLI does not reimplement daemon startup: it locates the daemon binary
/// next to itself, forwards the arguments, and propagates the exit code
/// (SPEC.md, §9.9).
///
/// # Errors
///
/// Returns a message when the daemon binary is missing next to this executable,
/// or when it could not be started.
pub(crate) fn exec_daemon(
    config: &Path,
    allow_insecure_config: bool,
) -> anyhow::Result<std::process::ExitCode> {
    let current = std::env::current_exe().context("locating the highland executable")?;
    let daemon = current.with_file_name(format!("highland-daemon{}", std::env::consts::EXE_SUFFIX));

    if !daemon.exists() {
        anyhow::bail!(
            "the daemon binary {} was not found next to {}; install the highland-daemon binary",
            daemon.display(),
            current.display()
        );
    }

    let mut command = Command::new(&daemon);
    command.arg("run").arg("--config").arg(config);
    if allow_insecure_config {
        command.arg("--allow-insecure-config");
    }

    let status = command
        .status()
        .with_context(|| format!("starting {}", daemon.display()))?;
    Ok(exit_code(status))
}

fn exit_code(status: std::process::ExitStatus) -> std::process::ExitCode {
    if status.success() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_daemon_binary_is_reported_clearly() {
        // The test binary is not named highland, so no daemon sits next to it.
        let error = match exec_daemon(Path::new("/etc/highland/config.toml"), false) {
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        };
        assert!(matches!(error, Some(message) if message.contains("highland-daemon")));
    }
}
