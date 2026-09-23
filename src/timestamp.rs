//! 墙钟时间戳。
//!
//! 日期换算自己做而不是引入 `chrono` / `time`：项目对时间的全部需求就是
//! 「给输出打一个可读且可解析的时刻」，为此拉一个日期库不值得。该实现原本
//! 长在 `diag` 里，补 RFC 3339 需求时提取到这里——否则两处会各自维护一份
//! 日历换算，迟早对不上。

use std::time::{SystemTime, UNIX_EPOCH};

/// RFC 3339（UTC，毫秒精度），如 `2026-09-23T07:54:29.412Z`。
///
/// 机器消费的字段统一用这个格式，便于把探测记录与其他遥测按时间对齐。
pub fn now_rfc3339_millis() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_rfc3339_millis(now.as_secs(), now.subsec_millis())
}

/// `YYYY-MM-DD HH:MM:SS`，给人看的表格用。
pub fn now_display() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_display(now.as_secs())
}

fn split_datetime(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = secs / 86400;
    let hour = (secs % 86400) / 3600;
    let min = (secs % 3600) / 60;
    let sec = secs % 60;
    let (year, month, day) = days_to_date(days as i64);
    (year, month, day, hour, min, sec)
}

fn format_display(secs: u64) -> String {
    let (year, month, day, hour, min, sec) = split_datetime(secs);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn format_rfc3339_millis(secs: u64, millis: u32) -> String {
    let (year, month, day, hour, min, sec) = split_datetime(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}Z")
}

/// 天数转日期（从 1970-01-01）
fn days_to_date(days: i64) -> (i64, u32, u32) {
    let mut year = 1970i64;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    let mut day = remaining as u32 + 1;

    for (i, &md) in month_days.iter().enumerate() {
        let md = if i == 1 && is_leap(year) { 29 } else { md };
        if day <= md {
            month = (i + 1) as u32;
            break;
        }
        day -= md;
    }

    (year, month, day)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 各基准时刻的 Unix 秒，便于核对：
    // 1704067200 = 2024-01-01T00:00:00Z
    // 1709164800 = 2024-02-29T00:00:00Z（1704067200 + 31 天 + 28 天）
    const EPOCH_2024_02_29: u64 = 1_709_164_800;
    const EPOCH_2024_01_01: u64 = 1_704_067_200;

    #[test]
    fn formats_the_unix_epoch() {
        assert_eq!(format_display(0), "1970-01-01 00:00:00");
        assert_eq!(format_rfc3339_millis(0, 0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn handles_leap_day() {
        assert_eq!(
            format_display(EPOCH_2024_02_29 + 12 * 3600),
            "2024-02-29 12:00:00"
        );
    }

    #[test]
    fn handles_year_boundary() {
        // 跨年前一秒：闰年累加若算错，这里会落到 2024-01-01 或 2023-12-30
        assert_eq!(format_display(EPOCH_2024_01_01 - 1), "2023-12-31 23:59:59");
        assert_eq!(format_display(EPOCH_2024_01_01), "2024-01-01 00:00:00");
    }

    #[test]
    fn pads_milliseconds_to_three_digits() {
        assert_eq!(format_rfc3339_millis(0, 7), "1970-01-01T00:00:00.007Z");
        assert_eq!(format_rfc3339_millis(0, 70), "1970-01-01T00:00:00.070Z");
        assert_eq!(format_rfc3339_millis(0, 700), "1970-01-01T00:00:00.700Z");
    }

    #[test]
    fn identifies_leap_years() {
        assert!(is_leap(2024));
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert!(!is_leap(2023));
    }
}
