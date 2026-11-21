//! Generates a realistic stress dataset (`harness.sqlite` + repo files) for benchmarking
//! huge threads AND huge prompts:
//! - 3 projects (`Home`, `Alpha`, `Beta`); `Home` has 1,500 source files so `Linker` resolves real paths
//! - 20 "Huge NN" threads:
//!   - Huge 01: ~2,500 rows with giant user prompts (20KB, 60KB, 150KB, 250KB), giant markdown replies
//!     (40KB-120KB with long code blocks, tables, lists, file links), dense 200-tool turns, big diffs, sub-agents
//!   - Huge 02..04: ~16,000 rows each (many compactions, dense tool turns, sub-agents, huge prompts in tail)
//!   - Huge 05..20: ~3,000 rows each (for 20-way parallel streaming)
//!   - Huge 02 also starts with a 150KB saved draft
//! - 100 ordinary threads filling the sidebar
//!
//! Usage: cargo run --release --example make_stress_dataset -p yh-core -- <out_dir>

use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use yh_core::db::{Database, Row, StateRows, ThreadRecord};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() as usize) % n }
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn uuid(&mut self) -> String {
        let a = self.next();
        let b = self.next();
        format!(
            "{:08X}-{:04X}-4{:03X}-{:04X}-{:012X}",
            (a >> 32) as u32,
            ((a >> 16) & 0xFFFF) as u16,
            (a & 0x0FFF) as u16,
            (0x8000 | ((b >> 48) & 0x3FFF)) as u16,
            b & 0xFFFF_FFFF_FFFF
        )
    }
}

const WORDS: &[&str] = &[
    "parser", "migration", "index", "thread", "session", "memory", "window", "sidebar",
    "transcript", "render", "layout", "switch", "commit", "branch", "release", "build",
    "test", "flaky", "timezone", "cache", "budget", "token", "query", "search",
    "worktree", "review", "merge", "stack", "diagram", "composer", "scroll", "frame",
];

fn sentence(rng: &mut Rng, n: usize) -> String {
    let mut s = String::with_capacity(n * 9);
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        let w = WORDS[rng.below(WORDS.len())];
        if i == 0 {
            let mut ch = w.chars();
            if let Some(c) = ch.next() {
                s.extend(c.to_uppercase());
                s.push_str(ch.as_str());
            }
        } else {
            s.push_str(w);
        }
    }
    s.push('.');
    s
}

fn huge_user_prompt(rng: &mut Rng, target_bytes: usize, label: &str) -> String {
    let mut s = String::with_capacity(target_bytes + 512);
    s.push_str(&format!("Please analyze this {label} and fix every issue:\n\n"));
    let mut line = 0usize;
    while s.len() < target_bytes {
        let m = line % 15;
        let sub = line % 30;
        let f = line % 1500;
        if line % 25 == 0 {
            s.push_str(&format!(
                "ERROR [{line:05}] in crates/mod_{m}/src/sub_{sub}/file_{f}.rs:{}: migration failed for timezone parser (see https://github.com/acme/repo/issues/{} #{})\n",
                (line % 400) + 1,
                (line % 500) + 1,
                (line % 200) + 1,
            ));
        } else if line % 25 == 12 {
            s.push_str(&format!(
                "  at crates/mod_{m}/src/sub_{sub}/file_{f}.rs:{}:14 — {}\n",
                (line % 300) + 1,
                sentence(rng, 16),
            ));
        } else {
            s.push_str(&format!("Line {line:05}: {}\n", sentence(rng, 14)));
        }
        line += 1;
    }
    s
}

