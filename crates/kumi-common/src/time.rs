//! Clocks as the TypeScript used them: `Date.now()` (milliseconds since the epoch) and
//! `performance.now()` (monotonic milliseconds since the process started).

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// `Date.now()`: milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `Date.now()` as JavaScript's number: whole milliseconds, as an `f64`.
pub fn now_ms_f64() -> f64 {
    now_ms() as f64
}

static START: OnceLock<Instant> = OnceLock::new();

/// `performance.now()`: monotonic milliseconds (fractional) since this clock was first read.
pub fn perf_now() -> f64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64() * 1000.0
}

/// `new Date(ms).toISOString()`, for the few places the TypeScript wrote one.
pub fn iso_string(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// The milliseconds an `iso_string` names (`YYYY-MM-DDTHH:MM:SS.mmmZ`, as it writes them); None for any other text.
pub fn iso_ms(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 24 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'.' || b[23] != b'Z' {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        let digits = text.get(from..to)?;
        digits.bytes().all(|c| c.is_ascii_digit()).then(|| digits.parse().ok())?
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second, millis) = (number(11, 13)?, number(14, 16)?, number(17, 19)?, number(20, 23)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(((days_from_civil(year, month as u32, day as u32) * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Howard Hinnant's civil-to-days algorithm, the inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_strings_match_javascript() {
        assert_eq!(iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(1_759_500_000_123), "2025-10-03T14:00:00.123Z");
        assert_eq!(iso_string(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn iso_strings_read_back_to_their_milliseconds() {
        for ms in [0, 1_759_500_000_123, -1, 951_782_400_000, 4_102_444_799_999, now_ms()] {
            assert_eq!(iso_ms(&iso_string(ms)), Some(ms), "{ms}");
        }
        for text in ["", "2025-10-03T14:00:00Z", "2025-13-03T14:00:00.123Z", "2025-10-03 14:00:00.123Z", "2025-1a-03T14:00:00.123Z"] {
            assert_eq!(iso_ms(text), None, "{text}");
        }
    }

    #[test]
    fn perf_now_is_monotonic() {
        let a = perf_now();
        let b = perf_now();
        assert!(b >= a);
    }
}
