// Rust guideline compliant 2026-09-27

//! Configuration error types.
//!
//! Three layers of failure are distinguished: the document could not be
//! parsed, the document does not match the schema, or the document is
//! semantically invalid. Semantic failures name the `V-nn` rule they violate
//! (SPEC.md, §10.4).

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// A position in a configuration document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceLocation {
    /// The configuration key path, for example `instance.api.vrid`.
    pub key: String,
    /// The one-based line, when known.
    pub line: Option<usize>,
    /// The one-based column, when known.
    pub column: Option<usize>,
}

impl SourceLocation {
    /// Creates a location for `key` without positional information.
    #[must_use]
    pub fn key(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            line: None,
            column: None,
        }
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key)?;
        match (self.line, self.column) {
            (Some(line), Some(column)) => write!(f, " at line {line}, column {column}"),
            (Some(line), None) => write!(f, " at line {line}"),
            _ => Ok(()),
        }
    }
}

/// A semantic validation failure.
///
/// The message always begins with the configuration key that carries the
/// problem, for example `instance.api.vrid`, so that a diagnostic points at a
/// single place in the document (SPEC.md, §10.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigViolation {
    /// The specification rule that was violated, for example `V-06`.
    pub rule: &'static str,
    /// What was wrong, prefixed with the configuration key.
    pub message: String,
}

impl fmt::Display for ConfigViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.rule, self.message)
    }
}

/// A rule identifier paired with its message, used while collecting violations.
pub type Violation = (&'static str, String);

/// The result of validating a configuration.
pub type ConfigViolations = Vec<ConfigViolation>;

/// An error raised while loading or validating a configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file could not be read.
    #[error("could not read configuration file {path}: {source}")]
    Read {
        /// The path that was attempted.
        path: PathBuf,
        /// The operating-system error.
        source: std::io::Error,
    },

    /// The file is not safe to read (SPEC.md, `V-26`).
    #[error(
        "configuration file {path} is world-writable; pass --allow-insecure-config to accept it"
    )]
    WorldWritable {
        /// The offending path.
        path: PathBuf,
    },

    /// The file exceeds the size limit (`L-06`).
    #[error("configuration file {path} is {size} bytes, above the {limit} byte limit")]
    TooLarge {
        /// The offending path.
        path: PathBuf,
        /// The observed size.
        size: usize,
        /// The accepted maximum.
        limit: usize,
    },

    /// The document could not be parsed.
    #[error("configuration is not valid TOML: {message}")]
    Syntax {
        /// The parser diagnostic.
        message: String,
    },

    /// The document does not match the schema.
    #[error("configuration does not match the schema: {message}")]
    Schema {
        /// The diagnostic, including the offending key.
        message: String,
    },

    /// The document is semantically invalid.
    #[error("configuration is not valid:\n{}", format_violations(.violations))]
    Invalid {
        /// Every violation found, in source order.
        violations: ConfigViolations,
    },
}

impl ConfigError {
    /// Returns the violations, when this is [`ConfigError::Invalid`].
    #[must_use]
    pub fn violations(&self) -> &[ConfigViolation] {
        match self {
            ConfigError::Invalid { violations } => violations,
            _ => &[],
        }
    }
}

impl From<DurationParseError> for ConfigError {
    fn from(error: DurationParseError) -> Self {
        ConfigError::Schema {
            message: error.to_string(),
        }
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        ConfigError::Syntax {
            message: error.to_string(),
        }
    }
}

/// Renders violations as one indented line each.
fn format_violations(violations: &ConfigViolations) -> String {
    violations
        .iter()
        .map(|v| format!("  {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The error returned when a duration string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum DurationParseError {
    /// The duration string was empty.
    #[error("duration is empty; write a value with a unit, for example 500ms")]
    Empty,

    /// A unit appeared where a number was expected.
    #[error("duration {input} is invalid: expected a number, found {character:?}")]
    UnexpectedCharacter {
        /// The rejected text.
        input: String,
        /// The offending character.
        character: char,
    },

    /// A number was written without a unit.
    #[error("duration {input} is missing a unit; write a value with a unit, for example 500ms")]
    MissingUnit {
        /// The rejected text.
        input: String,
    },

    /// The unit is not recognized.
    #[error("duration {input} is invalid: {unit:?} is not a known unit (ns, us, ms, s, m, h)")]
    UnknownUnit {
        /// The rejected text.
        input: String,
        /// The offending unit.
        unit: String,
    },

    /// A unit was used twice.
    #[error("duration {input} is invalid: unit {unit:?} is repeated")]
    RepeatedUnit {
        /// The rejected text.
        input: String,
        /// The repeated unit.
        unit: &'static str,
    },

    /// A component did not fit in a `u64`, or the sum overflowed.
    #[error("duration {input} is out of range")]
    ValueOverflow {
        /// The rejected text.
        input: String,
    },
}
