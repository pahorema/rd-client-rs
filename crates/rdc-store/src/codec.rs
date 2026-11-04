//! Text encodings of identifiers and instants in the existing database layout.
//!
//! Identifiers are stored as upper-case hyphenated UUIDs. Instants are stored
//! as `yyyy-MM-dd HH:mm:ss[.fffffff]+HH:mm`; this crate always writes UTC and
//! trims trailing fractional zeros (dropping the fraction when it is zero),
//! but reads any offset, a `T` separator, a missing offset (UTC) and any
//! number of fractional digits, because rows written by earlier versions use
//! local offsets too.

use jiff::{Timestamp, civil::DateTime, tz::Offset};
use uuid::Uuid;

use crate::error::{Result, StoreError};

/// Number of nanoseconds in the smallest stored fraction (100 ns).
const NANOS_PER_TICK: i32 = 100;

/// The stored text of an identifier: upper-case and hyphenated.
pub(crate) fn uuid_text(id: Uuid) -> String {
    let mut buffer = Uuid::encode_buffer();
    id.hyphenated().encode_upper(&mut buffer).to_owned()
}

/// Decodes an identifier in any letter case.
pub(crate) fn parse_uuid(column: &'static str, text: &str) -> Result<Uuid> {
    Uuid::try_parse(text.trim()).map_err(|_| StoreError::invalid(column, text))
}

/// The stored text of an instant, in UTC with 100 ns precision.
pub(crate) fn timestamp_text(instant: Timestamp) -> String {
    let civil = Offset::UTC.to_datetime(instant);
    let mut text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        civil.year(),
        civil.month(),
        civil.day(),
        civil.hour(),
        civil.minute(),
        civil.second()
    );
    let ticks = civil.subsec_nanosecond() / NANOS_PER_TICK;
    if ticks != 0 {
        let fraction = format!("{ticks:07}");
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    text.push_str("+00:00");
    text
}

/// The current instant at the stored precision, so that a value written
/// now reads back unchanged.
pub(crate) fn now() -> Timestamp {
    let nanos = Timestamp::now().as_nanosecond();
    let tick = i128::from(NANOS_PER_TICK);
    Timestamp::from_nanosecond(nanos - nanos.rem_euclid(tick))
        .expect("truncating the current instant stays in range")
}

/// The stored text of an optional instant.
pub(crate) fn optional_timestamp_text(instant: Option<Timestamp>) -> Option<String> {
    instant.map(timestamp_text)
}

/// Decodes a stored instant; see the module documentation for the accepted
/// forms.
pub(crate) fn parse_timestamp(column: &'static str, text: &str) -> Result<Timestamp> {
    parse_instant(text.trim()).ok_or_else(|| StoreError::invalid(column, text))
}

fn parse_instant(text: &str) -> Option<Timestamp> {
    let mut cursor = Cursor(text.as_bytes());
    let year = cursor.number(4)?;
    cursor.expect(b'-')?;
    let month = cursor.number(2)?;
    cursor.expect(b'-')?;
    let day = cursor.number(2)?;
    if !(cursor.eat(b' ') || cursor.eat(b'T') || cursor.eat(b't')) {
        return None;
    }
    let hour = cursor.number(2)?;
    cursor.expect(b':')?;
    let minute = cursor.number(2)?;
    let second = if cursor.eat(b':') {
        cursor.number(2)?
    } else {
        0
    };
    let nanosecond = if cursor.eat(b'.') {
        cursor.fraction()?
    } else {
        0
    };
    cursor.eat(b' ');
    let offset_seconds = cursor.offset()?;
    if !cursor.0.is_empty() {
        return None;
    }
    let civil = DateTime::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
        nanosecond,
    )
    .ok()?;
    Offset::from_seconds(offset_seconds)
        .ok()?
        .to_timestamp(civil)
        .ok()
}

/// A tiny byte cursor for [`parse_instant`].
struct Cursor<'text>(&'text [u8]);

