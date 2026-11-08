//! What Claude Code spent: the usage it records with every reply in its session files, kept
//! in an index of its own (`usage.sqlite` in the data folder, 0.0.x's schema), so a
//! year is summed in a moment and history stays after Claude Code deletes old sessions.
//! Files are read once, then only from where they were left (Claude Code appends). A reply
//! written more than once (a line per content block, copied into a fork) counts once, by
//! its message and request IDs. Prices are the API's, to put a subscription in dollars.

use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_5m: i64,
    pub cache_1h: i64,
}

impl Tokens {
    pub fn prompt(&self) -> i64 {
        self.input + self.cache_read + self.cache_5m + self.cache_1h
    }
    pub fn total(&self) -> i64 {
        self.prompt() + self.output
    }
    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_5m += o.cache_5m;
        self.cache_1h += o.cache_1h;
    }
    fn max(&mut self, o: &Tokens) {
        self.input = self.input.max(o.input);
        self.output = self.output.max(o.output);
        self.cache_read = self.cache_read.max(o.cache_read);
        self.cache_5m = self.cache_5m.max(o.cache_5m);
        self.cache_1h = self.cache_1h.max(o.cache_1h);
    }
}

#[derive(Clone, Debug)]
pub struct MessageUsage {
    pub key: i64,
    /// Seconds since 1970.
    pub at: f64,
    pub model: String,
    pub tokens: Tokens,
    pub fast: bool,
}

/// FNV-1a: the same on every launch (and as 0.0.x's, which keyed the same index).
pub fn stable_hash(text: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

/// A session file line's usage, if it's an assistant reply with some.
pub fn record(line: &[u8], fallback: &str) -> Option<MessageUsage> {
    let v: Value = serde_json::from_slice(line).ok()?;
    if v["type"].as_str() != Some("assistant") {
        return None;
    }
    let message = &v["message"];
    let model = message["model"].as_str()?;
    // Claude Code writes its own notices as `<synthetic>` replies.
    if model.is_empty() || model.starts_with('<') {
        return None;
    }
    let u = &message["usage"];
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    let write = n("cache_creation_input_tokens");
    let mut tokens = Tokens { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_5m: write, cache_1h: 0 };
    // Without the split by cache lifetime (older CLIs), every write is the 5-minute kind.
    let creation = &u["cache_creation"];
    if creation.is_object() {
        let h = creation["ephemeral_1h_input_tokens"].as_i64().unwrap_or(0).min(write);
        tokens.cache_1h = h;
        tokens.cache_5m = write - h;
    }
    if tokens.total() == 0 {
        return None;
    }
    let at = parse_time(v["timestamp"].as_str()?)?;
    let key = match message["id"].as_str() {
        Some(id) => stable_hash(&format!("{id}:{}", v["requestId"].as_str().unwrap_or(""))),
        None => stable_hash(fallback),
    };
    Some(MessageUsage { key, at, model: model.to_string(), tokens, fast: u["speed"].as_str() == Some("fast") })
}

/// `2026-10-28T09:41:07.123Z` or with an offset like `+02:00`, to seconds since 1970.
pub fn parse_time(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |from: usize, len: usize| -> Option<i64> {
        let mut v = 0;
        for &c in &b[from..from + len] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as i64;
        }
        Some(v)
    };
    let (year, month, day, hour, minute, second) = (num(0, 4)?, num(5, 2)?, num(8, 2)?, num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut i = 19;
    let mut fraction = 0.0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut scale = 0.1;
        while i < b.len() && b[i].is_ascii_digit() {
            fraction += (b[i] - b'0') as f64 * scale;
            scale /= 10.;
            i += 1;
        }
    }
    let mut offset = 0;
    if i + 6 <= b.len() && (b[i] == b'+' || b[i] == b'-') {
        if let (Some(h), Some(m)) = (num(i + 1, 2), num(i + 4, 2)) {
            offset = (h * 3600 + m * 60) * if b[i] == b'-' { -1 } else { 1 };
        }
    }
    Some((days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset) as f64 + fraction)
}

