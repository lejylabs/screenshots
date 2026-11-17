//! The application's legacy RFC3339 boundary. Calendar/arithmetic are provided by Jiff;
//! the spelling is checked here because history preserves leap seconds and every fraction digit.
use anyhow::{Context, Result, ensure};
use jiff::{SignedDuration, Timestamp, ToSpan, civil::Date};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub struct Rfc3339 {
    pub local_date: Date,
    pub offset_seconds: i32,
    pub leap_second: bool,
    second: i64,
    nanosecond: i32,
}

pub fn parse_rfc3339(value: &str) -> Result<Rfc3339> {
    let bytes = value.as_bytes();
    ensure!(bytes.len() >= 20, "Invalid RFC3339 timestamp");
    let digit = |i: usize| -> Result<i32> {
        let b = bytes[i];
        ensure!(b.is_ascii_digit(), "Invalid RFC3339 timestamp");
        Ok(i32::from(b - b'0'))
    };
    let pair = |i| -> Result<i32> { Ok(digit(i)? * 10 + digit(i + 1)?) };
    ensure!(
        bytes[4] == b'-'
            && bytes[7] == b'-'
            && matches!(bytes[10], b'T' | b't' | b' ')
            && bytes[13] == b':'
            && bytes[16] == b':',
        "Invalid RFC3339 timestamp"
    );
    let year = digit(0)? * 1000 + digit(1)? * 100 + pair(2)?;
    let local_date = Date::new(year as i16, pair(5)? as i8, pair(8)? as i8)?;
    let (hour, minute, second) = (pair(11)?, pair(14)?, pair(17)?);
    ensure!(
        hour <= 23 && minute <= 59 && second <= 60,
        "Invalid RFC3339 clock time"
    );
    let mut end = 19;
    let mut nanosecond = 0;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            if end - start < 9 {
                nanosecond = nanosecond * 10 + i32::from(bytes[end] - b'0');
            }
            end += 1;
        }
        ensure!(end > start, "Invalid RFC3339 fraction");
        for _ in (end - start)..9 {
            nanosecond *= 10;
        }
    }
    let suffix = &value[end..];
    let offset_seconds = if suffix == "Z" || suffix == "z" {
        0
    } else {
        // Preserve the previous parser's Unicode minus admission as well.
        let (negative, sign_len) = if suffix.starts_with('+') {
            (false, 1)
        } else if suffix.starts_with('-') {
            (true, 1)
        } else if suffix.starts_with('−') {
            (true, 3)
        } else {
            anyhow::bail!("Invalid RFC3339 offset")
        };
        ensure!(
            suffix.len() == sign_len + 5 && bytes[end + sign_len + 2] == b':',
            "Invalid RFC3339 offset"
        );
        let (h, m) = (pair(end + sign_len)?, pair(end + sign_len + 3)?);
        ensure!(h <= 23 && m <= 59, "Invalid RFC3339 offset");
        (h * 3600 + m * 60) * if negative { -1 } else { 1 }
    };
    // A civil day count covers the entire legacy 0000–9999 input range. Jiff's
    // Timestamp range reserves space for every offset and ends before the final day.
    let days = i64::from(local_date.since(Date::constant(1970, 1, 1))?.get_days());
    let local_second = days
        .checked_mul(86400)
        .and_then(|s| s.checked_add(i64::from(hour * 3600 + minute * 60 + second.min(59))))
        .context("RFC3339 calendar is outside the integer range")?;
    let utc_second = local_second
        .checked_sub(i64::from(offset_seconds))
        .context("RFC3339 offset is outside the integer range")?;
    Ok(Rfc3339 {
        local_date,
        offset_seconds,
        leap_second: second == 60,
        second: utc_second,
        nanosecond,
    })
}