impl Cursor<'_> {
    fn eat(&mut self, byte: u8) -> bool {
        match self.0.split_first() {
            Some((&first, rest)) if first == byte => {
                self.0 = rest;
                true
            }
            _ => false,
        }
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        self.eat(byte).then_some(())
    }

    /// Exactly `digits` decimal digits.
    fn number(&mut self, digits: usize) -> Option<i32> {
        let (head, rest) = self.0.split_at_checked(digits)?;
        let mut value = 0;
        for &byte in head {
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + i32::from(byte - b'0');
        }
        self.0 = rest;
        Some(value)
    }

    /// One or more fractional digits as nanoseconds; digits beyond the
    /// ninth are ignored.
    fn fraction(&mut self) -> Option<i32> {
        let length = self
            .0
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if length == 0 {
            return None;
        }
        let (digits, rest) = self.0.split_at(length);
        self.0 = rest;
        let mut nanos = 0;
        for position in 0..9 {
            let digit = digits
                .get(position)
                .map_or(0, |byte| i32::from(byte - b'0'));
            nanos = nanos * 10 + digit;
        }
        Some(nanos)
    }

    /// `Z`, `±HH:mm`, `±HHmm`, `±HH` or nothing (UTC), in seconds east of UTC.
    fn offset(&mut self) -> Option<i32> {
        if self.0.is_empty() || self.eat(b'Z') || self.eat(b'z') {
            return Some(0);
        }
        let sign = if self.eat(b'+') {
            1
        } else if self.eat(b'-') {
            -1
        } else {
            return None;
        };
        let hours = self.number(2)?;
        let minutes = if self.0.is_empty() {
            0
        } else {
            self.eat(b':');
            self.number(2)?
        };
        Some(sign * (hours * 3600 + minutes * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(text: &str) -> Timestamp {
        text.parse().expect("valid RFC 3339 instant")
    }

    #[test]
    fn uuids_are_written_upper_case() {
        let id = Uuid::parse_str("3f2504e0-4f89-11d3-9a0c-0305e82c3301").expect("valid uuid");
        assert_eq!(uuid_text(id), "3F2504E0-4F89-11D3-9A0C-0305E82C3301");
    }

    #[test]
    fn uuids_are_read_in_any_case() {
        let upper = parse_uuid("Id", "3F2504E0-4F89-11D3-9A0C-0305E82C3301").expect("upper");
        let lower = parse_uuid("Id", "3f2504e0-4f89-11d3-9a0c-0305e82c3301").expect("lower");
        assert_eq!(upper, lower);
        assert!(parse_uuid("Id", "not-a-uuid").is_err());
    }

    #[test]
    fn timestamps_trim_the_fraction() {
        let cases = [
            ("2026-05-28T16:30:00Z", "2026-05-28 16:30:00+00:00"),
            (
                "2026-05-28T16:30:00.1234567Z",
                "2026-05-28 16:30:00.1234567+00:00",
            ),
            ("2026-05-28T16:30:00.5Z", "2026-05-28 16:30:00.5+00:00"),
            ("2026-05-28T16:30:00.120Z", "2026-05-28 16:30:00.12+00:00"),
            (
                "2026-05-28T16:30:00.000000099Z",
                "2026-05-28 16:30:00+00:00",
            ),
            ("2026-05-28T18:30:00+02:00", "2026-05-28 16:30:00+00:00"),
        ];
        for (input, stored) in cases {
            assert_eq!(timestamp_text(instant(input)), stored, "input {input}");
        }
    }

    #[test]
    fn timestamps_round_trip() {
        let value = instant("2001-02-03T04:05:06.7000008Z");
        assert_eq!(
            parse_timestamp("Added", &timestamp_text(value)).expect("parse"),
            value
        );
    }

    #[test]
    fn timestamps_are_read_tolerantly() {
        let expected = instant("2026-05-28T16:30:00Z");
        for text in [
            "2026-05-28 16:30:00+00:00",
            "2026-05-28 18:30:00+02:00",
            "2026-05-28 12:30:00-04:00",
            "2026-05-28T16:30:00",
            "2026-05-28 16:30:00",
            "2026-05-28 16:30:00Z",
            "2026-05-28 16:30+00:00",
            "2026-05-28 16:30:00.0000000+00:00",
            "2026-05-28 18:30:00+0200",
            " 2026-05-28 16:30:00 +00:00 ",
        ] {
            assert_eq!(
                parse_timestamp("Added", text).expect(text),
                expected,
                "text {text}"
            );
        }
        assert_eq!(
            parse_timestamp("Added", "2026-05-28 16:30:00.1+00:00").expect("one digit"),
            instant("2026-05-28T16:30:00.1Z")
        );
        assert_eq!(
            parse_timestamp("Added", "2026-05-28 16:30:00.123456789123+00:00").expect("long"),
            instant("2026-05-28T16:30:00.123456789Z")
        );
        assert_eq!(
            parse_timestamp("Added", "0001-01-01 00:00:00+00:00").expect("minimum"),
            instant("0001-01-01T00:00:00Z")
        );
    }

    #[test]
    fn malformed_timestamps_are_rejected() {
        for text in [
            "",
            "2026-05-28",
            "2026/05/28 16:30:00",
            "2026-13-28 16:30:00",
            "2026-05-28 16:30:00.",
            "2026-05-28 16:30:00+2",
            "2026-05-28 16:30:00 garbage",
        ] {
            assert!(parse_timestamp("Added", text).is_err(), "text {text:?}");
        }
    }
}