fn huge_markdown_reply(rng: &mut Rng, sections: usize) -> String {
    let mut s = String::with_capacity(sections * 2500);
    for i in 0..sections {
        let f = (i * 17) % 1500;
        let m = f % 15;
        let sub = f % 30;
        s.push_str(&format!("## Section {i}: Findings in `crates/mod_{m}/src/sub_{sub}/file_{f}.rs`\n\n"));
        s.push_str(&format!(
            "Checked **crates/mod_{m}/src/sub_{sub}/file_{f}.rs:{}** and https://github.com/acme/repo/pull/{} #{}. {}\n\n",
            (i * 13) % 300 + 1,
            i + 10,
            i + 10,
            sentence(rng, 24),
        ));
        s.push_str(&format!(
            "- `crates/mod_{m}/src/sub_{sub}/file_{f}.rs:12`: {}\n- `crates/mod_{}/src/sub_{}/file_{}.rs:48`: *{}*\n- Regular note: {}\n\n",
            sentence(rng, 12),
            (f + 1) % 15,
            (f + 1) % 30,
            (f + 1) % 1500,
            sentence(rng, 12),
            sentence(rng, 15),
        ));
        if i % 2 == 0 {
            s.push_str("```rust\n");
            for j in 0..45 {
                s.push_str(&format!(
                    "pub fn handle_step_{i}_{j}(input: &str, count: usize) -> Result<String, &'static str> {{\n    // parser migration cache check for timezone\n    let msg = \"step {i}:{j} value\";\n    if count > {j} {{ return Ok(format!(\"{{input}} {{msg}}\")); }}\n    Err(\"timeout\")\n}}\n"
                ));
            }
            s.push_str("```\n\n");
        }
        if i % 3 == 0 {
            s.push_str("| Component | Before | After | File |\n|---|---|---|---|\n");
            for j in 0..12 {
                let tf = (f + j) % 1500;
                s.push_str(&format!(
                    "| {} | {} ms | {} ms | `crates/mod_{}/src/sub_{}/file_{tf}.rs:{}` |\n",
                    WORDS[(i + j) % WORDS.len()],
                    (j + 1) * 14,
                    (j + 1) * 3,
                    tf % 15,
                    tf % 30,
                    j * 10 + 1,
                ));
            }
            s.push('\n');
        }
    }
    s
}

fn short_answer(rng: &mut Rng) -> String {
    let mut parts = vec![sentence(rng, 22), sentence(rng, 18)];
    if rng.chance(35) {
        let mut code = String::from("```rust\n");
        for i in 0..rng.range(6, 24) {
            code.push_str(&format!("let {}_{i} = {}; // migration check\n", WORDS[rng.below(WORDS.len())], i));
        }
        code.push_str("```");
        parts.push(code);
    }
    if rng.chance(25) {
        let mut tbl = String::from("| Part | Before | After |\n|---|---|---|\n");
        for i in 0..5 {
            tbl.push_str(&format!("| {} | {} ms | {} ms |\n", WORDS[rng.below(WORDS.len())], i * 10 + 5, i * 2 + 1));
        }
        parts.push(tbl);
    }
    parts.join("\n\n")
}

fn make_tool_row(rng: &mut Rng, root: &str, date: f64, big_chance: usize) -> Row {
    let tools = ["Bash", "Read", "Read", "Grep", "Edit", "Write"];
    let name = tools[rng.below(tools.len())];
    let f = rng.below(1500);
    let path = format!("{root}/crates/mod_{}/src/sub_{}/file_{f}.rs", f % 15, f % 30);
    let input = match name {
        "Edit" => json!({
            "file_path": path,
            "old_string": (0..12).map(|i| format!("let old_line_{i} = {};", i)).collect::<Vec<_>>().join("\n"),
            "new_string": (0..14).map(|i| format!("let new_line_{i} = {}; // updated", i)).collect::<Vec<_>>().join("\n"),
        }),
        "Write" => json!({
            "file_path": path,
            "content": (0..60).map(|i| format!("pub fn line_{i}() -> usize {{ {i} }}")).collect::<Vec<_>>().join("\n"),
        }),
        "Bash" => json!({
            "command": format!("cargo test --release {}", WORDS[rng.below(WORDS.len())]),
            "description": sentence(rng, 5),
        }),
        _ => json!({ "file_path": path }),
    };
    let result_len = if rng.chance(big_chance) {
        rng.range(60_000, 220_000)
    } else if rng.chance(15) {
        rng.range(6_000, 24_000)
    } else {
        rng.range(80, 1_800)
    };
    let line = format!("test {} ... ok (migration timezone parser)\n", WORDS[rng.below(WORDS.len())]);
    let repeats = (result_len / line.len()).max(1);
    let mut result = String::with_capacity(repeats * line.len());
    for _ in 0..repeats {
        result.push_str(&line);
    }
    Row {
        id: format!("toolu_{}", rng.uuid()),
        kind: "tool".into(),
        text: String::new(),
        tool_name: Some(name.into()),
        tool_input: Some(input.to_string()),
        tool_result: Some(result),
        tool_is_error: Some(false),
        date: Some(date),
        ..Default::default()
    }
}

