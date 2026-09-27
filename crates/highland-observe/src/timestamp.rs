// Rust guideline compliant 2026-09-27

//! Timestamps in RFC 3339 form.
//!
//! Every event carries one, and an event stream whose timestamps are all the
//! epoch is not a stream an operator can read. The format is fixed by
//! `SPEC.md` §16: UTC, second precision, `Z` suffix.
//!
//! This is thirty lines rather than a dependency. A time crate would be the
//! obvious choice, and it would also be the largest thing in the workspace's
//! dependency graph, used for one function, in a crate whose whole point is that
//! it depends on nothing but `serde` and `thiserror`.

use std::time::{SystemTime, UNIX_EPOCH};

/// The seconds in a day.
const SECONDS_PER_DAY: u64 = 86_400;

/// Returns the current time in RFC 3339 form, for example
/// `2026-09-27T18:57:34Z`.
///
/// A clock before the epoch, or one that has moved backwards past it, formats as
/// the epoch rather than panicking: an event's timestamp being wrong is worth
/// less than the daemon refusing to start.
#[must_use]
pub fn now() -> String {
    let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs(),
        // A clock before 1970, which is a misconfigured host rather than a
        // condition to refuse to start over.
        Err(_) => 0,
    };
    format(seconds)
}

/// Formats a count of seconds since the epoch in RFC 3339 form.
#[must_use]
pub fn format(seconds: u64) -> String {
    let days = seconds / SECONDS_PER_DAY;
    let time_of_day = seconds % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a count of days since 1970-01-01 into a civil date.
///
/// Howard Hinnant's `civil_from_days`: the era arithmetic is exact for the whole
/// range a `u64` count of days covers, and a lookup table of month lengths would
/// need leap-year rules of its own. The sign gymnastics is the cost of doing that
/// arithmetic in the signed type the algorithm is written in; a `u64` count of
/// days since 1970 is past the year 5,800,000 either way.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "the era arithmetic is written in the signed type and its results are bounded by a calendar; a u64 count of days is past the year 5800000"
)]
fn civil_from_days(days: u64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01, which starts a 400-year era, so the leap day
    // falls at the end of the era rather than in the middle of it.
    let shifted = days as i64 + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // 0..=146_096
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // 0..=399
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // 0..=365
    let shifted_month = (5 * day_of_year + 2) / 153; // 0..=11, March is 0
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32; // 1..=31
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;

    // January and February belong to the following year.
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, format};

    #[test]
    fn the_epoch_formats_as_itself() {
        assert_eq!(format(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_known_instant_formats_correctly() {
        // 2026-09-18T05:47:34Z.
        assert_eq!(format(1_789_710_454), "2026-09-18T05:47:34Z");
        assert_eq!(format(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn a_leap_day_is_the_twenty_ninth_of_february() {
        // 2024-02-29T12:00:00Z. 2024 is a leap year in the Gregorian rules, which
        // is the part a hand-rolled calendar usually gets wrong.
        assert_eq!(format(1_709_208_000), "2024-02-29T12:00:00Z");
        // The day before, and the first of March, to catch an off-by-one that
        // only shows up at a boundary.
        assert_eq!(format(1_709_078_400), "2024-02-28T00:00:00Z");
        assert_eq!(format(1_709_251_200), "2024-03-01T00:00:00Z");
    }

    #[test]
    fn the_four_hundred_year_era_boundary_is_handled() {
        // 2000-02-29T00:00:00Z. The year is divisible by four hundred, so it is a
        // leap year, and the century rule is the part a hand-rolled calendar gets
        // wrong most often. The day after is where an off-by-one shows.
        assert_eq!(format(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format(951_868_800), "2000-03-01T00:00:00Z");
    }

    #[test]
    fn every_day_of_a_leap_year_advances_by_one() {
        // Walking the whole year is the check that a single date cannot give:
        // an off-by-one shows up as a day that does not follow its predecessor.
        let mut previous = format(1_704_067_200); // 2024-01-01T00:00:00Z
        for day in 1..366 {
            let current = format(1_704_067_200 + day * 86_400);
            assert!(
                current > previous,
                "day {day} of 2024 went backwards: {previous} then {current}"
            );
            previous = current;
        }
        assert_eq!(previous, "2024-12-31T00:00:00Z");
    }

    #[test]
    fn the_civil_conversion_agrees_with_the_formatting() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_089), (2025, 1, 1));
    }
}
