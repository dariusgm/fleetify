//! UTC time without a time crate: RFC 3339 timestamps and dates for run ids.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Today's date in UTC as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    let (y, m, d) = civil_from_days((unix_now() / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339_now() -> String {
    rfc3339(unix_now())
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Converts days since 1970-01-01 to a (year, month, day) date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn ci_loop_probe() -> u8 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_converted_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn rfc3339_formats_seconds() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400 + 3661), "2000-02-29T01:01:01Z");
    }
}
