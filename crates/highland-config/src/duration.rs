// Rust guideline compliant 2026-09-27

//! Duration parsing and serialization.
//!
//! Durations are written with an explicit unit, for example `500ms` or `1s`.
//! A bare number is rejected: `1000` is more often a unit mistake than a
//! request for a thousand seconds.

use std::fmt;
use std::time::Duration;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::error::DurationParseError;

/// A duration that reads and writes as a human-readable string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DurationSpec(pub Duration);

impl DurationSpec {
    /// Returns the inner duration.
    #[must_use]
    pub fn as_duration(self) -> Duration {
        self.0
    }

    /// Returns `true` when the duration is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }
}

impl From<Duration> for DurationSpec {
    fn from(duration: Duration) -> Self {
        Self(duration)
    }
}

impl From<DurationSpec> for Duration {
    fn from(spec: DurationSpec) -> Self {
        spec.0
    }
}

impl fmt::Display for DurationSpec {
    /// Renders the duration with the largest unit that is exact.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nanos = self.0.as_nanos();
        if nanos % 1_000_000 == 0 {
            write!(f, "{}ms", nanos / 1_000_000)
        } else if nanos % 1_000 == 0 {
            write!(f, "{}us", nanos / 1_000)
        } else if nanos % 1_000_000_000 == 0 {
            write!(f, "{}s", nanos / 1_000_000_000)
        } else {
            write!(f, "{nanos}ns")
        }
    }
}

impl Serialize for DurationSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DurationSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_duration(&text).map(Self).map_err(de::Error::custom)
    }
}

/// Parses a duration written as a sequence of number and unit pairs.
///
/// Supported units are `ns`, `us`, `ms`, `s`, `m`, and `h`. The components are
/// summed, so `1m30s` is ninety seconds.
///
/// # Errors
///
/// Returns [`DurationParseError`] when the text is empty, has no unit, uses an
/// unknown unit, or repeats a unit.
///
/// # Examples
///
/// ```
/// use highland_config::parse_duration;
/// use std::time::Duration;
///
/// assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
/// assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
/// assert!(parse_duration("1000").is_err());
/// ```
pub fn parse_duration(text: &str) -> Result<Duration, DurationParseError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(DurationParseError::Empty);
    }

    let mut rest = trimmed;
    let mut total = Duration::ZERO;
    let mut seen: Vec<&str> = Vec::new();

    while !rest.is_empty() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            let character = rest.chars().next().unwrap_or('?');
            return Err(DurationParseError::UnexpectedCharacter {
                input: trimmed.to_owned(),
                character,
            });
        }
        let (value_text, tail) = rest.split_at(digits);
        let value: u64 = value_text
            .parse()
            .map_err(|_| DurationParseError::ValueOverflow {
                input: trimmed.to_owned(),
            })?;

        let unit_len = tail.len()
            - tail
                .trim_start_matches(|c: char| c.is_ascii_alphabetic())
                .len();
        let (unit, remainder) = tail.split_at(unit_len);
        if unit.is_empty() {
            return Err(DurationParseError::MissingUnit {
                input: trimmed.to_owned(),
            });
        }
        let unit = UNITS
            .iter()
            .find(|candidate| **candidate == unit)
            .copied()
            .ok_or_else(|| DurationParseError::UnknownUnit {
                input: trimmed.to_owned(),
                unit: unit.to_owned(),
            })?;
        if seen.contains(&unit) {
            return Err(DurationParseError::RepeatedUnit {
                input: trimmed.to_owned(),
                unit,
            });
        }
        seen.push(unit);

        let seconds = match unit {
            "ms" => value / 1_000,
            "s" => value,
            "m" => value.saturating_mul(60),
            "h" => value.saturating_mul(3_600),
            _ => 0,
        };
        let scaled = match unit {
            "ns" => Duration::from_nanos(value),
            "us" => Duration::from_micros(value),
            "ms" => Duration::from_millis(value),
            _ => Duration::from_secs(seconds),
        };
        total = total
            .checked_add(scaled)
            .ok_or(DurationParseError::ValueOverflow {
                input: trimmed.to_owned(),
            })?;
        rest = remainder;
    }

    Ok(total)
}

/// The unit spellings accepted by [`parse_duration`], longest first.
const UNITS: &[&str] = &["ns", "us", "ms", "s", "m", "h"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_unit_durations_parse() {
        assert_eq!(parse_duration("10ms").unwrap(), Duration::from_millis(10));
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3_600));
        assert_eq!(parse_duration("5us").unwrap(), Duration::from_micros(5));
        assert_eq!(parse_duration("7ns").unwrap(), Duration::from_nanos(7));
    }

    #[test]
    fn component_durations_are_summed() {
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(
            parse_duration("1h2m3s").unwrap(),
            Duration::from_secs(3_723)
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            parse_duration("  500ms  ").unwrap(),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn a_bare_number_is_rejected() {
        assert!(matches!(
            parse_duration("1000"),
            Err(DurationParseError::MissingUnit { .. })
        ));
    }

    #[test]
    fn empty_and_unknown_inputs_are_rejected() {
        assert_eq!(parse_duration(""), Err(DurationParseError::Empty));
        assert!(
            matches!(parse_duration("5d"), Err(DurationParseError::UnknownUnit { unit, .. }) if unit == "d")
        );
        assert!(matches!(
            parse_duration("ms"),
            Err(DurationParseError::UnexpectedCharacter { character: 'm', .. })
        ));
    }

    #[test]
    fn a_repeated_unit_is_rejected() {
        assert!(
            matches!(parse_duration("1s1s"), Err(DurationParseError::RepeatedUnit { unit, .. }) if unit == "s")
        );
        assert!(parse_duration("1s500ms").is_ok());
    }

    #[test]
    fn durations_render_in_the_documented_spelling() {
        assert_eq!(
            DurationSpec(Duration::from_millis(500)).to_string(),
            "500ms"
        );
        assert_eq!(DurationSpec(Duration::from_secs(2)).to_string(), "2000ms");
        assert_eq!(DurationSpec(Duration::from_micros(1)).to_string(), "1us");
        assert_eq!(DurationSpec(Duration::from_secs(3)).to_string(), "3000ms");
    }

    #[test]
    fn durations_round_trip_through_toml() {
        #[derive(Deserialize, Serialize, PartialEq, Debug)]
        struct Holder {
            interval: DurationSpec,
        }

        let holder: Holder = toml::from_str("interval = \"1s\"").expect("parses");
        assert_eq!(holder.interval, DurationSpec(Duration::from_secs(1)));
        let text = toml::to_string(&holder).expect("serializes");
        assert!(text.contains("1000ms"), "unexpected rendering: {text}");
    }
}
