//! On-demand native local offset conversion. No timezone database, cache or worker.
use jiff::{Timestamp, Zoned, tz::Offset};

/// Convert an instant with the OS's current local timezone rules, including DST.
/// Called only while constructing a calendar or explicitly capturing debug metadata.
pub fn local_datetime(timestamp: Timestamp) -> Zoned {
    let seconds = local_offset_seconds(timestamp);
    timestamp.to_zoned(
        Offset::from_seconds(seconds)
            .expect("native timezone offset")
            .to_time_zone(),
    )
}

#[cfg(any(unix, windows))]
fn unix_second(timestamp: Timestamp) -> i64 {
    timestamp.as_second() - i64::from(timestamp.subsec_nanosecond() < 0)
}

#[cfg(unix)]
fn local_offset_seconds(timestamp: Timestamp) -> i32 {
    let seconds: libc::time_t = unix_second(timestamp)
        .try_into()
        .expect("native time_t range");
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    unsafe extern "C" {
        fn tzset();
    }
    // Refresh native rules on demand; localtime_r alone can retain an old zone.
    // No environment mutation or application timezone cache is involved.
    let result = unsafe {
        tzset();
        libc::localtime_r(&seconds, local.as_mut_ptr())
    };
    assert!(!result.is_null(), "native local time conversion failed");
    let local = unsafe { local.assume_init() };
    local
        .tm_gmtoff
        .try_into()
        .expect("native timezone offset range")
}

#[cfg(windows)]
fn local_offset_seconds(timestamp: Timestamp) -> i32 {
    use windows::Win32::{Foundation::SYSTEMTIME, System::Time::SystemTimeToTzSpecificLocalTimeEx};
    let utc = Offset::UTC.to_datetime(timestamp);
    let input = SYSTEMTIME {
        wYear: utc.year() as u16,
        wMonth: utc.month() as u16,
        wDayOfWeek: 0,
        wDay: utc.day() as u16,
        wHour: utc.hour() as u16,
        wMinute: utc.minute() as u16,
        wSecond: utc.second() as u16,
        wMilliseconds: 0,
    };
    let mut output = SYSTEMTIME::default();
    // A null timezone selects the current dynamic native rules for this instant.
    unsafe { SystemTimeToTzSpecificLocalTimeEx(None, &input, &mut output) }
        .expect("native local time conversion failed");
    let local = jiff::civil::DateTime::new(
        output.wYear as i16,
        output.wMonth as i8,
        output.wDay as i8,
        output.wHour as i8,
        output.wMinute as i8,
        output.wSecond as i8,
        0,
    )
    .expect("native local calendar");
    let days = i64::from(
        local
            .date()
            .since(jiff::civil::Date::constant(1970, 1, 1))
            .expect("difference between validated civil dates")
            .get_days(),
    );
    // All Jiff civil days fit within i64 seconds; no narrower Timestamp conversion.
    let local_seconds = days * 86400
        + i64::from(local.hour()) * 3600
        + i64::from(local.minute()) * 60
        + i64::from(local.second());
    (local_seconds - unix_second(timestamp))
        .try_into()
        .expect("native timezone offset range")
}

#[cfg(target_family = "wasm")]
fn local_offset_seconds(timestamp: Timestamp) -> i32 {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        timestamp.as_nanosecond().div_euclid(1_000_000) as f64,
    ));
    (-date.get_timezone_offset() * 60.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_conversion_keeps_the_instant_and_milliseconds() {
        let now = Timestamp::now();
        let local = local_datetime(now);
        assert_eq!(local.timestamp(), now);
        assert_eq!(local.subsec_nanosecond(), now.subsec_nanosecond());
    }
}