impl Rfc3339 {
    /// Exact checked milliseconds for durable scheduling after precision admission.
    pub fn unix_milliseconds(self) -> Result<i64> {
        ensure!(
            !self.leap_second,
            "Leap seconds have no ordinary Unix millisecond value"
        );
        ensure!(
            self.nanosecond % 1_000_000 == 0,
            "Timestamp exceeds millisecond precision"
        );
        self.second
            .checked_mul(1000)
            .and_then(|ms| ms.checked_add(i64::from(self.nanosecond / 1_000_000)))
            .context("Timestamp is outside the millisecond integer range")
    }
    pub fn nanosecond(self) -> i32 {
        self.nanosecond
    }
    pub fn compare_instant(self, other: Timestamp) -> Ordering {
        let (second, nanosecond) = unix_parts(other);
        (self.second, self.leap_second, self.nanosecond).cmp(&(second, false, nanosecond))
    }
    /// The previous finite-sequence implementation counted the explicitly named leap
    /// second on the target UTC day, without assuming any intervening leap seconds.
    pub fn duration_since(self, other: Timestamp) -> Result<SignedDuration> {
        let (other_second, other_nanosecond) = unix_parts(other);
        let seconds = self
            .second
            .checked_sub(other_second)
            .context("Timestamp duration overflow")?;
        // Each fraction is in 0..1e9; the difference stays within the constructor's no-overflow bound.
        let mut duration = SignedDuration::new(seconds, self.nanosecond - other_nanosecond);
        if self.leap_second
            && self.second.rem_euclid(86400) >= unix_parts(other).0.rem_euclid(86400)
        {
            duration = duration
                .checked_add(SignedDuration::from_secs(1))
                .context("Timestamp duration overflow")?;
        }
        Ok(duration)
    }
    fn utc_prefix(self) -> Result<String> {
        let days = self.second.div_euclid(86400);
        let date = Date::constant(1970, 1, 1).checked_add(days.days());
        let (year, month, day) = match date {
            Ok(date) => (i32::from(date.year()), date.month(), date.day()),
            // A negative RFC3339 offset can move the last accepted date into year 10000.
            Err(_)
                if days
                    == i64::from(Date::MAX.since(Date::constant(1970, 1, 1))?.get_days()) + 1 =>
            {
                (10000, 1, 1)
            }
            Err(error) => return Err(error.into()),
        };
        let clock = self.second.rem_euclid(86400) as i32;
        Ok(format_prefix(
            year,
            month,
            day,
            clock / 3600,
            clock / 60 % 60,
            clock % 60 + i32::from(self.leap_second),
        ))
    }
    pub fn utc_seconds(self) -> Result<String> {
        let mut text = self.utc_prefix()?;
        text.push('Z');
        Ok(text)
    }
    pub fn utc_auto(self) -> Result<String> {
        let mut text = self.utc_prefix()?;
        push_fraction(&mut text, self.nanosecond);
        text.push('Z');
        Ok(text)
    }
}

// Jiff's scalar seconds truncate toward zero; RFC3339 clock fields use a
// nonnegative fraction after the preceding whole second, including before 1970.
fn unix_parts(time: Timestamp) -> (i64, i32) {
    let nanos = time.subsec_nanosecond();
    (
        time.as_second() - i64::from(nanos < 0),
        nanos.rem_euclid(1_000_000_000),
    )
}

