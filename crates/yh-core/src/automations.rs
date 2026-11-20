//! Automations: a prompt a project runs by itself, once at a time or on a schedule (every so
//! many minutes, at a time of day on some weekdays, or a cron line). Each run starts a new
//! thread in the project, or goes to the same thread every time. Times are the Mac's local
//! time. They're kept in one setting, `automations` (a JSON array), so the database keeps the
//! format 0.0.x wrote; an entry this version can't read is kept and saved back untouched.
//!
//! The app's timer asks `due` and records each run with `ran`; a run missed while the app was
//! closed or the Mac slept happens once when it's back, then the schedule carries on. The agent
//! tools are `automation_tools.rs`; parsing what people write (`parse`) and saying it back
//! (`Schedule::describe`) are here, so the chat and the page agree.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

/// The setting the automations are kept in.
pub const SETTING: &str = "automations";
/// Runs remembered per automation, newest last.
const MAX_RUNS: usize = 20;
/// A run later than this after it was due counts as caught up (the app was closed, or asleep).
const LATE: f64 = 120.0;
/// The shortest repeat: a minute.
const MIN_EVERY: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// Once, at this moment (seconds since 1970).
    Once { at: f64 },
    /// Every so many minutes, counted from `from`.
    Every { minutes: u32, #[serde(default)] from: f64 },
    /// At a time of day on the weekdays given (0 Sunday … 6 Saturday; none: every day).
    Daily { hour: u32, minute: u32, #[serde(default)] weekdays: Vec<u8> },
    /// A cron line: minute, hour, day of month, month, day of week.
    Cron { expr: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// When it ran.
    pub at: f64,
    /// The thread it started or sent to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Why it couldn't run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// It ran late: it came due while the app was closed or the Mac slept.
    #[serde(default)]
    pub caught_up: bool,
    /// Run Now, not the schedule.
    #[serde(default)]
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// What's sent to Claude each run.
    pub prompt: String,
    pub schedule: Schedule,
    /// Every run goes to one thread (`thread_id`, made by the first run when there's none)
    /// rather than a new thread each time.
    #[serde(default)]
    pub same_thread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub enabled: bool,
    pub created_at: f64,
    /// The thread whose agent made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// When it runs next; None when it's paused or a one-time run has happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<f64>,
    #[serde(default)]
    pub runs: Vec<Run>,
}

impl Automation {
    /// A new automation, on, its first run worked out. A one-time run in the past is refused.
    pub fn new(project_id: &str, name: &str, prompt: &str, schedule: Schedule, now: f64) -> Result<Automation, String> {
        let next = schedule.next_after(now);
        if next.is_none() {
            return Err("That time has already passed.".into());
        }
        Ok(Automation {
            id: crate::library::new_uuid(),
            project_id: project_id.into(),
            name: name.trim().into(),
            prompt: prompt.trim().into(),
            schedule,
            same_thread: false,
            thread_id: None,
            model: None,
            effort: None,
            enabled: true,
            created_at: now,
            created_by: None,
            next_run: next,
            runs: vec![],
        })
    }

    /// Whether the timer should run it now.
    pub fn due(&self, now: f64) -> bool {
        self.enabled && self.next_run.is_some_and(|t| t <= now)
    }

    /// Records a run and works out the next one. A one-time automation is done after its run
    /// (not after Run Now: it still runs at its time).
    pub fn ran(&mut self, now: f64, thread_id: Option<String>, error: Option<String>, manual: bool) {
        let caught_up = !manual && self.next_run.is_some_and(|t| now - t > LATE);
        if self.same_thread && error.is_none() && thread_id.is_some() {
            self.thread_id = thread_id.clone();
        }
        self.runs.push(Run { at: now, thread_id, error, caught_up, manual });
        if self.runs.len() > MAX_RUNS {
            self.runs.drain(..self.runs.len() - MAX_RUNS);
        }
        if !manual {
            self.next_run = self.schedule.next_after(now);
            if self.next_run.is_none() {
                self.enabled = false;
            }
        }
    }

    /// Paused or resumed; on resume, the next run from now (none left: it stays paused).
    pub fn set_enabled(&mut self, on: bool, now: f64) {
        self.next_run = if on { self.schedule.next_after(now) } else { None };
        self.enabled = on && self.next_run.is_some();
    }

    /// A new schedule, the next run from now.
    pub fn set_schedule(&mut self, schedule: Schedule, now: f64) -> Result<(), String> {
        let next = schedule.next_after(now);
        if next.is_none() {
            return Err("That time has already passed.".into());
        }
        self.schedule = schedule;
        if self.enabled {
            self.next_run = next;
        }
        Ok(())
    }

    pub fn last_run(&self) -> Option<&Run> {
        self.runs.last()
    }

    /// The first 8 characters of its id, as the tools show it.
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

// ---------------------------------------------------------------------------------------------
// Local time

/// A moment in local time: (year, month 1–12, day, hour, minute, weekday 0 Sunday).
fn local(seconds: f64) -> (i32, u32, u32, u32, u32, u32) {
    let t = seconds.floor() as libc::time_t;
    // SAFETY: localtime_r writes into the `tm` given and nothing else.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_wday as u32)
}

/// Seconds since 1970 of a local date and time; out-of-range days roll over (the 32nd is the
/// next month's 1st). A time skipped by daylight saving lands an hour on.
fn moment(year: i32, month: u32, day: i32, hour: u32, minute: u32) -> f64 {
    // SAFETY: mktime reads and normalizes the `tm` given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as f64 }
}

/// The local zone's short name ("CET", "PST").
pub fn zone() -> String {
    let t = now() as libc::time_t;
    // SAFETY: as in `local`; tm_zone points at a static string of the C library.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    if tm.tm_zone.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }.to_string_lossy().into_owned()
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "today at 9:00", "tomorrow at 9:00", "yesterday at 9:00", "Mon, Nov 24 at 9:00",
/// "Nov 24, 2027 at 9:00": a run's time, past or coming.
pub fn when(t: f64, now: f64) -> String {
    let (y, m, d, hh, mm, wd) = local(t);
    let time = format!("{hh}:{mm:02}");
    let day = |offset: f64| {
        let o = local(now + offset);
        (o.0, o.1, o.2) == (y, m, d)
    };
    if day(0.0) {
        return format!("today at {time}");
    }
    if day(86_400.0) {
        return format!("tomorrow at {time}");
    }
    if day(-86_400.0) {
        return format!("yesterday at {time}");
    }
    let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
    if y == local(now).0 {
        format!("{}, {month} {d} at {time}", DAYS[wd as usize % 7])
    } else {
        format!("{month} {d}, {y} at {time}")
    }
}

// ---------------------------------------------------------------------------------------------
// Schedules

impl Schedule {
    /// The first run strictly after `after`, or None (a one-time run that's passed, a cron line
    /// that never matches).
    pub fn next_after(&self, after: f64) -> Option<f64> {
        match self {
            Schedule::Once { at } => (*at > after).then_some(*at),
            Schedule::Every { minutes, from } => {
                let period = (*minutes).max(MIN_EVERY) as f64 * 60.0;
                if *from > after {
                    return Some(*from);
                }
                let k = ((after - from) / period).floor() + 1.0;
                Some(from + k * period)
            }
            Schedule::Daily { hour, minute, weekdays } => {
                let (y, m, d, ..) = local(after);
                (0..9).map(|offset| moment(y, m, d as i32 + offset, *hour, *minute)).find(|t| *t > after && (weekdays.is_empty() || weekdays.contains(&(local(*t).5 as u8))))
            }
            Schedule::Cron { expr } => Cron::parse(expr).ok()?.next_after(after),
        }
    }

    /// "Every day at 9:00", "Weekdays at 9:00", "Mon, Wed at 18:30", "Every 2 hours", "Once,
    /// tomorrow at 9:00", "Cron 0 9 * * 1-5".
    pub fn describe(&self, now: f64) -> String {
        match self {
            Schedule::Once { at } => format!("Once, {}", when(*at, now)),
            Schedule::Every { minutes, .. } => every_words(*minutes),
            Schedule::Daily { hour, minute, weekdays } => {
                let time = format!("{hour}:{minute:02}");
                let mut days = weekdays.clone();
                days.sort_unstable();
                days.dedup();
                match days.as_slice() {
                    [] | [0, 1, 2, 3, 4, 5, 6] => format!("Every day at {time}"),
                    [1, 2, 3, 4, 5] => format!("Weekdays at {time}"),
                    [0, 6] => format!("Weekends at {time}"),
                    [one] => format!("{}s at {time}", DAY_NAMES[*one as usize % 7]),
                    many => format!("{} at {time}", many.iter().map(|d| DAYS[*d as usize % 7]).collect::<Vec<_>>().join(", ")),
                }
            }
            Schedule::Cron { expr } => format!("Cron {expr}"),
        }
    }

    /// Repeats (anything but once).
    pub fn repeats(&self) -> bool {
        !matches!(self, Schedule::Once { .. })
    }
}

const DAY_NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn every_words(minutes: u32) -> String {
    match minutes {
        1 => "Every minute".into(),
        1440 => "Every day".into(),
        m if m % 1440 == 0 => format!("Every {} days", m / 1440),
        60 => "Every hour".into(),
        m if m % 60 == 0 => format!("Every {} hours", m / 60),
        m => format!("Every {m} minutes"),
    }
}

/// What `parse` takes, for error messages and the tools' descriptions.
pub const FORMS: &str = "\"in 30m\", \"tomorrow 9:00\", \"2026-12-01 14:30\" (once); \"every 15m\", \"every 2h\", \"hourly\"; \"daily 9:00\", \"weekdays 8:30\", \"weekends 10am\", \"mon,thu 18:00\", \"every friday at 17:00\"; \"cron 0 9 * * 1-5\"";

/// A schedule from what someone wrote (see `FORMS`), in local time.
pub fn parse(text: &str, now: f64) -> Result<Schedule, String> {
    let lower = text.trim().to_lowercase().replace(" at ", " ").replace('\u{a0}', " ");
    let words: Vec<&str> = lower.split_whitespace().collect();
    let fail = || Err(format!("Couldn't read the schedule \"{}\". Write it like {FORMS}.", text.trim()));
    let Some(first) = words.first().copied() else { return fail() };
    if first == "cron" || (words.len() == 5 && Cron::parse(&lower).is_ok()) {
        let expr = if first == "cron" { words[1..].join(" ") } else { words.join(" ") };
        Cron::parse(&expr)?;
        return Ok(Schedule::Cron { expr });
    }
    if first == "hourly" && words.len() == 1 {
        return Ok(Schedule::Every { minutes: 60, from: start(now, 60) });
    }
    let rest = &words[1..];
    match first {
        "in" => return span(&rest.join("")).map(|m| Schedule::Once { at: (now + m as f64 * 60.0).floor() }).ok_or(()).or_else(|_| fail()),
        "once" | "on" => return once(rest, now).ok_or(()).or_else(|_| fail()),
        "every" | "each" => {
            // "every 15m", "every 2 hours", "every hour", "every day 9:00", "every monday 9:00"
            if let Some(m) = span(&rest.join("")).or_else(|| span(&format!("1{}", rest.join("")))) {
                let m = m.max(MIN_EVERY);
                return Ok(Schedule::Every { minutes: m, from: start(now, m) });
            }
            return daily(rest, now).ok_or(()).or_else(|_| fail());
        }
        _ => {}
    }
    // "monday 9:00" is the next Monday; "mondays", "mon,thu" and "weekly mon" repeat.
    let one_day = words.len() <= 2 && weekday(first).is_some() && !first.ends_with('s');
    if !one_day {
        if let Some(s) = daily(&words, now) {
            return Ok(s);
        }
    }
    once(&words, now).ok_or(()).or_else(|_| fail())
}

/// Where "every …" counts from: one period on, on the minute.
fn start(now: f64, minutes: u32) -> f64 {
    (now / 60.0).floor() * 60.0 + minutes as f64 * 60.0
}

/// "30m", "2h", "1d", "90min", "2hours", "1h30m": minutes.
fn span(text: &str) -> Option<u32> {
    let mut total = 0u64;
    let mut rest = text;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let n: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        total += n * match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => 1,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60,
            "d" | "day" | "days" => 1440,
            "w" | "week" | "weeks" => 10080,
            _ => return None,
        };
    }
    (total > 0 && total <= u32::MAX as u64).then_some(total as u32)
}

/// "9", "9:30", "09:30", "9am", "9:30pm", "21:00", "noon", "midnight": (hour, minute).
fn time_of_day(text: &str) -> Option<(u32, u32)> {
    match text {
        "noon" => return Some((12, 0)),
        "midnight" => return Some((0, 0)),
        _ => {}
    }
    let (body, pm) = match (text.strip_suffix("am"), text.strip_suffix("pm")) {
        (Some(b), _) => (b, Some(false)),
        (_, Some(b)) => (b, Some(true)),
        _ => (text, None),
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if !body.is_empty() && body.len() <= 2 => (body.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match pm {
        Some(true) if (1..=12).contains(&h) => h % 12 + 12,
        Some(false) if (1..=12).contains(&h) => h % 12,
        Some(_) => return None,
        None => h,
    };
    (h < 24 && m < 60).then_some((h, m))
}

fn weekday(word: &str) -> Option<u8> {
    let w = word.trim_end_matches('s');
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"].iter().position(|d| w.starts_with(d) && (w.len() == 3 || DAY_NAMES.iter().any(|n| n.to_lowercase().starts_with(w)))).map(|i| i as u8)
}

/// "daily 9:00", "day 9", "weekdays 8:30", "weekends", "mon,thu 18:00", "monday 9am",
/// "weekly fri 17:00": a time on some days. A missing time is 9:00.
fn daily(words: &[&str], _now: f64) -> Option<Schedule> {
    let (days_words, time) = match words.last().and_then(|w| time_of_day(w)) {
        Some(t) if words.len() > 1 => (&words[..words.len() - 1], t),
        _ => (words, (9, 0)),
    };
    let joined = days_words.iter().filter(|w| **w != "weekly" && **w != "every").copied().collect::<Vec<_>>().join(",");
    let weekdays: Vec<u8> = match joined.as_str() {
        "daily" | "day" | "days" | "everyday" | "" if !days_words.is_empty() => vec![],
        "weekdays" | "weekday" | "workdays" => vec![1, 2, 3, 4, 5],
        "weekends" | "weekend" => vec![0, 6],
        list => {
            let mut days = vec![];
            for part in list.split(',').filter(|p| !p.is_empty()) {
                if let Some((a, b)) = part.split_once('-') {
                    let (a, b) = (weekday(a)?, weekday(b)?);
                    let mut d = a;
                    loop {
                        days.push(d);
                        if d == b {
                            break;
                        }
                        d = (d + 1) % 7;
                    }
                } else {
                    days.push(weekday(part)?);
                }
            }
            if days.is_empty() {
                return None;
            }
            days.sort_unstable();
            days.dedup();
            days
        }
    };
    Some(Schedule::Daily { hour: time.0, minute: time.1, weekdays })
}

/// "tomorrow 9:00", "today 17:30", "2026-12-01 14:30", "2026-12-01T14:30", "dec 1 14:30",
/// "monday 9:00" (the next one), "14:30" (today, or tomorrow when it's passed).
fn once(words: &[&str], now: f64) -> Option<Schedule> {
    let (y, m, d, ..) = local(now);
    let joined = words.join(" ");
    // ISO: "2026-12-01T14:30[:00]"
    let (date_part, time_part) = match joined.split_once('t') {
        Some((a, b)) if a.len() == 10 && a.as_bytes()[4] == b'-' => (a.to_string(), Some(b.split(':').take(2).collect::<Vec<_>>().join(":"))),
        _ => match words {
            [date, time] => (date.to_string(), Some(time.to_string())),
            [one] if time_of_day(one).is_some() => (String::new(), Some(one.to_string())),
            [one] => (one.to_string(), None),
            [mon, day, time] if MONTHS.iter().any(|n| mon.starts_with(&n.to_lowercase())) => (format!("{mon} {day}"), Some(time.to_string())),
            _ => return None,
        },
    };
    let (hh, mm) = match &time_part {
        Some(t) => time_of_day(t)?,
        None => (9, 0),
    };
    let at = match date_part.as_str() {
        "" => {
            let t = moment(y, m, d as i32, hh, mm);
            if t > now { t } else { moment(y, m, d as i32 + 1, hh, mm) }
        }
        "today" => moment(y, m, d as i32, hh, mm),
        "tomorrow" => moment(y, m, d as i32 + 1, hh, mm),
        iso if iso.len() == 10 && iso.as_bytes()[4] == b'-' && iso.as_bytes()[7] == b'-' => {
            let year: i32 = iso[..4].parse().ok()?;
            let month: u32 = iso[5..7].parse().ok()?;
            let day: i32 = iso[8..10].parse().ok()?;
            if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                return None;
            }
            moment(year, month, day, hh, mm)
        }
        other => {
            if let Some(wd) = weekday(other) {
                (1..=7).map(|o| moment(y, m, d as i32 + o, hh, mm)).find(|t| local(*t).5 as u8 == wd)?
            } else {
                // "dec 1"
                let (mon, day) = other.split_once(' ')?;
                let month = MONTHS.iter().position(|n| mon.starts_with(&n.to_lowercase()))? as u32 + 1;
                let day: i32 = day.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
                let t = moment(y, month, day, hh, mm);
                if t > now { t } else { moment(y + 1, month, day, hh, mm) }
            }
        }
    };
    Some(Schedule::Once { at })
}

// ---------------------------------------------------------------------------------------------
// Cron

/// A parsed cron line: which minutes, hours, days, months and weekdays match.
#[derive(Debug)]
struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    fn parse(expr: &str) -> Result<Cron, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("A cron line has five fields (minute hour day month weekday), like \"0 9 * * 1-5\"; \"{expr}\" has {}.", fields.len()));
        }
        let bad = |what: &str, f: &str| format!("Couldn't read the cron {what} field \"{f}\".");
        let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
        let days = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
        let minutes = field(fields[0], 0, 59, &[], 0).ok_or_else(|| bad("minute", fields[0]))?;
        let hours = field(fields[1], 0, 23, &[], 0).ok_or_else(|| bad("hour", fields[1]))?;
        let dom = field(fields[2], 1, 31, &[], 0).ok_or_else(|| bad("day", fields[2]))?;
        let mon = field(fields[3], 1, 12, &months, 1).ok_or_else(|| bad("month", fields[3]))?;
        let mut dow = field(fields[4], 0, 7, &days, 0).ok_or_else(|| bad("weekday", fields[4]))?;
        if dow & (1 << 7) != 0 {
            dow = (dow & !(1 << 7)) | 1;
        }
        Ok(Cron { minutes: minutes as u64, hours: hours as u32, days: dom as u32, months: mon as u16, weekdays: dow as u8, any_day: fields[2] == "*", any_weekday: fields[4] == "*" })
    }

    fn day_matches(&self, day: u32, weekday: u32) -> bool {
        let d = self.days & (1 << day) != 0;
        let w = self.weekdays & (1 << weekday) != 0;
        // As cron has it: when both are restricted, either matches.
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => w,
            (false, true) => d,
            (false, false) => d || w,
        }
    }

    fn next_after(&self, after: f64) -> Option<f64> {
        let (y, m, d, ..) = local(after);
        // Five years of days at most: "0 0 30 2 *" never comes.
        for offset in 0..(366 * 5) {
            let midnight = moment(y, m, d as i32 + offset, 0, 0);
            let (yy, mm, dd, _, _, wd) = local(midnight);
            if self.months & (1 << mm) == 0 || !self.day_matches(dd, wd) {
                continue;
            }
            for hour in (0..24).filter(|h| self.hours & (1 << h) != 0) {
                for minute in (0..60).filter(|mi| self.minutes & (1u64 << mi) != 0) {
                    let t = moment(yy, mm, dd as i32, hour, minute);
                    // A time daylight saving skips rolls into the next hour: not this one.
                    if t > after && local(t).3 == hour {
                        return Some(t);
                    }
                }
            }
        }
        None
    }
}

