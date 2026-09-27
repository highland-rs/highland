// Rust guideline compliant 2026-09-27

//! Logging initialization.

use tracing_subscriber::EnvFilter;

/// Installs the global tracing subscriber.
///
/// # Errors
///
/// Returns a message when a subscriber is already installed, which happens
/// when the daemon is started twice in one process, as tests sometimes do.
pub(super) fn init(level: &str, format: &str, json: bool) -> Result<(), String> {
    if !matches!(format, "text" | "json") {
        return Err(format!("log format {format:?} is not one of text, json"));
    }
    let filter = EnvFilter::try_new(level)
        .map_err(|error| format!("log level {level:?} is invalid: {error}"))?;

    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let installed = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    installed.map_err(|error| format!("a tracing subscriber is already installed: {error}"))
}

/// Returns the filter expression used for a level, including the daemon's own
/// target at the requested level.
#[must_use]
pub(super) fn filter_for(level: &str) -> String {
    format!("{level},highland=info")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_format_is_refused_before_a_subscriber_is_installed() {
        assert!(init("info", "yaml", false).is_err());
    }

    #[test]
    fn a_filter_always_includes_the_daemon_target() {
        assert_eq!(filter_for("debug"), "debug,highland=info");
    }
}
