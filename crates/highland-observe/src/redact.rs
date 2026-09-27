// Rust guideline compliant 2026-09-27

//! Redaction of sensitive values.
//!
//! Redaction is applied at the observability boundary so that a secret cannot
//! reach a log, a metric label, or an event payload even if a caller is
//! careless (SPEC.md, `S-01`).

use std::collections::BTreeSet;

/// The placeholder written in place of a redacted value.
pub const REDACTED: &str = "[redacted]";

/// Replaces values whose key matches a configured pattern.
///
/// Matching is on substrings, so `check.database.password` is covered by
/// configuring `password`. Comparison is case-insensitive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redactor {
    patterns: BTreeSet<String>,
}

impl Redactor {
    /// Creates a redactor covering the default sensitive-key patterns.
    #[must_use]
    pub fn with_defaults() -> Self {
        let patterns = [
            "password",
            "passwd",
            "secret",
            "token",
            "key",
            "credential",
            "authorization",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        Self { patterns }
    }

    /// Creates a redactor that redacts nothing.
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Adds a pattern, in lower case.
    pub fn add_pattern(&mut self, pattern: impl AsRef<str>) -> &mut Self {
        self.patterns.insert(pattern.as_ref().to_lowercase());
        self
    }

    /// Returns `true` when a value with this key must be redacted.
    #[must_use]
    pub fn is_sensitive(&self, key: &str) -> bool {
        let key = key.to_lowercase();
        self.patterns
            .iter()
            .any(|pattern| key.contains(pattern.as_str()))
    }

    /// Returns the value to log for `key`.
    #[must_use]
    pub fn value_for(&self, key: &str, value: &str) -> String {
        if self.is_sensitive(key) {
            REDACTED.to_owned()
        } else {
            value.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_patterns_cover_common_secret_names() {
        let redactor = Redactor::with_defaults();
        for key in [
            "check.database.password",
            "instance.api.check.token",
            "Authorization",
            "tls.key",
        ] {
            assert!(redactor.is_sensitive(key), "{key} should be redacted");
        }
        assert!(!redactor.is_sensitive("instance.api.check.url"));
    }

    #[test]
    fn a_redacted_value_never_contains_the_secret() {
        let redactor = Redactor::with_defaults();
        assert_eq!(redactor.value_for("password", "hunter2"), REDACTED);
        assert_eq!(
            redactor.value_for("url", "http://example.test"),
            "http://example.test"
        );
    }

    #[test]
    fn extra_patterns_can_be_added() {
        let mut redactor = Redactor::disabled();
        redactor.add_pattern("community");
        assert!(redactor.is_sensitive("snmp.community"));
        assert!(!redactor.is_sensitive("password"));
    }
}
