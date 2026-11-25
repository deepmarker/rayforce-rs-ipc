//! Checked conversions around Rayforce's 2000-01-01 epoch.
//!
//! A Rayforce timestamp is nanoseconds since 2000-01-01T00:00:00Z, held in an `i64`, and
//! `INT64_MIN` is the engine's null. Unix time counts from 1970, so the difference is a constant
//! — and adding it to a value near either end of the range overflows. These helpers check that
//! instead of wrapping, which is the bug the OCaml client's temporal tests were written for.
//!
//! No floating point is involved anywhere: a nanosecond count above 2^53 cannot survive a trip
//! through an `f64`, and market timestamps are past it.

use std::fmt;

/// 2000-01-01T00:00:00Z, in nanoseconds.
pub const EPOCH_OFFSET_NANOS: i64 = 946_684_800_000_000_000;

/// 2000-01-01T00:00:00Z, in seconds.
pub const EPOCH_OFFSET_SECONDS: i64 = 946_684_800;

/// Nanoseconds in a second.
const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// A timestamp or timespan that cannot be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalError {
    /// The engine's null, which has no instant.
    Null,
    /// The instant is outside the range an `i64` of nanoseconds can hold on the other clock.
    OutOfRange,
}

impl fmt::Display for TemporalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null timestamp"),
            Self::OutOfRange => f.write_str("timestamp outside the range of an i64 of nanoseconds"),
        }
    }
}

impl std::error::Error for TemporalError {}

/// A Rayforce timestamp, in nanoseconds since 2000-01-01, to Unix nanoseconds.
///
/// # Errors
///
/// [`TemporalError::Null`] for `i64::MIN`, [`TemporalError::OutOfRange`] when the sum does not
/// fit in an `i64`.
pub fn to_unix_nanos(rayforce_nanos: i64) -> Result<i64, TemporalError> {
    if rayforce_nanos == i64::MIN {
        return Err(TemporalError::Null);
    }
    rayforce_nanos
        .checked_add(EPOCH_OFFSET_NANOS)
        .ok_or(TemporalError::OutOfRange)
}

/// Unix nanoseconds to a Rayforce timestamp, in nanoseconds since 2000-01-01.
///
/// # Errors
///
/// [`TemporalError::OutOfRange`] when the difference does not fit in an `i64`.
pub fn from_unix_nanos(unix_nanos: i64) -> Result<i64, TemporalError> {
    unix_nanos
        .checked_sub(EPOCH_OFFSET_NANOS)
        .ok_or(TemporalError::OutOfRange)
}

/// A Rayforce timestamp to whole Unix seconds, truncating toward the epoch the way integer
/// division does.
///
/// # Errors
///
/// As [`to_unix_nanos`].
pub fn to_unix_seconds(rayforce_nanos: i64) -> Result<i64, TemporalError> {
    to_unix_nanos(rayforce_nanos).map(|nanos| nanos.div_euclid(NANOS_PER_SECOND))
}

/// Whole Unix seconds to a Rayforce timestamp.
///
/// # Errors
///
/// [`TemporalError::OutOfRange`] when the product does not fit in an `i64`.
pub fn from_unix_seconds(unix_seconds: i64) -> Result<i64, TemporalError> {
    unix_seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|nanos| nanos.checked_sub(EPOCH_OFFSET_NANOS))
        .ok_or(TemporalError::OutOfRange)
}

/// A timespan is already nanoseconds with no epoch, so converting one to a `std` [`Duration`]
/// only has to rule out the null and negative values.
///
/// # Errors
///
/// [`TemporalError::Null`] for `i64::MIN` and for negative spans.
///
/// [`Duration`]: std::time::Duration
pub fn timespan_to_duration(nanos: i64) -> Result<std::time::Duration, TemporalError> {
    if nanos == i64::MIN {
        return Err(TemporalError::Null);
    }
    u64::try_from(nanos)
        .map(std::time::Duration::from_nanos)
        .map_err(|_| TemporalError::OutOfRange)
}

/// A `std` [`Duration`] to a timespan, in nanoseconds.
///
/// # Errors
///
/// [`TemporalError::OutOfRange`] when the span does not fit in an `i64`.
///
/// [`Duration`]: std::time::Duration
pub fn duration_to_timespan(duration: std::time::Duration) -> Result<i64, TemporalError> {
    i64::try_from(duration.as_nanos()).map_err(|_| TemporalError::OutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_maps_to_zero() {
        assert_eq!(from_unix_nanos(EPOCH_OFFSET_NANOS), Ok(0));
        assert_eq!(to_unix_nanos(0), Ok(EPOCH_OFFSET_NANOS));
        assert_eq!(from_unix_seconds(EPOCH_OFFSET_SECONDS), Ok(0));
        assert_eq!(to_unix_seconds(0), Ok(EPOCH_OFFSET_SECONDS));
    }

    #[test]
    fn contemporary_timestamps_round_trip() {
        // 2026-09-18T12:00:00.5Z
        let unix_nanos = 1_789_732_800_500_000_000;
        let rayforce = from_unix_nanos(unix_nanos).unwrap();
        assert_eq!(to_unix_nanos(rayforce), Ok(unix_nanos));
        assert_eq!(to_unix_seconds(rayforce), Ok(1_789_732_800));
    }

    #[test]
    fn null_has_no_instant() {
        assert_eq!(to_unix_nanos(i64::MIN), Err(TemporalError::Null));
        assert_eq!(timespan_to_duration(i64::MIN), Err(TemporalError::Null));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        assert_eq!(to_unix_nanos(i64::MAX), Err(TemporalError::OutOfRange));
        assert_eq!(from_unix_nanos(i64::MIN), Err(TemporalError::OutOfRange));
        assert_eq!(from_unix_seconds(i64::MAX), Err(TemporalError::OutOfRange));
    }

    #[test]
    fn nanosecond_precision_survives_above_two_to_the_fifty_third() {
        let nanos = (1_i64 << 53) + 1;
        let rayforce = from_unix_nanos(nanos).unwrap();
        assert_eq!(to_unix_nanos(rayforce), Ok(nanos));
        assert_eq!(
            duration_to_timespan(std::time::Duration::from_nanos(nanos as u64)),
            Ok(nanos)
        );
    }
}