fn make_subagent_row(rng: &mut Rng, date: f64) -> Row {
    let id = format!("toolu_{}", rng.uuid());
    let input = json!({
        "description": format!("Audit {} module", WORDS[rng.below(WORDS.len())]),
        "subagent_type": "Explore",
        "prompt": sentence(rng, 20),
    });
    let entries: Vec<_> = (0..12)
        .map(|i| json!({
            "id": format!("s{i}"),
            "kind": "tool",
            "text": "",
            "toolName": "Read",
            "toolInput": json!({"file_path": format!("crates/mod_{}/src/sub_{}/file_{i}.rs", i % 15, i % 30)}).to_string(),
            "toolResult": sentence(rng, 14),
            "toolIsError": false,
        }))
        .collect();
    let sub = json!({
        "description": "Audit module",
        "agentType": "Explore",
        "prompt": sentence(rng, 18),
        "isBackground": false,
        "status": "completed",
        "toolUses": 12,
        "tokens": 42000,
        "durationMs": 18500,
        "report": short_answer(rng),
        "startedAt": date,
        "entries": entries,
    });
    Row {
        id,
        kind: "tool".into(),
        text: String::new(),
        tool_name: Some("Agent".into()),
        tool_input: Some(input.to_string()),
        tool_result: Some(short_answer(rng)),
        tool_is_error: Some(false),
        date: Some(date),
        subagent: Some(sub.to_string().into_bytes()),
        ..Default::default()
    }
}

