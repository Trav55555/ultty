//! Local wall-clock time for file names.
//!
//! Uses the C library's time zone conversion (`localtime_r`), which honours
//! `TZ` and `/etc/localtime`. Ultty never changes its own environment at run
//! time, so the C library's unsynchronised `TZ` read cannot race a writer.

use std::time::{SystemTime, UNIX_EPOCH};

/// Formats `time` as `YYYY-MM-DD_HH-MM-SS` in the local time zone, so file
/// names sort by when they were made.
///
/// Returns the Unix seconds in decimal if the C library cannot convert the
/// time (it is out of range for `time_t` or the year overflows).
pub fn file_stamp(time: SystemTime) -> String {
    let secs = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |s| -s),
    };
    // Infallible where `time_t` is 64-bit; 32-bit `time_t` can overflow.
    let Some(clock) = libc::time_t::try_from(secs).ok() else {
        return secs.to_string();
    };
    // SAFETY: `libc::tm` is a plain C struct of integers and, on some
    // platforms, a `tm_zone` pointer; all-zero is a valid value for each.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers refer to live locals for the whole call;
    // `localtime_r` only reads `clock` and writes `tm`.
    if unsafe { libc::localtime_r(&clock, &mut tm) }.is_null() {
        return secs.to_string();
    }
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        i64::from(tm.tm_year) + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// True when `stamp` is `YYYY-MM-DD_HH-MM-SS` with in-range fields.
    fn is_stamp(stamp: &str) -> bool {
        let fields: Vec<&str> = stamp.split(['-', '_']).collect();
        let [year, month, day, hour, minute, second] = fields[..] else {
            return false;
        };
        let parse = |s: &str| (s.len() == 2).then(|| s.parse::<u32>().ok()).flatten();
        year.len() == 4
            && year.parse::<u32>().is_ok()
            && parse(month).is_some_and(|m| (1..=12).contains(&m))
            && parse(day).is_some_and(|d| (1..=31).contains(&d))
            && parse(hour).is_some_and(|h| h < 24)
            && parse(minute).is_some_and(|m| m < 60)
            && parse(second).is_some_and(|s| s < 61)
    }

    #[test]
    fn now_formats_as_a_sortable_stamp() {
        let stamp = file_stamp(SystemTime::now());
        assert!(is_stamp(&stamp), "{stamp}");
    }

    #[test]
    fn stamps_sort_in_time_order() {
        // A day apart, so no local offset or DST change can reorder them.
        let earlier = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let later = earlier + Duration::from_secs(86_400);
        let (a, b) = (file_stamp(earlier), file_stamp(later));
        assert!(is_stamp(&a) && is_stamp(&b), "{a} {b}");
        assert!(a < b, "{a} !< {b}");
        // 1_800_000_000 is 2027-01-15 08:00 UTC; any real offset keeps the date
        // within a day of that.
        assert!(a.starts_with("2027-01-1"), "{a}");
    }

    #[test]
    fn times_before_the_epoch_still_format() {
        let stamp = file_stamp(UNIX_EPOCH - Duration::from_secs(86_400 * 365));
        assert!(is_stamp(&stamp), "{stamp}");
        assert!(
            stamp.starts_with("1969-") || stamp.starts_with("1968-"),
            "{stamp}"
        );
    }
}