/// One cron field as bits: "*", "5", "1-5", "*/15", "0-30/10", "mon-fri", lists of those.
fn field(text: &str, lo: u32, hi: u32, names: &[&str], name_base: u32) -> Option<u64> {
    let value = |s: &str| -> Option<u32> {
        if let Ok(n) = s.parse::<u32>() {
            return (lo..=hi).contains(&n).then_some(n);
        }
        names.iter().position(|n| s.eq_ignore_ascii_case(n)).map(|i| i as u32 + name_base)
    };
    let mut bits = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (a, b) = match range {
            "*" => (lo, hi),
            r => match r.split_once('-') {
                Some((a, b)) => (value(a)?, value(b)?),
                None if step > 1 => (value(r)?, hi),
                None => (value(r)?, value(r)?),
            },
        };
        if a > b {
            return None;
        }
        let mut n = a;
        while n <= b {
            bits |= 1 << n;
            n += step;
        }
    }
    (bits != 0).then_some(bits)
}

// ---------------------------------------------------------------------------------------------
// Storage

/// Where the automations are kept: the library's settings, or a stand-in in tests.
pub trait Settings: Send + Sync {
    fn get(&self) -> Option<Value>;
    fn set(&self, value: &Value);
}

impl Settings for crate::library::Library {
    fn get(&self) -> Option<Value> {
        self.setting(SETTING)
    }
    fn set(&self, value: &Value) {
        self.set_setting(SETTING, value)
    }
}