fn create_dummy_repo(root: &Path, count: usize) {
    for i in 0..count {
        let dir = root.join(format!("crates/mod_{}/src/sub_{}", i % 15, i % 30));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("file_{i}.rs"));
        if !file.exists() {
            let _ = std::fs::write(&file, format!("// file {i}\npub fn f_{i}() -> usize {{ {i} }}\n"));
        }
    }
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: make_stress_dataset <dir>"));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(out.join("home")).unwrap();
    std::fs::create_dir_all(out.join("alpha")).unwrap();
    std::fs::create_dir_all(out.join("beta")).unwrap();

    // Populate Home with 1,500 realistic source files so Linker is actually active!
    let home_dir = out.join("home");
    create_dummy_repo(&home_dir, 1500);
    let home_str = home_dir.to_string_lossy().into_owned();

    let mut rng = Rng(0x5EED_CAFE_1234_5678);
    let db = Database::open_with(&out, None).expect("open db");

    let projects = [
        (rng.uuid(), "Home", home_str.clone()),
        (rng.uuid(), "Alpha", out.join("alpha").to_string_lossy().into_owned()),
        (rng.uuid(), "Beta", out.join("beta").to_string_lossy().into_owned()),
    ];

    let base_ts = 1_793_000_000.0;
    let mut thread_records = Vec::new();
    let mut total_rows = 0usize;

    // 20 Huge threads in Home
    for idx in 0..20 {
        let tid = rng.uuid();
        let title = format!("Huge {:02}", idx + 1);
        let sid = rng.uuid();
        let created = base_ts + idx as f64 * 60.0;
        let updated = base_ts + 10_000.0 - idx as f64 * 10.0;
        let draft = if idx == 1 {
            Some(huge_user_prompt(&mut rng, 150_000, "saved 150KB draft"))
        } else {
            None
        };
        let mut obj = json!({
            "id": tid,
            "projectID": projects[0].0,
            "title": title,
            "createdAt": yh_core::library::iso(created as i64),
            "updatedAt": yh_core::library::iso(updated as i64),
            "model": "haiku",
            "sessionID": sid,
        });
        if let Some(d) = draft {
            obj["draft"] = json!(d);
        }
        thread_records.push(ThreadRecord {
            id: tid.clone(),
            project_id: projects[0].0.clone(),
            updated_at: Some(updated),
            session_id: Some(sid),
            json: serde_json::to_vec(&obj).unwrap(),
        });

        let mut rows: Vec<(usize, Row)> = Vec::new();
        let mut date = created;

        if idx == 0 {
            // Huge 01: ~2,200 rows + tail specifically packed with huge prompts, huge replies, dense work turns, sub-agents!
            for turn in 0..18 {
                date += 60.0;
                let prompt = if turn % 4 == 0 {
                    huge_user_prompt(&mut rng, 45_000, &format!("turn {turn} log"))
                } else {
                    sentence(&mut rng, 30)
                };
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "user".into(),
                    text: prompt,
                    date: Some(date),
                    turn_ended_at: Some(date + 45.0),
                    ..Default::default()
                }));
                for _ in 0..90 {
                    if rng.chance(40) {
                        rows.push((rows.len(), Row {
                            id: rng.uuid(),
                            kind: "thinking".into(),
                            text: sentence(&mut rng, 45),
                            date: Some(date + 5.0),
                            ..Default::default()
                        }));
                    }
                    rows.push((rows.len(), make_tool_row(&mut rng, &home_str, date + 10.0, 2)));
                }
                if turn % 3 == 0 {
                    rows.push((rows.len(), make_subagent_row(&mut rng, date + 20.0)));
                }
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "assistant".into(),
                    text: if turn % 3 == 0 { huge_markdown_reply(&mut rng, 8) } else { short_answer(&mut rng) },
                    date: Some(date + 45.0),
                    ..Default::default()
                }));
            }
            // Tail (newest ~160 rows): explicitly has 25KB, 70KB, 160KB, 240KB user prompts + 90KB markdown replies + a dense 120-tool turn!
            for (sz, label) in [(25_000, "25KB bug report"), (70_000, "70KB stack trace"), (160_000, "160KB build log"), (240_000, "240KB monolith prompt")] {
                date += 120.0;
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "user".into(),
                    text: huge_user_prompt(&mut rng, sz, label),
                    date: Some(date),
                    turn_ended_at: Some(date + 90.0),
                    ..Default::default()
                }));
                for s in 0..24 {
                    if s % 4 == 0 {
                        rows.push((rows.len(), Row {
                            id: rng.uuid(),
                            kind: "thinking".into(),
                            text: (0..8).map(|_| sentence(&mut rng, 25)).collect::<Vec<_>>().join("\n\n"),
                            date: Some(date + 10.0),
                            ..Default::default()
                        }));
                    }
                    rows.push((rows.len(), make_tool_row(&mut rng, &home_str, date + 20.0, 10)));
                }
                rows.push((rows.len(), make_subagent_row(&mut rng, date + 40.0)));
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "assistant".into(),
                    text: huge_markdown_reply(&mut rng, 12), // ~70-90 KB rich markdown
                    date: Some(date + 90.0),
                    ..Default::default()
                }));
            }
        } else {
            // Huge 02..04: ~14,000 rows; Huge 05..20: ~2,500 rows
            let turns = if idx < 4 { 75 } else { 15 };
            for turn in 0..turns {
                date += 60.0;
                let prompt = if turn + 2 >= turns {
                    huge_user_prompt(&mut rng, 65_000, "tail 65KB prompt")
                } else {
                    let n = rng.range(10, 45);
                    sentence(&mut rng, n)
                };
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "user".into(),
                    text: prompt,
                    date: Some(date),
                    turn_ended_at: Some(date + 45.0),
                    ..Default::default()
                }));
                let steps = if idx < 4 { 120 } else { 90 };
                for s in 0..steps {
                    if s % 3 == 0 {
                        rows.push((rows.len(), Row {
                            id: rng.uuid(),
                            kind: "thinking".into(),
                            text: sentence(&mut rng, 35),
                            date: Some(date + 5.0),
                            ..Default::default()
                        }));
                    }
                    rows.push((rows.len(), make_tool_row(&mut rng, &home_str, date + 10.0, 1)));
                    if s % 35 == 20 {
                        rows.push((rows.len(), Row {
                            id: rng.uuid(),
                            kind: "assistant".into(),
                            text: short_answer(&mut rng),
                            date: Some(date + 15.0),
                            ..Default::default()
                        }));
                    }
                }
                if turn % 5 == 0 {
                    rows.push((rows.len(), make_subagent_row(&mut rng, date + 25.0)));
                }
                rows.push((rows.len(), Row {
                    id: rng.uuid(),
                    kind: "assistant".into(),
                    text: if turn + 2 >= turns { huge_markdown_reply(&mut rng, 8) } else { short_answer(&mut rng) },
                    date: Some(date + 45.0),
                    ..Default::default()
                }));
                if turn % 6 == 5 {
                    rows.push((rows.len(), Row {
                        id: rng.uuid(),
                        kind: "notice".into(),
                        text: "Conversation compacted automatically · 192k → 18k tokens".into(),
                        date: Some(date + 50.0),
                        ..Default::default()
                    }));
                }
            }
        }
        let n = rows.len();
        total_rows += n;
        db.save_rows(&tid, n, rows);
        db.flush();
    }

    // 100 ordinary threads
    for idx in 0..100 {
        let tid = rng.uuid();
        let proj = &projects[idx % projects.len()];
        let created = base_ts - 500_000.0 + idx as f64 * 60.0;
        let obj = json!({
            "id": tid,
            "projectID": proj.0,
            "title": sentence(&mut rng, 4).trim_end_matches('.'),
            "createdAt": yh_core::library::iso(created as i64),
            "updatedAt": yh_core::library::iso(created as i64),
            "model": "haiku",
        });
        thread_records.push(ThreadRecord {
            id: tid.clone(),
            project_id: proj.0.clone(),
            updated_at: Some(created),
            session_id: None,
            json: serde_json::to_vec(&obj).unwrap(),
        });
        let rows = vec![
            (0, Row { id: rng.uuid(), kind: "user".into(), text: sentence(&mut rng, 15), date: Some(created), ..Default::default() }),
            (1, make_tool_row(&mut rng, &home_str, created + 5.0, 0)),
            (2, Row { id: rng.uuid(), kind: "assistant".into(), text: short_answer(&mut rng), date: Some(created + 10.0), ..Default::default() }),
        ];
        db.save_rows(&tid, 3, rows);
    }

    let mut values = BTreeMap::new();
    values.insert("selectedThreadID".into(), serde_json::to_vec(&thread_records[0].id).unwrap());
    values.insert("homeProjectID".into(), serde_json::to_vec(&projects[0].0).unwrap());
    values.insert("defaults".into(), serde_json::to_vec(&json!({"model": "haiku", "effort": "low"})).unwrap());
    values.insert("titleGeneration".into(), serde_json::to_vec(&json!({"provider": "claude", "model": "haiku"})).unwrap());
    values.insert("autoSettleDays".into(), serde_json::to_vec(&json!(0)).unwrap());
    values.insert("browserAutoShow".into(), serde_json::to_vec(&json!(false)).unwrap());

    let state = StateRows {
        values,
        projects: projects
            .iter()
            .map(|(id, name, path)| (id.clone(), serde_json::to_vec(&json!({"id": id, "name": name, "path": path, "permissionMode": "auto"})).unwrap()))
            .collect(),
        threads: thread_records,
    };
    db.save_state(state);
    db.flush();
    println!("Generated stress dataset at {}: 120 threads (20 huge, {total_rows} rows) + 1,500 repo files", out.display());
}
