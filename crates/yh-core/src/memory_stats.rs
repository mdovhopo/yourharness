//! How a workspace's memory is doing: how much there is, how old, how linked, and whether
//! agents actually use it. What the Insights tab draws and the reconciler starts from (its
//! `stats` tool). The Swift app's `MemoryStats`, ported.

use crate::memory::{Memory, Store};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub bytes: usize,
    pub recall_count: i64,
    pub created_at: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Day {
    /// Days since 1970, local.
    pub day: i64,
    pub created: usize,
    pub updated: usize,
    pub recalled: usize,
    pub searches: usize,
    pub search_hits: usize,
    pub sessions: usize,
    /// Active memories at the end of the day.
    pub total: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub active: usize,
    pub proposed: usize,
    pub archived: usize,
    pub bytes: usize,
    pub largest: Vec<Summary>,
    pub by_kind: Vec<(String, usize)>,
    pub by_origin: Vec<(String, usize)>,
    pub pinned: usize,
    pub tasks: Vec<(String, usize)>,
    pub links: usize,
    pub orphans: usize,
    pub recalls: i64,
    pub recalls_in_window: usize,
    pub recalled_in_window: usize,
    pub top_recalled: Vec<Summary>,
    pub never_recalled: Vec<Summary>,
    pub searches: usize,
    pub searches_with_hits: usize,
    pub missed_queries: Vec<String>,
    pub sessions: usize,
    pub average_injected_bytes: usize,
    pub age_buckets: Vec<(&'static str, usize)>,
    pub days: Vec<Day>,
    pub likely_duplicates: Vec<Vec<Summary>>,
    pub window: usize,
}

impl Stats {
    pub fn average_bytes(&self) -> usize {
        if self.active == 0 { 0 } else { self.bytes / self.active }
    }
    /// A rough token count: about four bytes a token for English prose.
    pub fn tokens(bytes: usize) -> usize {
        bytes.div_ceil(4)
    }
    pub fn search_hit_rate(&self) -> Option<f64> {
        (self.searches > 0).then(|| self.searches_with_hits as f64 / self.searches as f64)
    }
    pub fn recall_rate(&self) -> Option<f64> {
        (self.active > 0).then(|| self.recalled_in_window as f64 / self.active as f64)
    }
}

struct Event {
    kind: String,
    memory_id: Option<String>,
    at: f64,
    query: Option<String>,
    count: Option<i64>,
    detail: Option<String>,
}

fn events_since(store: &Store, scope: &str, since: f64) -> Vec<Event> {
    let scope = scope.to_string();
    store
        .db
        .read_with(|c| {
            let mut s = c.prepare("SELECT kind, memory_id, at, query, count, detail FROM memory_events WHERE scope = ? AND at >= ? ORDER BY at")?;
            let rows = s.query_map(rusqlite::params![scope, since], |r| Ok(Event { kind: r.get(0)?, memory_id: r.get(1)?, at: r.get(2)?, query: r.get(3)?, count: r.get(4)?, detail: r.get(5)? }))?;
            rows.collect()
        })
        .unwrap_or_default()
}

fn summary(m: &Memory) -> Summary {
    Summary { id: m.id.clone(), title: m.title.clone(), kind: m.kind.clone(), bytes: m.size(), recall_count: m.recall_count, created_at: m.created_at }
}

fn sorted_counts(map: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Counts for `scope` as of now, over the last `window` days.
pub fn compute(store: &Store, scope: &str, window: usize) -> Stats {
    let now = crate::library::now_ms() / 1000.;
    let memories = store.memories(Some(scope), None);
    let events = events_since(store, scope, now - window as f64 * 86_400.);
    let mut s = Stats { window, ..Default::default() };
    let active: Vec<&Memory> = memories.iter().filter(|m| m.status == "active").collect();
    s.active = active.len();
    s.proposed = memories.iter().filter(|m| m.status == "proposed").count();
    s.archived = memories.iter().filter(|m| m.status == "archived").count();
    s.bytes = active.iter().map(|m| m.size()).sum();
    s.pinned = active.iter().filter(|m| m.pinned).count();
    let mut tasks: HashMap<String, usize> = HashMap::new();
    let mut by_kind: HashMap<String, usize> = HashMap::new();
    let mut by_origin: HashMap<String, usize> = HashMap::new();
    for m in &active {
        if m.is_task() {
            *tasks.entry(m.task_state.clone().unwrap_or_else(|| "todo".into())).or_default() += 1;
        }
        *by_kind.entry(m.kind.clone()).or_default() += 1;
        *by_origin.entry(m.origin.clone().unwrap_or_else(|| "agent".into())).or_default() += 1;
    }
    s.tasks = sorted_counts(tasks);
    s.by_kind = sorted_counts(by_kind);
    s.by_origin = sorted_counts(by_origin);
    let mut largest = active.clone();
    largest.sort_by(|a, b| b.size().cmp(&a.size()));
    s.largest = largest.iter().take(5).map(|m| summary(m)).collect();
    let active_ids: HashSet<&str> = active.iter().map(|m| m.id.as_str()).collect();
    let mut linked: HashSet<String> = HashSet::new();
    for m in &active {
        for (to, _) in &m.links {
            if active_ids.contains(to.as_str()) {
                s.links += 1;
                linked.insert(m.id.clone());
                linked.insert(to.clone());
            }
        }
    }
    s.orphans = active.iter().filter(|m| !linked.contains(&m.id)).count();
    s.recalls = memories.iter().map(|m| m.recall_count).sum();
    let mut top: Vec<&&Memory> = active.iter().filter(|m| m.recall_count > 0).collect();
    top.sort_by(|a, b| b.recall_count.cmp(&a.recall_count).then(b.last_recalled_at.unwrap_or(0.).total_cmp(&a.last_recalled_at.unwrap_or(0.))));
    s.top_recalled = top.iter().take(8).map(|m| summary(m)).collect();
    let mut never: Vec<&&Memory> = active.iter().filter(|m| m.recall_count == 0 && m.created_at < now - 7. * 86_400. && !m.pinned && !m.is_task()).collect();
    never.sort_by(|a, b| a.created_at.total_cmp(&b.created_at));
    s.never_recalled = never.iter().map(|m| summary(m)).collect();
    let mut recalled_ids: HashSet<String> = HashSet::new();
    let mut injected = 0usize;
    for e in &events {
        match e.kind.as_str() {
            "recalled" => {
                s.recalls_in_window += 1;
                if let Some(id) = &e.memory_id {
                    recalled_ids.insert(id.clone());
                }
            }
            "searched" => {
                s.searches += 1;
                if e.count.unwrap_or(0) > 0 {
                    s.searches_with_hits += 1;
                } else if let Some(q) = &e.query {
                    if !s.missed_queries.contains(q) {
                        s.missed_queries.push(q.clone());
                    }
                }
            }
            "injected" => {
                s.sessions += 1;
                injected += e.detail.as_deref().and_then(|d| d.parse::<usize>().ok()).unwrap_or(0);
            }
            _ => {}
        }
    }
    s.recalled_in_window = recalled_ids.iter().filter(|id| active_ids.contains(id.as_str())).count();
    s.missed_queries.truncate(12);
    s.average_injected_bytes = if s.sessions == 0 { 0 } else { injected / s.sessions };
    let ages: [(&str, f64); 5] = [("Today", 86_400.), ("This week", 7. * 86_400.), ("This month", 30. * 86_400.), ("1–3 months", 91. * 86_400.), ("Older", f64::INFINITY)];
    let mut counts = [0usize; 5];
    for m in &active {
        let age = now - m.created_at;
        counts[ages.iter().position(|(_, limit)| age < *limit).unwrap_or(4)] += 1;
    }
    s.age_buckets = ages.iter().zip(counts).map(|((l, _), c)| (*l, c)).collect();
    // Days: events by local day, and what was active at each day's end.
    let today = crate::usage::local_day(now);
    let first = today - window as i64 + 1;
    let mut days: Vec<Day> = (first..=today).map(|d| Day { day: d, ..Default::default() }).collect();
    for e in &events {
        let d = crate::usage::local_day(e.at);
        let Some(day) = days.get_mut((d - first) as usize).filter(|_| d >= first) else { continue };
        match e.kind.as_str() {
            "created" | "imported" => day.created += 1,
            "updated" | "merged" | "progressed" => day.updated += 1,
            "recalled" => day.recalled += 1,
            "searched" => {
                day.searches += 1;
                if e.count.unwrap_or(0) > 0 {
                    day.search_hits += 1;
                }
            }
            "injected" => day.sessions += 1,
            _ => {}
        }
    }
    for day in &mut days {
        // The day's end: the next local midnight.
        let end = day_start(day.day + 1);
        day.total = memories.iter().filter(|m| m.created_at < end && (m.status != "archived" || m.updated_at >= end)).count();
    }
    s.days = days;
    let not_tasks: Vec<&Memory> = active.iter().copied().filter(|m| !m.is_task()).collect();
    s.likely_duplicates = duplicates(&not_tasks).into_iter().map(|g| g.into_iter().map(summary).collect()).collect();
    s
}

/// The start of a local day (days since 1970) in seconds since 1970.
fn day_start(day: i64) -> f64 {
    // Local midnight: a day's UTC start shifted by this time zone's offset then.
    let utc = day as f64 * 86_400.;
    let offset = {
        let secs = utc as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        tm.tm_gmtoff as f64
    };
    utc - offset
}

const STOP: &[&str] = &["the", "and", "for", "with", "use", "uses", "how", "what", "when", "not", "never", "always", "about", "from", "into", "that", "this", "are", "you", "your"];

/// Groups of memories whose titles share most of their words.
pub fn duplicates<'a>(memories: &[&'a Memory]) -> Vec<Vec<&'a Memory>> {
    let words: Vec<HashSet<String>> = memories
        .iter()
        .map(|m| m.title.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 2 && !STOP.contains(w)).map(str::to_string).collect())
        .collect();
    let mut groups = vec![];
    let mut grouped: HashSet<usize> = HashSet::new();
    for i in 0..memories.len() {
        if grouped.contains(&i) || words[i].is_empty() {
            continue;
        }
        let mut group = vec![i];
        for j in i + 1..memories.len() {
            if grouped.contains(&j) || words[j].is_empty() {
                continue;
            }
            let shared = words[i].intersection(&words[j]).count();
            let smaller = words[i].len().min(words[j].len());
            if shared >= 2 || (smaller == 1 && shared == 1 && words[i] == words[j]) || (shared >= 1 && smaller >= 2 && shared as f64 / smaller as f64 >= 0.75) {
                group.push(j);
            }
        }
        if group.len() > 1 {
            grouped.extend(group.iter().copied());
            groups.push(group.into_iter().map(|k| memories[k]).collect());
        }
    }
    groups
}