/// One change at a time: the timer on the main thread and agents' tools on theirs.
static LOCK: Mutex<()> = Mutex::new(());

/// Every automation, in the order they were made.
pub fn all(store: &dyn Settings) -> Vec<Automation> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    split(store).0
}

fn split(store: &dyn Settings) -> (Vec<Automation>, Vec<Value>) {
    let mut known = vec![];
    let mut unknown = vec![];
    if let Some(Value::Array(items)) = store.get() {
        for item in items {
            match serde_json::from_value::<Automation>(item.clone()) {
                Ok(a) => known.push(a),
                Err(_) => unknown.push(item),
            }
        }
    }
    (known, unknown)
}

/// Changes the automations and saves them; what this version couldn't read is kept.
pub fn edit<R>(store: &dyn Settings, change: impl FnOnce(&mut Vec<Automation>) -> R) -> R {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut known, unknown) = split(store);
    let before = known.clone();
    let result = change(&mut known);
    if known != before {
        let mut items: Vec<Value> = known.iter().filter_map(|a| serde_json::to_value(a).ok()).collect();
        items.extend(unknown);
        store.set(&Value::Array(items));
    }
    result
}

/// An automation of `project` by its id or the start of it (6 characters or more), or its name.
pub fn find<'a>(list: &'a [Automation], project: &str, key: &str) -> Result<&'a Automation, String> {
    let key = key.trim();
    let mine: Vec<&Automation> = list.iter().filter(|a| a.project_id == project).collect();
    if let Some(a) = mine.iter().find(|a| a.id == key) {
        return Ok(a);
    }
    let by_id: Vec<&&Automation> = mine.iter().filter(|a| key.len() >= 6 && a.id.to_lowercase().starts_with(&key.to_lowercase())).collect();
    if by_id.len() == 1 {
        return Ok(by_id[0]);
    }
    let by_name: Vec<&&Automation> = mine.iter().filter(|a| a.name.eq_ignore_ascii_case(key)).collect();
    match by_name.len() {
        1 => Ok(by_name[0]),
        0 => Err(format!("No automation \"{key}\" in this project.{}", if mine.is_empty() { String::new() } else { format!(" There are: {}.", mine.iter().map(|a| format!("{} ({})", a.name, a.short_id())).collect::<Vec<_>>().join(", ")) })),
        _ => Err(format!("More than one automation is called \"{key}\"; give its id.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-21 (a Saturday) at 10:15 local.
    fn base() -> f64 {
        moment(2026, 11, 21, 10, 15)
    }

    #[test]
    fn every_counts_from_its_start() {
        let s = Schedule::Every { minutes: 30, from: base() };
        assert_eq!(s.next_after(base() - 1.0), Some(base()));
        assert_eq!(s.next_after(base()), Some(base() + 1800.0));
        assert_eq!(s.next_after(base() + 3601.0), Some(base() + 5400.0));
    }

    #[test]
    fn daily_picks_the_next_matching_day() {
        let s = Schedule::Daily { hour: 9, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 22, 9, 0)));
        let s = Schedule::Daily { hour: 11, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 21, 11, 0)));
        // Weekdays from a Saturday: Monday.
        let s = Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 2, 3, 4, 5] };
        assert_eq!(s.next_after(base()), Some(moment(2026, 11, 23, 9, 0)));
        // Over a month's end.
        let s = Schedule::Daily { hour: 8, minute: 0, weekdays: vec![] };
        assert_eq!(s.next_after(moment(2026, 11, 30, 9, 0)), Some(moment(2026, 12, 1, 8, 0)));
    }

    #[test]
    fn cron_lines() {
        let next = |e: &str, after: f64| Schedule::Cron { expr: e.into() }.next_after(after);
        assert_eq!(next("0 9 * * 1-5", base()), Some(moment(2026, 11, 23, 9, 0)));
        assert_eq!(next("*/15 * * * *", base()), Some(moment(2026, 11, 21, 10, 30)));
        assert_eq!(next("30 10 * * *", base()), Some(moment(2026, 11, 21, 10, 30)));
        assert_eq!(next("0 0 1 * *", base()), Some(moment(2026, 12, 1, 0, 0)));
        assert_eq!(next("0 12 * dec sun", base()), Some(moment(2026, 12, 6, 12, 0)));
        assert_eq!(next("0 0 30 2 *", base()), None);
        // Day of month or weekday, as cron has it: the 1st, or any Monday.
        assert_eq!(next("0 9 1 * mon", base()), Some(moment(2026, 11, 23, 9, 0)));
        assert!(Cron::parse("61 * * * *").is_err());
        assert!(Cron::parse("* * *").is_err());
    }

    #[test]
    fn parses_what_people_write() {
        let now = base();
        assert_eq!(parse("in 30m", now), Ok(Schedule::Once { at: now + 1800.0 }));
        assert_eq!(parse("in 1h30m", now), Ok(Schedule::Once { at: now + 5400.0 }));
        assert_eq!(parse("tomorrow 9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("tomorrow at 9am", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("2026-12-01 14:30", now), Ok(Schedule::Once { at: moment(2026, 12, 1, 14, 30) }));
        assert_eq!(parse("2026-12-01T14:30:00", now), Ok(Schedule::Once { at: moment(2026, 12, 1, 14, 30) }));
        assert_eq!(parse("once dec 3 8:00", now), Ok(Schedule::Once { at: moment(2026, 12, 3, 8, 0) }));
        assert_eq!(parse("9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }));
        assert_eq!(parse("monday 9:00", now), Ok(Schedule::Once { at: moment(2026, 11, 23, 9, 0) }));
        assert_eq!(parse("every 15m", now), Ok(Schedule::Every { minutes: 15, from: start(now, 15) }));
        assert_eq!(parse("every 2 hours", now), Ok(Schedule::Every { minutes: 120, from: start(now, 120) }));
        assert_eq!(parse("every hour", now), Ok(Schedule::Every { minutes: 60, from: start(now, 60) }));
        assert_eq!(parse("hourly", now), Ok(Schedule::Every { minutes: 60, from: start(now, 60) }));
        assert_eq!(parse("daily 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![] }));
        assert_eq!(parse("every day at 18:30", now), Ok(Schedule::Daily { hour: 18, minute: 30, weekdays: vec![] }));
        assert_eq!(parse("weekdays 8:30", now), Ok(Schedule::Daily { hour: 8, minute: 30, weekdays: vec![1, 2, 3, 4, 5] }));
        assert_eq!(parse("weekends 10am", now), Ok(Schedule::Daily { hour: 10, minute: 0, weekdays: vec![0, 6] }));
        assert_eq!(parse("mon,thu 18:00", now), Ok(Schedule::Daily { hour: 18, minute: 0, weekdays: vec![1, 4] }));
        assert_eq!(parse("every friday at 5pm", now), Ok(Schedule::Daily { hour: 17, minute: 0, weekdays: vec![5] }));
        assert_eq!(parse("weekly mon 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1] }));
        assert_eq!(parse("cron 0 9 * * 1-5", now), Ok(Schedule::Cron { expr: "0 9 * * 1-5".into() }));
        assert_eq!(parse("0 9 * * 1-5", now), Ok(Schedule::Cron { expr: "0 9 * * 1-5".into() }));
        assert_eq!(parse("mondays 9:00", now), Ok(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1] }));
        assert!(parse("whenever", now).is_err());
        assert!(parse("every", now).is_err());
        assert!(parse("25:00", now).is_err());
    }

    #[test]
    fn says_it_back() {
        let now = base();
        assert_eq!(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 2, 3, 4, 5] }.describe(now), "Weekdays at 9:00");
        assert_eq!(Schedule::Daily { hour: 9, minute: 5, weekdays: vec![5] }.describe(now), "Fridays at 9:05");
        assert_eq!(Schedule::Daily { hour: 9, minute: 0, weekdays: vec![1, 3] }.describe(now), "Mon, Wed at 9:00");
        assert_eq!(Schedule::Every { minutes: 120, from: 0.0 }.describe(now), "Every 2 hours");
        assert_eq!(Schedule::Once { at: moment(2026, 11, 22, 9, 0) }.describe(now), "Once, tomorrow at 9:00");
        assert_eq!(when(moment(2026, 11, 24, 9, 0), now), "Tue, Nov 24 at 9:00");
    }

    #[test]
    fn runs_move_the_schedule_on() {
        let now = base();
        let mut a = Automation::new("P", "Check", "Look", Schedule::Every { minutes: 60, from: now + 60.0 }, now).unwrap();
        assert!(!a.due(now));
        assert!(a.due(now + 60.0));
        // Missed for a day: runs once, caught up, next from now.
        let late = now + 86_400.0;
        a.ran(late, Some("T".into()), None, false);
        assert!(a.runs[0].caught_up);
        assert!(a.next_run.unwrap() > late && !a.due(late));
        // Run Now leaves the schedule as it was.
        let next = a.next_run;
        a.ran(late + 5.0, Some("T2".into()), None, true);
        assert_eq!(a.next_run, next);
        // Once: done after its run.
        let mut once = Automation::new("P", "Once", "Look", Schedule::Once { at: now + 60.0 }, now).unwrap();
        once.ran(now + 61.0, None, None, false);
        assert!(!once.enabled && once.next_run.is_none());
        assert!(Automation::new("P", "Past", "Look", Schedule::Once { at: now - 60.0 }, now).is_err());
        // Same thread: the first run's thread is kept.
        let mut same = Automation::new("P", "Log", "Look", Schedule::Every { minutes: 5, from: now }, now).unwrap();
        same.same_thread = true;
        same.ran(now + 1.0, Some("T9".into()), None, false);
        assert_eq!(same.thread_id.as_deref(), Some("T9"));
    }

    struct Fake(Mutex<Option<Value>>);
    impl Settings for Fake {
        fn get(&self) -> Option<Value> {
            self.0.lock().unwrap().clone()
        }
        fn set(&self, value: &Value) {
            *self.0.lock().unwrap() = Some(value.clone());
        }
    }

    #[test]
    fn keeps_what_it_cannot_read() {
        let store = Fake(Mutex::new(Some(serde_json::json!([{ "id": "future", "schedule": { "kind": "onTheMoon" } }]))));
        let now = base();
        edit(&store, |list| list.push(Automation::new("P", "A", "Do it", Schedule::Every { minutes: 10, from: now }, now).unwrap()));
        let saved = store.get().unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 2);
        assert_eq!(all(&store).len(), 1);
        assert_eq!(find(&all(&store), "P", "a").unwrap().name, "A");
        assert!(find(&all(&store), "Q", "A").is_err());
    }
}
