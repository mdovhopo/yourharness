//! Times as the transcript says them: when a message was sent ("10:42", "Yesterday 10:42",
//! "Oct 20, 10:42"), how long a turn took ("2m 14s") and how long one has been going ("0:12").

/// A moment in local time: (year, month 1–12, day, hour, minute, second).
fn local(seconds: f64) -> (i32, u32, u32, u32, u32, u32) {
    let t = seconds.floor() as libc::time_t;
    // SAFETY: localtime_r writes into the `tm` given and nothing else.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "10:42", "Yesterday 10:42", "Oct 20, 10:42", "Oct 20, 2025, 10:42": when a message was sent,
/// as macOS's 24-hour clock says it. Seconds since 1970.
pub fn message_time(date: f64, now: f64) -> String {
    let (y, m, d, hh, mm, _) = local(date);
    let time = format!("{hh}:{mm:02}");
    let today = local(now);
    if (y, m, d) == (today.0, today.1, today.2) {
        return time;
    }
    let yesterday = local(now - 86_400.0);
    if (y, m, d) == (yesterday.0, yesterday.1, yesterday.2) {
        return format!("Yesterday {time}");
    }
    let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
    if y == today.0 { format!("{month} {d}, {time}") } else { format!("{month} {d}, {y}, {time}") }
}

/// The whole date and time, for when there's room: "Oct 20, 2026 at 10:42:07".
pub fn full_time(date: f64) -> String {
    let (y, m, d, hh, mm, ss) = local(date);
    format!("{} {d}, {y} at {hh}:{mm:02}:{ss:02}", MONTHS[(m as usize).saturating_sub(1).min(11)])
}

/// "45s", "2m 14s", "1h 3m": how long a finished turn took.
pub fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total < 60 {
        format!("{}s", total.max(1))
    } else if total < 3600 {
        format!("{}m {}s", total / 60, total % 60)
    } else {
        format!("{}h {}m", total / 3600, (total % 3600) / 60)
    }
}

/// "0:12", "4:05", "1:02:09": how long a turn has been going, as a stopwatch says it.
pub fn elapsed(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    if total < 3600 {
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
    }
}

/// "now", "5m", "3h", "2d", "3w", "1mo", "2y": how long ago, as the sidebar says it.
pub fn short_age(date: f64, now: f64) -> String {
    let seconds = (now - date).max(0.0);
    let (minutes, hours, days) = (seconds / 60.0, seconds / 3600.0, seconds / 86_400.0);
    if seconds < 60.0 {
        "now".into()
    } else if seconds < 3600.0 {
        format!("{}m", minutes as i64)
    } else if seconds < 86_400.0 {
        format!("{}h", hours as i64)
    } else if seconds < 7.0 * 86_400.0 {
        format!("{}d", days as i64)
    } else if seconds < 30.0 * 86_400.0 {
        format!("{}w", (days / 7.0) as i64)
    } else if seconds < 365.0 * 86_400.0 {
        format!("{}mo", ((days / 30.0) as i64).max(1))
    } else {
        format!("{}y", (days / 365.0) as i64)
    }
}

/// "42m", "2h 14m", "3d 4h", "now": time left until `date`.
pub fn countdown(date: f64, now: f64) -> String {
    let minutes = (((date - now) / 60.0) as i64).max(0);
    let (days, hours, rest) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    if days > 0 {
        if hours > 0 { format!("{days}d {hours}h") } else { format!("{days}d") }
    } else if hours > 0 {
        if rest > 0 { format!("{hours}h {rest}m") } else { format!("{hours}h") }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "now".into()
    }
}

/// "in 2 hours", "3 minutes ago", "now", "tomorrow", "yesterday": as macOS's named relative
/// dates say it.
pub fn relative(date: f64, now: f64) -> String {
    let delta = date - now;
    let s = delta.abs();
    let (n, unit) = if s < 60.0 {
        return "now".into();
    } else if s < 3600.0 {
        ((s / 60.0) as i64, "minute")
    } else if s < 86_400.0 {
        ((s / 3600.0) as i64, "hour")
    } else if s < 7.0 * 86_400.0 {
        let days = (s / 86_400.0) as i64;
        if days == 1 {
            return if delta > 0.0 { "tomorrow".into() } else { "yesterday".into() };
        }
        (days, "day")
    } else if s < 30.0 * 86_400.0 {
        ((s / (7.0 * 86_400.0)) as i64, "week")
    } else if s < 365.0 * 86_400.0 {
        ((s / (30.0 * 86_400.0)) as i64, "month")
    } else {
        ((s / (365.0 * 86_400.0)) as i64, "year")
    };
    let unit = if n == 1 { unit.to_string() } else { format!("{unit}s") };
    if delta > 0.0 { format!("in {n} {unit}") } else { format!("{n} {unit} ago") }
}

/// "Nov 7, 2026 at 17:00": a date and time, short.
pub fn short_date_time(date: f64) -> String {
    let (y, m, d, hh, mm, _) = local(date);
    format!("{} {d}, {y} at {hh}:{mm:02}", MONTHS[(m as usize).saturating_sub(1).min(11)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_and_countdowns() {
        let now = 1_800_000_000.0;
        assert_eq!(short_age(now - 30.0, now), "now");
        assert_eq!(short_age(now - 300.0, now), "5m");
        assert_eq!(short_age(now - 3.0 * 3600.0, now), "3h");
        assert_eq!(short_age(now - 2.0 * 86_400.0, now), "2d");
        assert_eq!(short_age(now - 21.0 * 86_400.0, now), "3w");
        assert_eq!(short_age(now - 40.0 * 86_400.0, now), "1mo");
        assert_eq!(short_age(now - 800.0 * 86_400.0, now), "2y");
        assert_eq!(countdown(now + 42.0 * 60.0, now), "42m");
        assert_eq!(countdown(now + 134.0 * 60.0, now), "2h 14m");
        assert_eq!(countdown(now + 2.0 * 3600.0, now), "2h");
        assert_eq!(countdown(now + 76.0 * 3600.0, now), "3d 4h");
        assert_eq!(countdown(now - 5.0, now), "now");
        assert_eq!(relative(now + 7200.0, now), "in 2 hours");
        assert_eq!(relative(now - 180.0, now), "3 minutes ago");
        assert_eq!(relative(now - 86_400.0 * 1.5, now), "yesterday");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0.2), "1s");
        assert_eq!(duration(45.0), "45s");
        assert_eq!(duration(134.0), "2m 14s");
        assert_eq!(duration(3780.0), "1h 3m");
        assert_eq!(elapsed(12.7), "0:12");
        assert_eq!(elapsed(245.0), "4:05");
        assert_eq!(elapsed(3729.0), "1:02:09");
    }

    #[test]
    fn message_times() {
        let now = 1_800_000_000.0;
        assert!(!message_time(now, now).contains(' '));
        assert!(message_time(now - 86_400.0, now).starts_with("Yesterday "));
        assert!(message_time(now - 40.0 * 86_400.0, now).contains(", "));
        assert!(message_time(now - 400.0 * 86_400.0, now).matches(", ").count() == 2);
    }
}