/// The `stats` tool's text.
pub fn describe(s: &Stats, day: impl Fn(f64) -> String) -> String {
    let short = |id: &str| id.chars().take(8).collect::<String>().to_lowercase();
    let _ = &day;
    let mut lines = vec![
        format!("Active: {}, archived: {}, pinned: {}.", s.active, s.archived, s.pinned),
        format!("Size: {} bytes (about {} tokens), {} on average.", s.bytes, Stats::tokens(s.bytes), s.average_bytes()),
        format!("Kinds: {}.", { let mut k = s.by_kind.clone(); k.sort(); k.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", ") }),
        format!("Links: {}; memories with none: {}.", s.links, s.orphans),
        format!(
            "Last {} days: {} sessions, {} reads of {} memories, {} searches ({} found something).",
            s.window, s.sessions, s.recalls_in_window, s.recalled_in_window, s.searches, s.searches_with_hits
        ),
    ];
    if !s.largest.is_empty() {
        lines.push(format!("Largest: {}", s.largest.iter().map(|m| format!("{} {} ({} B)", short(&m.id), m.title, m.bytes)).collect::<Vec<_>>().join("; ")));
    }
    if !s.never_recalled.is_empty() {
        lines.push(format!("Never read, older than a week: {}", s.never_recalled.iter().take(30).map(|m| format!("{} {}", short(&m.id), m.title)).collect::<Vec<_>>().join("; ")));
    }
    for g in &s.likely_duplicates {
        lines.push(format!("Possible duplicates: {}", g.iter().map(|m| format!("{} {}", short(&m.id), m.title)).collect::<Vec<_>>().join(" | ")));
    }
    if !s.missed_queries.is_empty() {
        lines.push(format!("Searches that found nothing: {}", s.missed_queries.iter().map(|q| format!("\"{q}\"")).collect::<Vec<_>>().join(", ")));
    }
    lines.join("\n")
}