fn format_prefix(year: i32, month: i8, day: i8, hour: i32, minute: i32, second: i32) -> String {
    let sign = if year < 0 {
        "-"
    } else if year > 9999 {
        "+"
    } else {
        ""
    };
    let year = year.unsigned_abs();
    format!("{sign}{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

fn utc_prefix(time: Timestamp) -> String {
    let civil = jiff::tz::Offset::UTC.to_datetime(time);
    format_prefix(
        i32::from(civil.year()),
        civil.month(),
        civil.day(),
        i32::from(civil.hour()),
        i32::from(civil.minute()),
        i32::from(civil.second()),
    )
}

pub fn utc_seconds(time: Timestamp) -> String {
    let mut text = utc_prefix(time);
    text.push('Z');
    text
}

/// Preserve the public 0/3/6/9 fractional-digit representation (including zeros).
pub fn utc_auto(time: Timestamp) -> String {
    let mut text = utc_prefix(time);
    push_fraction(&mut text, unix_parts(time).1);
    text.push('Z');
    text
}

fn push_fraction(text: &mut String, nanos: i32) {
    if nanos != 0 {
        let digits = if nanos % 1_000_000 == 0 {
            3
        } else if nanos % 1000 == 0 {
            6
        } else {
            9
        };
        text.push('.');
        text.push_str(&format!("{nanos:09}")[..digits]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_calendar_day_keeps_integer_and_serialized_boundaries() -> Result<()> {
        let final_ms = parse_rfc3339("9999-12-31T23:59:59.999Z")?;
        assert_eq!(final_ms.unix_milliseconds()?, 253402300799999);
        assert_eq!(final_ms.utc_auto()?, "9999-12-31T23:59:59.999Z");
        assert_eq!(final_ms.utc_seconds()?, "9999-12-31T23:59:59Z");
        assert!(final_ms.compare_instant(Timestamp::MAX).is_gt());
        assert_eq!(
            final_ms.duration_since(Timestamp::UNIX_EPOCH)?,
            SignedDuration::new(253402300799, 999000000)
        );
        let offset = parse_rfc3339("9999-12-31T23:59:59.999-23:59")?;
        assert_eq!(offset.unix_milliseconds()?, 253402387139999);
        assert_eq!(offset.utc_auto()?, "+10000-01-01T23:58:59.999Z");
        assert_eq!(
            parse_rfc3339("0000-01-01T00:00:00Z")?.unix_milliseconds()?,
            -62167219200000
        );
        assert_eq!(
            parse_rfc3339("1969-12-31T23:59:59.999Z")?.unix_milliseconds()?,
            -1
        );
        let leap = parse_rfc3339("9999-12-31T23:59:60.999Z")?;
        assert!(leap.unix_milliseconds().is_err());
        assert_eq!(leap.utc_auto()?, "9999-12-31T23:59:60.999Z");
        assert!(
            parse_rfc3339("9999-12-31T23:59:59.999999999Z")?
                .unix_milliseconds()
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn legacy_rfc3339_boundary_and_serialization() -> Result<()> {
        for s in [
            "0000-01-01T00:00:00Z",
            "2024-02-29 12:34:60.123456789123456789z",
            "9999-12-31T23:59:59-23:59",
            "2024-01-01t00:00:00-00:00",
            "2024-01-01T00:00:00−00:00",
        ] {
            parse_rfc3339(s)?;
        }
        for s in [
            "2023-02-29T00:00:00Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:00:61Z",
            "2024-01-01T00:00:00.Z",
            "2024-01-01T00:00:00+24:00",
            "2024-01-01T00:00:00Zx",
            "20240101T000000Z",
            "2024-01-01T00:00:00+01",
        ] {
            assert!(parse_rfc3339(s).is_err(), "{s}");
        }
        for (s, expected) in [
            ("2024-01-01T00:00:00.100Z", "2024-01-01T00:00:00.100Z"),
            ("2024-01-01T00:00:00.000001Z", "2024-01-01T00:00:00.000001Z"),
            (
                "2024-01-01T00:00:00.000000001Z",
                "2024-01-01T00:00:00.000000001Z",
            ),
            ("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:00Z"),
            ("2024-01-01T00:00:60Z", "2024-01-01T00:00:60Z"),
        ] {
            assert_eq!(parse_rfc3339(s)?.utc_auto()?, expected);
        }
        assert_eq!(
            parse_rfc3339("0000-01-01T00:00:00+01:00")?.utc_auto()?,
            "-0001-12-31T23:00:00Z"
        );
        assert_eq!(
            utc_auto("1969-12-31T23:59:59.900Z".parse()?),
            "1969-12-31T23:59:59.900Z"
        );
        assert!(
            parse_rfc3339("1969-12-31T23:59:59.9Z")?
                .compare_instant("1969-12-31T23:59:59.9Z".parse()?)
                .is_eq()
        );
        assert_eq!(
            parse_rfc3339("2030-01-01T00:00:60.1Z")?
                .duration_since("2030-01-01T00:00:58.9Z".parse()?)?,
            SignedDuration::from_millis(1200)
        );
        assert_eq!(
            parse_rfc3339("2030-01-01T00:00:60.1Z")?
                .duration_since("2029-12-31T23:59:58.9Z".parse()?)?,
            SignedDuration::from_millis(60200)
        );
        let leap = parse_rfc3339("2024-01-01T00:00:60.1Z")?;
        assert!(
            leap.compare_instant("2024-01-01T00:00:59.999999999Z".parse()?)
                .is_gt()
        );
        assert!(
            leap.compare_instant("2024-01-01T00:01:00Z".parse()?)
                .is_lt()
        );
        Ok(())
    }
}
