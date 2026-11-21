use std::path::Path;
use std::time::Instant;
use yh_core::claude;
use yh_core::db::Database;
use yh_core::find;
use yh_core::library::{EntryData, Library};
use yh_core::links::{Linker, Repo};
use yh_core::markdown::{self, Styled};

fn bench<F: FnMut()>(name: &str, iters: usize, mut f: F) {
    // Warmup
    f();
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        f();
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let p95 = times[(times.len() * 95) / 100];
    let worst = times[times.len() - 1];
    println!("{name:<48} median={median:7.2} ms  p95={p95:7.2} ms  worst={worst:7.2} ms  (n={iters})");
}

fn make_huge_prompt(lines: usize) -> String {
    let mut s = String::with_capacity(lines * 90);
    for i in 0..lines {
        s.push_str(&format!(
            "Line {i:04}: check crates/yh-app/src/ui/rows.rs:123 and https://example.com/issue/{i} #42 for parser timezone migration\n"
        ));
    }
    s
}

fn make_huge_markdown(sections: usize) -> String {
    let mut s = String::with_capacity(sections * 1500);
    for i in 0..sections {
        s.push_str(&format!("## Section {i}: Investigating `crates/yh-app/src/ui/composer.rs`\n\n"));
        s.push_str("Here is a **detailed** analysis of `src/ui/rows.rs:42` and https://github.com/acme/repo/pull/123 #99. ");
        s.push_str("The parser drops the timezone when the migration runs twice across multiple threads.\n\n");
        s.push_str("- First item mentioning `crates/yh-core/src/markdown.rs` and *emphasis*\n");
        s.push_str("- Second item mentioning `crates/yh-core/src/db.rs:500` and ~~old code~~\n\n");
        s.push_str("```rust\n");
        for j in 0..25 {
            s.push_str(&format!(
                "pub fn process_row_{i}_{j}(input: &str, count: usize) -> Result<String, &'static str> {{\n    // comment about parser and timezone\n    let value = \"hello world {j}\";\n    if count > 100 {{ return Ok(format!(\"{{input}}: {{value}}\")); }}\n    Err(\"failed\")\n}}\n"
            ));
        }
        s.push_str("```\n\n");
        s.push_str("| Column A | Column B | Column C |\n|---|---|---|\n");
        for j in 0..6 {
            s.push_str(&format!("| row_{i}_{j} | `crates/yh-core/src/links.rs` | value {j} |\n"));
        }
        s.push('\n');
    }
    s
}

fn make_files(n: usize) -> Vec<String> {
    let mut files = Vec::with_capacity(n);
    files.push("crates/yh-app/src/ui/rows.rs".into());
    files.push("crates/yh-app/src/ui/composer.rs".into());
    files.push("crates/yh-core/src/markdown.rs".into());
    files.push("crates/yh-core/src/db.rs".into());
    files.push("crates/yh-core/src/links.rs".into());
    for i in 0..n {
        files.push(format!("crates/mod_{}/src/sub_{}/file_{i}.rs", i % 20, i % 50));
    }
    files
}