/// Days from 1970-01-01 (Howard Hinnant's days_from_civil).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// MARK: Prices

#[derive(Clone, Copy, Debug)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub read: f64,
}

impl Price {
    pub fn cost(&self, t: &Tokens) -> f64 {
        (t.input as f64 * self.input + t.output as f64 * self.output + t.cache_5m as f64 * self.write_5m + t.cache_1h as f64 * self.write_1h + t.cache_read as f64 * self.read)
            / 1_000_000.
    }
    fn scaled(self, f: f64) -> Price {
        Price { input: self.input * f, output: self.output * f, write_5m: self.write_5m * f, write_1h: self.write_1h * f, read: self.read * f }
    }
}

/// A Haiku 5.5 request whose prompt is longer than this costs more.
pub const LONG_PROMPT: i64 = 100_000;

const fn p(input: f64, output: f64, write_5m: f64, write_1h: f64, read: f64) -> Price {
    Price { input, output, write_5m, write_1h, read }
}

/// Anthropic's API prices per million tokens (checked
/// 2026-10-28 against platform.claude.com/docs/en/about-claude/pricing): (price, long-prompt
/// price, fast-mode multiplier).
fn entry(model: &str) -> Option<(Price, Option<Price>, Option<f64>)> {
    let fable51 = p(10., 50., 12.5, 20., 0.25);
    let fable5 = p(10., 50., 12.5, 20., 1.);
    let opus5 = p(5., 25., 6.25, 10., 0.5);
    let opus41 = p(15., 75., 18.75, 30., 1.5);
    let sonnet4 = p(3., 15., 3.75, 6., 0.3);
    Some(match family(model).as_str() {
        "claude-fable-5-1" | "claude-mythos-5-1" => (fable51, None, None),
        "claude-fable-5" | "claude-mythos-5" => (fable5, None, None),
        "claude-opus-5-5" => (p(4., 20., 5., 8., 0.2), None, Some(2.)),
        "claude-opus-5" | "claude-opus-4-8" => (opus5, None, Some(2.)),
        "claude-opus-4-7" | "claude-opus-4-6" | "claude-opus-4-5" => (opus5, None, None),
        "claude-opus-4-1" | "claude-opus-4" => (opus41, None, None),
        "claude-sonnet-5-5" => (p(2., 10., 2.5, 4., 0.1), None, None),
        "claude-sonnet-5" => (p(2., 10., 2.5, 4., 0.2), None, None),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4" => (sonnet4, None, None),
        "claude-haiku-5-5" => (p(0.1, 0.5, 0.125, 0.2, 0.01), Some(p(0.5, 2.5, 0.625, 1., 0.05)), None),
        "claude-haiku-4-5" => (p(1., 5., 1.25, 2., 0.1), None, None),
        "claude-3-5-haiku" => (p(0.8, 4., 1., 1.6, 0.08), None, None),
        _ => return None,
    })
}

/// A model ID as one family: `claude-opus-4-5-20251101`, `us.anthropic.claude-opus-4-5-20251101-v1:0`,
/// `claude-opus-4-5@20251101` and `claude-opus-4-5[1m]` are all `claude-opus-4-5`.
pub fn family(model: &str) -> String {
    let mut id = model.trim().to_lowercase();
    if let Some(i) = id.find("claude-") {
        id = id[i..].to_string();
    }
    for sep in ['[', '@', ':'] {
        if let Some(i) = id.find(sep) {
            id.truncate(i);
        }
    }
    if id.ends_with("-v1") || id.ends_with("-v2") {
        id.truncate(id.len() - 3);
    }
    if let Some(i) = id.rfind('-') {
        let tail = &id[i + 1..];
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_digit()) {
            id.truncate(i);
        }
    }
    id
}

