//! UTC dates and times as milliseconds since the Unix epoch, without a date
//! library: PGN's `UTCDate`/`UTCTime`, the dialog's `YYYY-MM-DD`, and the
//! months of Chess.com's archives.

use std::time::{SystemTime, UNIX_EPOCH};

const DAY_MS: i64 = 86_400_000;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Days since 1970-01-01 of a (proleptic Gregorian) date; Howard Hinnant's
/// `days_from_civil`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01: (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// The start of a month, in ms.
pub fn month_start_ms(y: i64, m: u32) -> i64 {
    days_from_civil(y, m, 1) * DAY_MS
}

/// (year, month) of a time in ms.
pub fn month_of(ms: i64) -> (i64, u32) {
    let (y, m, _) = civil_from_days(ms.div_euclid(DAY_MS));
    (y, m)
}

/// `2026.10.23` or `2026-10-23` → (2026, 10, 23); `None` for an unknown or
/// impossible part.
fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let mut parts = s.trim().split(['.', '-']);
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// `16:19:50` → ms into the day.
fn parse_time(s: &str) -> Option<i64> {
    let mut parts = s.trim().split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let sec: i64 = parts.next().map_or(Some(0), |p| p.split('.').next()?.parse().ok())?;
    if parts.next().is_some() || h > 23 || m > 59 || sec > 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * 1000)
}

/// The start of a day given as `YYYY-MM-DD` (or PGN's `YYYY.MM.DD`), in ms.
pub fn date_ms(date: &str) -> Option<i64> {
    let (y, m, d) = parse_date(date)?;
    Some(days_from_civil(y, m, d) * DAY_MS)
}

/// When a game started, from its date and (if known) time, in ms.
pub fn date_time_ms(date: &str, time: Option<&str>) -> Option<i64> {
    let day = date_ms(date)?;
    Some(day + time.and_then(parse_time).unwrap_or(0))
}

/// A time in ms as `YYYY-MM-DD`.
pub fn ymd(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        for days in [-1_000_000, -1, 0, 1, 11_016, 19_000, 20_749, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
    }

    #[test]
    fn reads_pgn_dates_and_times() {
        // 2018-12-15 19:03:11 UTC, a Lichess game's createdAt to the second.
        assert_eq!(date_time_ms("2018.12.15", Some("19:03:11")), Some(1_544_900_591_000));
        assert_eq!(date_time_ms("2018.12.15", None), date_ms("2018-12-15"));
        assert_eq!(date_time_ms("2018.12.15", Some("??:??:??")), date_ms("2018-12-15"));
        assert_eq!(date_time_ms("2018.??.??", Some("19:03:11")), None);
        assert_eq!(date_ms("2018-13-01"), None);
        assert_eq!(date_ms("2026-10-23"), Some(month_start_ms(2026, 10) + 22 * DAY_MS));
        assert_eq!(month_of(date_ms("2026-10-23").unwrap()), (2026, 10));
        assert_eq!(ymd(1_544_900_591_000), "2018-12-15");
    }
}