fn main() {
    println!("=== yh-core micro-benchmarks ===\n");

    let files = make_files(3000);
    let linker = Linker::new(
        "/work/yourharness",
        Some(Repo { host: "github.com".into(), path: "acme/yourharness".into(), gitlab: false }),
        &files,
    );

    let prompt_50k = make_huge_prompt(500); // ~55 KB
    let prompt_200k = make_huge_prompt(2000); // ~220 KB
    let prompt_200k_plain = "The quick brown fox jumps over the lazy dog without any slashes or dots\n".repeat(3000);
    println!("prompt_50k = {} bytes, prompt_200k = {} bytes", prompt_50k.len(), prompt_200k.len());

    bench("Linker::link (55KB prompt w/ paths+urls)", 10, || {
        let mut s = Styled { text: prompt_50k.clone(), spans: vec![], links: vec![] };
        linker.link(&mut s);
        std::hint::black_box(s.links.len());
    });
    bench("Linker::link (220KB prompt w/ paths+urls)", 5, || {
        let mut s = Styled { text: prompt_200k.clone(), spans: vec![], links: vec![] };
        linker.link(&mut s);
        std::hint::black_box(s.links.len());
    });
    bench("Linker::link (220KB plain prose)", 10, || {
        let mut s = Styled { text: prompt_200k_plain.clone(), spans: vec![], links: vec![] };
        linker.link(&mut s);
        std::hint::black_box(s.links.len());
    });

    bench("claude::trailing_command (220KB prompt)", 50, || {
        std::hint::black_box(claude::trailing_command(&prompt_200k));
    });
    let known: std::collections::HashSet<String> = ["review", "compact", "commit", "plan"].into_iter().map(str::to_string).collect();
    bench("claude::chained_commands (220KB prompt)", 20, || {
        std::hint::black_box(claude::chained_commands(&prompt_200k, &known));
    });

    let q_hit = find::query("migration");
    let q_miss = find::query("zzzznonexistent");
    bench("find::ranges (55KB, 500 matches)", 5, || {
        std::hint::black_box(find::ranges(&prompt_50k, &q_hit));
    });
    bench("find::ranges (220KB, 2000 matches)", 3, || {
        std::hint::black_box(find::ranges(&prompt_200k, &q_hit));
    });
    bench("find::contains (220KB, miss)", 20, || {
        std::hint::black_box(find::contains(&prompt_200k, &q_miss));
    });

    // Simulate searching 500 rows of 4KB each (2 MB)
    let rows_2mb: Vec<String> = (0..500).map(|i| format!("Row {i}: {}", "the parser handles utf8 text and tool output ".repeat(90))).collect();
    bench("find::contains across 500x4KB rows (miss)", 5, || {
        let hits = rows_2mb.iter().filter(|r| find::contains(r, &q_miss)).count();
        std::hint::black_box(hits);
    });

    let md_100k = make_huge_markdown(18);
    println!("\nmd_100k = {} bytes", md_100k.len());
    bench("markdown::parse (100KB rich md)", 10, || {
        std::hint::black_box(markdown::parse(&md_100k));
    });
    let parsed_md = markdown::parse(&md_100k);
    bench("Linker::link_markdown (100KB rich md)", 10, || {
        let mut md = markdown::Markdown { blocks: parsed_md.blocks.clone() };
        linker.link_markdown(&mut md);
        std::hint::black_box(md.blocks.len());
    });

    let big_code: String = (0..2000)
        .map(|j| format!("pub fn process_row_{j}(input: &str) -> usize {{ /* comment */ let x = \"str_{j}\"; if x.len() > 10 {{ return {j}; }} 0 }}\n"))
        .collect();
    println!("big_code = {} bytes (2000 lines)", big_code.len());
    bench("markdown::highlight (2000-line Rust)", 10, || {
        std::hint::black_box(markdown::highlight(&big_code, "rust"));
    });

    // Real extreme preset DB benchmarks (read-only! open_with None, never writes)
    let extreme_dir = Path::new("/Users/tricky_lake/dev/tricky-max/yourharness/test-harness-root/extreme");
    if extreme_dir.join("harness.sqlite").exists() {
        println!("\n=== Real extreme preset DB benchmarks (read-only) ===");
        let tmp = std::env::temp_dir().join(format!("yh-bench-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        // APFS clone
        let status = std::process::Command::new("cp")
            .args(["-c", &extreme_dir.join("harness.sqlite").to_string_lossy(), &tmp.join("harness.sqlite").to_string_lossy()])
            .status();
        if status.is_ok_and(|s| s.success()) {
            let db = Database::open_with(&tmp, None).expect("open db");
            let state = db.load_state();
            let huge_id = state
                .threads
                .iter()
                .find(|t| String::from_utf8_lossy(&t.json).contains("\"Huge 1\"") || String::from_utf8_lossy(&t.json).contains("Huge"))
                .map(|t| t.id.clone())
                .unwrap();
            let count = db.transcript_count(&huge_id);
            println!("Extreme thread {huge_id}: {count} rows, total threads in state: {}", state.threads.len());
            bench("db.load_state (120 threads)", 10, || {
                std::hint::black_box(db.load_state());
            });
            bench("db.transcript_count (32k rows)", 20, || {
                std::hint::black_box(db.transcript_count(&huge_id));
            });
            bench("db.read_rows (last 160 of 32k)", 10, || {
                let rows = db.read_rows(&huge_id, count.saturating_sub(160), 160);
                let entries: Vec<EntryData> = rows.into_iter().filter_map(EntryData::from_row).collect();
                std::hint::black_box(entries.len());
            });
            bench("db.read_rows (middle 160 of 32k)", 10, || {
                let rows = db.read_rows(&huge_id, count / 2, 160);
                let entries: Vec<EntryData> = rows.into_iter().filter_map(EntryData::from_row).collect();
                std::hint::black_box(entries.len());
            });
            bench("db.subagent_calls (32k rows)", 5, || {
                std::hint::black_box(db.subagent_calls(&huge_id));
            });

            // Benchmark Library::save() with 120 threads + a 200KB draft
            unsafe { std::env::set_var("YH_DATA_DIR", &tmp) };
            let lib = Library::open();
            bench("Library::set_setting (120 threads, no draft)", 20, || {
                lib.set_setting("selectedThreadID", &serde_json::Value::from(huge_id.as_str()));
            });
            lib.update_thread(&huge_id, |r| {
                r.insert("draft".into(), serde_json::Value::from(prompt_200k.as_str()));
            });
            bench("Library::set_setting (120 threads + 220KB draft)", 20, || {
                lib.set_setting("selectedThreadID", &serde_json::Value::from(huge_id.as_str()));
            });
            db.flush();
            drop(lib);
            drop(db);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