/// "Opus 5.5", "Haiku 3.5": for people.
pub fn display_name(model: &str) -> String {
    let f = family(model);
    let Some(rest) = f.strip_prefix("claude-") else { return model.to_string() };
    let parts: Vec<&str> = rest.split('-').collect();
    let names: Vec<String> = parts
        .iter()
        .filter(|p| p.chars().next().is_some_and(|c| c.is_alphabetic()))
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    let numbers: Vec<&str> = parts.iter().filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())).copied().collect();
    if names.is_empty() {
        return model.to_string();
    }
    if numbers.is_empty() {
        names.join(" ")
    } else {
        format!("{} {}", names.join(" "), numbers.join("."))
    }
}

// MARK: The index

pub struct UsageStore {
    writer: Mutex<Option<Connection>>,
    path: PathBuf,
}

const BUCKET: &str = "CAST(R.at / 900 AS INTEGER)";

fn long(row: &str) -> String {
    format!("({row}.input + {row}.cache_read + {row}.cache_5m + {row}.cache_1h > {LONG_PROMPT})")
}

fn add_to_bucket(row: &str) -> String {
    format!(
        "INSERT INTO buckets VALUES ({}, {row}.model, {row}.fast, {}, {row}.input, {row}.output, {row}.cache_read, {row}.cache_5m, {row}.cache_1h, 1)
        ON CONFLICT DO UPDATE SET input = input + excluded.input, output = output + excluded.output,
            cache_read = cache_read + excluded.cache_read, cache_5m = cache_5m + excluded.cache_5m,
            cache_1h = cache_1h + excluded.cache_1h, messages = messages + 1;",
        BUCKET.replace("R.", &format!("{row}.")),
        long(row)
    )
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        c.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY, size INTEGER NOT NULL, modified REAL NOT NULL, offset INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS messages (key INTEGER PRIMARY KEY, at REAL NOT NULL, model TEXT NOT NULL, input INTEGER NOT NULL,
                output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_5m INTEGER NOT NULL, cache_1h INTEGER NOT NULL,
                fast INTEGER NOT NULL DEFAULT 0, source INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS messages_at ON messages(at);
            CREATE TABLE IF NOT EXISTS buckets (bucket INTEGER NOT NULL, model TEXT NOT NULL, fast INTEGER NOT NULL, long INTEGER NOT NULL,
                input INTEGER NOT NULL, output INTEGER NOT NULL, cache_read INTEGER NOT NULL, cache_5m INTEGER NOT NULL,
                cache_1h INTEGER NOT NULL, messages INTEGER NOT NULL, PRIMARY KEY (bucket, model, fast, long)) WITHOUT ROWID;
            CREATE TRIGGER IF NOT EXISTS messages_added AFTER INSERT ON messages BEGIN {} END;
            CREATE TRIGGER IF NOT EXISTS messages_changed AFTER UPDATE ON messages BEGIN
                UPDATE buckets SET input = input - OLD.input, output = output - OLD.output, cache_read = cache_read - OLD.cache_read,
                    cache_5m = cache_5m - OLD.cache_5m, cache_1h = cache_1h - OLD.cache_1h, messages = messages - 1
                WHERE bucket = {} AND model = OLD.model AND fast = OLD.fast AND long = {};
                {}
            END;
            PRAGMA user_version = 1;",
            add_to_bucket("NEW"),
            BUCKET.replace("R.", "OLD."),
            long("OLD"),
            add_to_bucket("NEW")
        ))?;
    }
    Ok(c)
}

pub struct Refresh {
    pub files_read: usize,
    pub files_total: usize,
    pub records: usize,
}

impl UsageStore {
    /// `usage.sqlite` in `directory`. An index of other files: if it can't be read, it's
    /// made again.
    pub fn new(directory: &Path) -> UsageStore {
        let _ = std::fs::create_dir_all(directory);
        let path = directory.join("usage.sqlite");
        let mut c = open(&path).ok();
        if c.is_none() {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
            c = open(&path).ok();
        }
        UsageStore { writer: Mutex::new(c), path }
    }

