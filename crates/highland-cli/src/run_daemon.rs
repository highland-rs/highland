// Rust guideline compliant 2026-09-27

//! Delegating `highland run` to the daemon binary.

use std::path::Path;
use std::process::Command;

use anyhow::Context as _;

/// Runs the daemon in the foreground by **replacing** this process with it.
///
/// The CLI does not reimplement daemon startup: it locates the daemon binary next
/// to itself and hands over (SPEC.md, §9.9).
///
/// Replacing rather than spawning is deliberate. A child daemon would make this
/// process a second supervisor in front of the real one, and a signal sent to
/// `highland run` would land on the wrong process: `SIGHUP` would never reach
/// the daemon that owns the sockets, and a `SIGTERM` would stop the CLI while
/// the daemon kept forwarding addresses. After the handover there is exactly one
/// process, and it is the daemon.
pub(crate) fn exec_daemon(config: &Path, allow_insecure_config: bool) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;

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

    // Only reached when the handover fails, which is the one case an error is
    // for. On success this never returns.
    let error = command.exec();
    Err(anyhow::Error::new(error).context(format!("running {}", daemon.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_daemon_binary_is_reported_rather_than_panicked() {
        // The test binary is not called `highland`, so no daemon sits next to it.
        let error = match exec_daemon(Path::new("/etc/highland/config.toml"), false) {
            Ok(()) => None,
            Err(error) => Some(error.to_string()),
        };
        let message = error.unwrap_or_default();
        assert!(
            message.contains("highland-daemon"),
            "unexpected outcome: {message}"
        );
    }
}