/// The memory as a map: Fruchterman–Reingold in a unit square (linked memories pull
/// together, everything pushes apart, a little gravity keeps the loners in view), seeded by
/// ID so it lays out the same way every time. Points are in 0…1.
pub fn graph_layout(ids: &[String], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = ids.len();
    if n == 0 {
        return vec![];
    }
    let mut pos: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| {
            let mut h: u64 = 1469598103934665603;
            for b in id.bytes() {
                h = (h ^ b as u64).wrapping_mul(1099511628211);
            }
            ((h & 0xFFFF) as f64 / 65535., ((h >> 16) & 0xFFFF) as f64 / 65535.)
        })
        .collect();
    let k = (1.0 / n as f64).sqrt() * 0.9;
    let iterations = if n > 300 { 120 } else { 260 };
    let mut temperature = 0.1;
    for _ in 0..iterations {
        let mut shift = vec![(0.0f64, 0.0f64); n];
        for i in 0..n {
            for j in i + 1..n {
                let (mut dx, mut dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let mut d = (dx * dx + dy * dy).sqrt();
                if d < 1e-4 {
                    dx = 0.001 * (i as f64 - j as f64);
                    dy = 0.001;
                    d = 0.0015;
                }
                let f = k * k / d;
                shift[i].0 += dx / d * f;
                shift[i].1 += dy / d * f;
                shift[j].0 -= dx / d * f;
                shift[j].1 -= dy / d * f;
            }
        }
        for &(a, b) in edges {
            if a == b {
                continue;
            }
            let (dx, dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
            let d = (dx * dx + dy * dy).sqrt().max(1e-4);
            let f = d * d / k;
            shift[a].0 -= dx / d * f;
            shift[a].1 -= dy / d * f;
            shift[b].0 += dx / d * f;
            shift[b].1 += dy / d * f;
        }
        for i in 0..n {
            shift[i].0 += (0.5 - pos[i].0) * 0.6 * k * 10.;
            shift[i].1 += (0.5 - pos[i].1) * 0.6 * k * 10.;
            let len = (shift[i].0 * shift[i].0 + shift[i].1 * shift[i].1).sqrt().max(1e-9);
            let step = len.min(temperature);
            pos[i].0 += shift[i].0 / len * step;
            pos[i].1 += shift[i].1 / len * step;
        }
        temperature *= 0.985;
    }
    // Fitted to the square, keeping proportions.
    let (min_x, max_x) = pos.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_y, max_y) = pos.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let span = (max_x - min_x).max(max_y - min_y).max(1e-6);
    pos.iter()
        .map(|p| if n == 1 { (0.5, 0.5) } else { ((p.0 - min_x) / span + (1. - (max_x - min_x) / span) / 2., (p.1 - min_y) / span + (1. - (max_y - min_y) / span) / 2.) })
        .collect()
}