    /// Reads what's new in the session files under `<config>/projects`, newest first, a
    /// batch of files at once on all cores.
    pub fn refresh(&self, config: &Path) -> Refresh {
        let files = session_files(&config.join("projects"));
        let mut guard = self.writer.lock().unwrap();
        let Some(c) = guard.as_mut() else { return Refresh { files_read: 0, files_total: 0, records: 0 } };
        let mut known: HashMap<String, (i64, f64, i64)> = HashMap::new();
        if let Ok(mut s) = c.prepare("SELECT path, size, modified, offset FROM files") {
            if let Ok(rows) = s.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))) {
                known.extend(rows.flatten());
            }
        }
        let changed: Vec<&(PathBuf, i64, f64)> = files
            .iter()
            .filter(|(p, size, modified)| known.get(&p.to_string_lossy().into_owned()).is_none_or(|(s, m, _)| s != size || m != modified))
            .collect();
        let mut result = Refresh { files_read: 0, files_total: changed.len(), records: 0 };
        if changed.is_empty() {
            return result;
        }
        let _ = c.execute_batch("PRAGMA cache_size = -65536");
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
        for batch in changed.chunks((cores * 4).max(16)) {
            let starts: Vec<i64> =
                batch.iter().map(|(p, size, _)| known.get(&p.to_string_lossy().into_owned()).map_or(0, |(_, _, off)| if off <= size { *off } else { 0 })).collect();
            let reads: Vec<(Vec<MessageUsage>, i64)> = std::thread::scope(|s| {
                let handles: Vec<_> = batch.iter().zip(&starts).map(|((p, size, _), start)| s.spawn(move || read_file(p, *start, *size))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
            });
            let mut merged: HashMap<i64, MessageUsage> = HashMap::new();
            for (records, _) in &reads {
                for r in records {
                    merged.entry(r.key).and_modify(|m| m.tokens.max(&r.tokens)).or_insert_with(|| r.clone());
                }
            }
            let Ok(tx) = c.transaction() else { continue };
            let mut ok = true;
            {
                let mut keys: Vec<&i64> = merged.keys().collect();
                keys.sort();
                for k in keys {
                    let r = &merged[k];
                    let t = &r.tokens;
                    ok &= tx
                        .execute(
                            "INSERT INTO messages (key, at, model, input, output, cache_read, cache_5m, cache_1h, fast, source) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
                             ON CONFLICT(key) DO UPDATE SET input = max(input, excluded.input), output = max(output, excluded.output),
                                cache_read = max(cache_read, excluded.cache_read), cache_5m = max(cache_5m, excluded.cache_5m), cache_1h = max(cache_1h, excluded.cache_1h)",
                            params![r.key, r.at, r.model, t.input, t.output, t.cache_read, t.cache_5m, t.cache_1h, r.fast as i64],
                        )
                        .is_ok();
                }
                for (i, (p, size, modified)) in batch.iter().enumerate() {
                    ok &= tx
                        .execute("INSERT OR REPLACE INTO files (path, size, modified, offset) VALUES (?, ?, ?, ?)", params![p.to_string_lossy(), size, modified, starts[i] + reads[i].1])
                        .is_ok();
                }
            }
            // Nothing of a batch that failed is kept; its files are read again next time.
            if ok && tx.commit().is_ok() {
                result.files_read += batch.len();
                result.records += merged.len();
            }
        }
        let _ = c.execute_batch("PRAGMA cache_size = -2000; PRAGMA shrink_memory;");
        result
    }

    /// The last `days` days, today included, in the local time zone.
    pub fn report(&self, days: usize) -> Report {
        let today = local_day(now());
        let first = today - days.max(1) as i64 + 1;
        let mut report = Report { first_day: first, days: vec![Day::default(); days.max(1)], models: vec![], total: Spend::default(), all_models: vec![] };
        let Ok(c) = Connection::open_with_flags(&self.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return report };
        let mut totals: HashMap<String, Spend> = HashMap::new();
        let mut firsts: HashMap<String, (i64, f64)> = HashMap::new();
        let Ok(mut s) = c.prepare("SELECT bucket, model, fast, long, input, output, cache_read, cache_5m, cache_1h, messages FROM buckets WHERE messages > 0") else { return report };
        let rows = s.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? == 1,
                r.get::<_, i64>(3)? == 1,
                Tokens { input: r.get(4)?, output: r.get(5)?, cache_read: r.get(6)?, cache_5m: r.get(7)?, cache_1h: r.get(8)? },
                r.get::<_, i64>(9)?,
            ))
        });
        let mut day_of: HashMap<i64, i64> = HashMap::new();
        for (bucket, model, fast, long, tokens, messages) in rows.into_iter().flatten().flatten() {
            let spend = spend(&model, fast, long, tokens, messages);
            let fam = family(&model);
            let day = *day_of.entry(bucket).or_insert_with(|| local_day(bucket as f64 * 900.));
            let e = firsts.entry(fam.clone()).or_insert((day, 0.));
            e.0 = e.0.min(day);
            e.1 += spend.cost + spend.unpriced as f64 / 1e12;
            if day < first || day > today {
                continue;
            }
            let d = &mut report.days[(day - first) as usize];
            d.models.entry(fam.clone()).or_default().add(&spend);
            d.total.add(&spend);
            totals.entry(fam).or_default().add(&spend);
            report.total.add(&spend);
        }
        report.models = totals.into_iter().collect();
        report.models.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(b.1.tokens.total().cmp(&a.1.tokens.total())).then(a.0.cmp(&b.0)));
        let mut order: Vec<(String, (i64, f64))> = firsts.into_iter().collect();
        order.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.total_cmp(&a.1 .1)).then(a.0.cmp(&b.0)));
        report.all_models = order.into_iter().map(|(m, _)| m).collect();
        report
    }
}

fn spend(model: &str, fast: bool, long: bool, tokens: Tokens, messages: i64) -> Spend {
    let mut s = Spend { tokens, messages, ..Default::default() };
    match entry(model) {
        Some((price, long_price, fast_factor)) => {
            let mut price = if long { long_price.unwrap_or(price) } else { price };
            if let (true, Some(f)) = (fast, fast_factor) {
                price = price.scaled(f);
            }
            s.cost = price.cost(&tokens);
        }
        None => s.unpriced = tokens.total(),
    }
    s
}

#[derive(Clone, Debug, Default)]
pub struct Spend {
    pub tokens: Tokens,
    /// At API prices; tokens of models with no price aren't in it.
    pub cost: f64,
    pub unpriced: i64,
    pub messages: i64,
}

impl Spend {
    fn add(&mut self, o: &Spend) {
        self.tokens.add(&o.tokens);
        self.cost += o.cost;
        self.unpriced += o.unpriced;
        self.messages += o.messages;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Day {
    /// By model family.
    pub models: HashMap<String, Spend>,
    pub total: Spend,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Days since 1970 (local) of the first day.
    pub first_day: i64,
    /// Every day of the range, the oldest first.
    pub days: Vec<Day>,
    /// The models used in the range, the costliest first.
    pub models: Vec<(String, Spend)>,
    pub total: Spend,
    /// Every model ever seen, the newest first: colors follow a model whatever the range.
    pub all_models: Vec<String>,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

/// The local day a moment falls on, as days since 1970.
pub fn local_day(t: f64) -> i64 {
    let secs = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    days_from_civil(tm.tm_year as i64 + 1900, tm.tm_mon as i64 + 1, tm.tm_mday as i64)
}

/// (year, month, day) of days since 1970.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Every `.jsonl` under `projects`, sub-agents' included, the newest first: (path, size, modified).
fn session_files(projects: &Path) -> Vec<(PathBuf, i64, f64)> {
    let mut files = vec![];
    let mut stack = vec![projects.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "jsonl") {
                let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0., |d| d.as_secs_f64());
                files.push((path, meta.len() as i64, modified));
            }
        }
    }
    files.sort_by(|a, b| b.2.total_cmp(&a.2));
    files
}

/// The usage in a file from `offset` to its last complete line, and how far that was.
fn read_file(path: &Path, offset: i64, size: i64) -> (Vec<MessageUsage>, i64) {
    if size <= offset {
        return (vec![], 0);
    }
    let Ok(mut f) = std::fs::File::open(path) else { return (vec![], 0) };
    if f.seek(SeekFrom::Start(offset as u64)).is_err() {
        return (vec![], 0);
    }
    let mut data = Vec::with_capacity((size - offset) as usize);
    if f.take((size - offset) as u64).read_to_end(&mut data).is_err() {
        return (vec![], 0);
    }
    let source = path.to_string_lossy();
    let mut records = vec![];
    let mut start = 0;
    let mut consumed = 0;
    while let Some(nl) = data[start..].iter().position(|b| *b == b'\n') {
        let line = &data[start..start + nl];
        if !line.is_empty() && memmem(line, b"\"usage\"") {
            if let Some(r) = record(line, &format!("{source}:{}", offset + start as i64)) {
                records.push(r);
            }
        }
        start += nl + 1;
        consumed = start;
    }
    (records, consumed as i64)
}

fn memmem(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_and_times() {
        assert_eq!(parse_time("1970-01-02T00:00:00Z"), Some(86_400.));
        assert_eq!(parse_time("1970-01-02T02:00:00.5+02:00"), Some(86_400.5));
        let line = br#"{"type":"assistant","timestamp":"2026-10-28T09:41:07.123Z","requestId":"r","message":{"id":"m","model":"claude-haiku-5-5-20261001","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60}}}}"#;
        let r = record(line, "x").unwrap();
        assert_eq!(r.tokens, Tokens { input: 10, output: 5, cache_read: 0, cache_5m: 40, cache_1h: 60 });
        assert_eq!(r.key, stable_hash("m:r"));
        assert!(record(br#"{"type":"assistant","timestamp":"2026-10-28T09:41:07Z","message":{"model":"<synthetic>","usage":{"output_tokens":1}}}"#, "x").is_none());
    }

    #[test]
    fn names_and_prices() {
        assert_eq!(family("us.anthropic.claude-opus-4-5-20251101-v1:0"), "claude-opus-4-5");
        assert_eq!(family("claude-opus-4-5[1m]"), "claude-opus-4-5");
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-3-5-haiku"), "Haiku 3.5");
        let t = Tokens { input: 1_000_000, ..Default::default() };
        assert_eq!(spend("claude-opus-5-5", true, false, t, 1).cost, 8.);
        assert_eq!(spend("claude-haiku-5-5", false, true, t, 1).cost, 0.5);
        assert_eq!(civil(days_from_civil(2026, 11, 7)), (2026, 11, 7));
    }

    #[test]
    fn indexes_and_counts_once() {
        let dir = std::env::temp_dir().join(format!("yh-usage-test-{}", std::process::id()));
        let projects = dir.join("config/projects/p");
        std::fs::create_dir_all(&projects).unwrap();
        let ts = {
            let d = local_day(now());
            let (y, m, day) = civil(d);
            format!("{y:04}-{m:02}-{day:02}T12:00:00Z")
        };
        let line = format!(r#"{{"type":"assistant","timestamp":"{ts}","requestId":"r","message":{{"id":"m","model":"claude-opus-5-5","usage":{{"input_tokens":1000000,"output_tokens":0}}}}}}"#);
        // The same reply in two files (a fork's copy) counts once.
        std::fs::write(projects.join("a.jsonl"), format!("{line}\n")).unwrap();
        std::fs::write(projects.join("b.jsonl"), format!("{line}\n")).unwrap();
        let store = UsageStore::new(&dir);
        let r = store.refresh(&dir.join("config"));
        assert_eq!(r.files_read, 2);
        let report = store.report(7);
        assert_eq!(report.total.messages, 1);
        assert_eq!(report.total.cost, 4.);
        assert_eq!(store.refresh(&dir.join("config")).files_total, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
