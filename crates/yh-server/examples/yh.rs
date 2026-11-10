//! A command-line client for the app's server, for tests and for trying the protocol:
//!
//! ```text
//! cargo run -p yh-server --example yh -- <ws url with ?token=…> watch [thread]
//! cargo run -p yh-server --example yh -- <url> send <thread> <text…> [--now]   (then follows the turn)
//! cargo run -p yh-server --example yh -- <url> stop <thread>
//! cargo run -p yh-server --example yh -- <url> allow|deny <thread> [request id]   (the first waiting)
//! cargo run -p yh-server --example yh -- <url> withdraw|send-now <thread> <message id>
//! cargo run -p yh-server --example yh -- <url> raw '<any message as JSON>'   (YH_WAIT=<s>: listens longer)
//! cargo run -p yh-server --example yh -- <url> listen <seconds> [thread]   (what a client gets: counts and bytes by type)
//! cargo run -p yh-server --example yh -- <url> browser <thread> <seconds> [out.jpg] ['<input as JSON>'…]
//!     (watches its browser, drawing each frame at once; inputs sent a second apart; the last frame saved)
//! ```
//!
//! Prints what it gets, one line a message: `THREAD`, `SNAPSHOT`, `ROWS`, `LIVE`, `DONE`. `watch`
//! without a thread follows the first one that starts working; `send` ends once the turn it
//! started is over.

use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use yh_server::wire::{FromApp, Live, Row, ToApp};

type Socket = WebSocket<MaybeTlsStream<std::net::TcpStream>>;

fn usage() -> ! {
    eprintln!("yh <url> watch [thread] | send <thread> <text…> [--now] | stop <thread> | allow|deny <thread> [request] | withdraw|send-now <thread> <message>");
    std::process::exit(2)
}

fn send(ws: &mut Socket, m: FromApp) {
    ws.send(Message::text(serde_json::to_string(&m).unwrap())).unwrap();
}

/// The next message, or None after a short wait.
fn next(ws: &mut Socket) -> Option<ToApp> {
    match ws.read() {
        Ok(Message::Text(t)) => serde_json::from_str(&t).ok(),
        Ok(_) => None,
        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => None,
        Err(e) => {
            eprintln!("closed: {e}");
            std::process::exit(1)
        }
    }
}

fn line(r: &Row) -> String {
    let text: String = r.text.chars().take(70).collect();
    format!("#{} {} {}{:?}", r.seq, r.kind, r.tool_name.as_deref().map(|t| format!("{t} ")).unwrap_or_default(), text)
}

fn live_line(l: &Live) -> String {
    let requests: Vec<String> = l.requests.iter().map(|r| format!("{}:{}", r.id, r.tool)).collect();
    let queue: Vec<&str> = l.queue.iter().map(|q| q.id.as_str()).collect();
    format!("running={} requests=[{}] queue=[{}] activity={:?}", l.running, requests.join(","), queue.join(","), l.activity)
}

/// Prints a message; true when it says the watched thread's turn is over (after it began).
fn print(m: &ToApp, began: &mut bool) -> bool {
    match m {
        ToApp::Hello { server, data, protocol, mac, hosts } => println!("HELLO {server} · {data} · protocol {protocol}{} · hosts [{}]", mac.as_deref().map(|m| format!(" · mac {m}")).unwrap_or_default(), hosts.join(",")),
        ToApp::Threads { projects, threads } => println!("THREADS {} projects, {} threads", projects.len(), threads.len()),
        ToApp::ThreadUpserted { thread: t } => println!("THREAD {} {:?} running={} waiting={}", t.id, t.title, t.running, t.waiting),
        ToApp::ThreadRemoved { id } => println!("REMOVED {id}"),
        ToApp::Snapshot { count, rows, .. } => {
            println!("SNAPSHOT {count} rows");
            rows.iter().for_each(|r| println!("  {}", line(r)));
        }
        ToApp::RowsChanged { count, rows, .. } => {
            println!("ROWS count={count}");
            rows.iter().for_each(|r| println!("  {}", line(r)));
        }
        ToApp::PageRows { rows, .. } => println!("PAGE {} rows", rows.len()),
        ToApp::Live { live, .. } => {
            println!("LIVE {}", live_line(live));
            if live.running {
                *began = true;
            } else if *began {
                return true;
            }
        }
        ToApp::Done { id, error, thread } => println!("DONE {id} {}{}", error.as_deref().unwrap_or("ok"), thread.as_deref().map(|t| format!(" thread={t}")).unwrap_or_default()),
        ToApp::Limits { windows, subscription } => println!("LIMITS {}{}", subscription.as_ref().map(|s| format!("[{} {}] ", s.plan, s.email.as_deref().unwrap_or_default())).unwrap_or_default(), windows.iter().map(|w| format!("{} {:.0}%", w.name, w.used * 100.)).collect::<Vec<_>>().join(", ")),
        ToApp::Power { power } => println!("POWER {}%{}{}", power.percent, if power.charging { " charging" } else { "" }, if power.plugged_in { " plugged in" } else { "" }),
        ToApp::SearchResults { query, hits } => println!("SEARCH {query:?} {} hits{}", hits.len(), hits.first().map(|h| format!(": {}", h.snippet)).unwrap_or_default()),
        ToApp::Files { query, files, .. } => println!("FILES {query:?} {}", files.join(", ")),
        ToApp::Options { models, modes, .. } => println!("OPTIONS models=[{}] modes=[{}]", models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(","), modes.join(",")),
        ToApp::Subagent { seq, run, .. } => println!("SUBAGENT #{seq} {:?} {} steps={} report={}", run.info.description, run.info.status, run.steps.len(), run.report.is_some()),
        ToApp::Media { seq, index, media_type, data, error, .. } => println!("MEDIA #{seq}.{index} {} {} {}", media_type.as_deref().unwrap_or("-"), data.as_ref().map_or(0, |d| d.len()), error.as_deref().unwrap_or("")),
        ToApp::Diagram { key, drawing, error } => println!("DIAGRAM {key} {}", drawing.as_ref().map(|d| format!("{} {}x{} paths={} labels={}", d.kind, d.width, d.height, d.paths.len(), d.labels.len())).unwrap_or_else(|| error.clone().unwrap_or_default())),
        ToApp::ToolResult { seq, input, result, .. } => println!("TOOL #{seq} input={} result={}", input.as_ref().map_or(0, |s| s.chars().count()), result.as_ref().map_or(0, |s| s.chars().count())),
        ToApp::Error { message } => println!("ERROR {message}"),
        ToApp::Pulls { project, pulls, errors, .. } => println!("PULLS {project} {} PRs{}{}", pulls.len(), pulls.iter().take(4).map(|p| format!(" | {}#{} {} [{} d{}]", p.repo, p.number, p.title, p.section, p.depth)).collect::<String>(), errors.iter().map(|e| format!(" ! {e}")).collect::<String>()),
        ToApp::Paired { device, token } => println!("PAIRED {device} token={token}"),
        ToApp::Pull { url, pull: Some(p), .. } => println!("PULL {url} {} [{}] checks {} comments {} merge {:?} {}", p.title, p.state, p.checks.len(), p.comments.len(), p.merge_status, p.merge_methods.join("/")),
        ToApp::Pull { url, error, .. } => println!("PULL {url} ! {}", error.as_deref().unwrap_or("")),
        ToApp::PullFiles { url, files, error } => println!("FILES {url} {}{}", files.iter().map(|f| format!(" | {} {} +{} -{}{}", f.status, f.path, f.additions, f.deletions, if f.has_patch { "" } else { " (no diff)" })).collect::<String>(), error.as_ref().map(|e| format!(" ! {e}")).unwrap_or_default()),
        ToApp::Browser { state, .. } => println!(
            "BROWSER tabs=[{}] selected={:?} {}x{} back={} control={} editing={}{}",
            state.tabs.iter().map(|t| format!("{} {:?} {}{}", t.id, t.title, t.url, if t.loading { " loading" } else { "" })).collect::<Vec<_>>().join(" | "),
            state.selected,
            state.width,
            state.height,
            state.can_back,
            state.user_control,
            state.editing,
            state.dialog.as_ref().map(|d| format!(" dialog={} {:?}", d.kind, d.message)).unwrap_or_default()
        ),
        ToApp::BrowserSummary { summary, .. } => println!("SUMMARY tabs={} {} {:?} opened_at={:?}", summary.tabs, summary.url, summary.title, summary.opened_at),
        ToApp::BrowserFrame { tab, seq, width, height, data, .. } => println!("FRAME #{seq} tab {tab} {width}x{height} {} bytes", data.len() * 3 / 4),
        ToApp::PullPatch { path, patch, error, .. } => println!("PATCH {path} {} lines{}", patch.as_ref().map(|p| p.lines().count()).unwrap_or(0), error.as_ref().map(|e| format!(" ! {e}")).unwrap_or_default()),
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(url), Some(command)) = (args.first(), args.get(1)) else { usage() };
    let rest = &args[2..];
    let (mut ws, _) = tungstenite::connect(url.as_str()).unwrap_or_else(|e| {
        eprintln!("can't connect: {e}");
        std::process::exit(1)
    });
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    }
    let mut began = false;
    let id = || Some(format!("a{}", std::process::id()));
    match command.as_str() {
        "watch" => {
            send(&mut ws, FromApp::SubscribeApp);
            let mut thread = rest.first().cloned();
            if let Some(t) = &thread {
                send(&mut ws, FromApp::SubscribeThread { thread: t.clone(), limit: Some(5), known: None });
            }
            loop {
                let Some(m) = next(&mut ws) else { continue };
                print(&m, &mut began);
                if let (None, ToApp::ThreadUpserted { thread: t }) = (&thread, &m) {
                    if t.running {
                        thread = Some(t.id.clone());
                        send(&mut ws, FromApp::SubscribeThread { thread: t.id.clone(), limit: Some(5), known: None });
                    }
                }
            }
        }
        "send" => {
            let Some(thread) = rest.first().cloned() else { usage() };
            let now = rest.iter().any(|a| a == "--now");
            let text = rest[1..].iter().filter(|a| *a != "--now").cloned().collect::<Vec<_>>().join(" ");
            send(&mut ws, FromApp::SubscribeThread { thread: thread.clone(), limit: Some(3), known: None });
            send(&mut ws, FromApp::Send { id: id(), thread, text, now, images: vec![] });
            let until = Instant::now() + Duration::from_secs(600);
            while Instant::now() < until {
                if let Some(m) = next(&mut ws) {
                    if print(&m, &mut began) {
                        println!("TURN over");
                        return;
                    }
                }
            }
        }
        "stop" => {
            let Some(thread) = rest.first().cloned() else { usage() };
            send(&mut ws, FromApp::Stop { id: id(), thread });
            wait_done(&mut ws);
        }
        "allow" | "deny" => {
            let Some(thread) = rest.first().cloned() else { usage() };
            let request_id = match rest.get(1) {
                Some(r) => r.clone(),
                // The first waiting, from the thread's live state.
                None => {
                    send(&mut ws, FromApp::SubscribeThread { thread: thread.clone(), limit: Some(1), known: None });
                    let until = Instant::now() + Duration::from_secs(30);
                    loop {
                        if Instant::now() > until {
                            eprintln!("nothing waits on you there");
                            std::process::exit(1)
                        }
                        if let Some(ToApp::Live { live, .. }) = next(&mut ws) {
                            if let Some(r) = live.requests.first() {
                                println!("REQUEST {} {} {:?}", r.id, r.tool, r.description);
                                break r.id.clone();
                            }
                        }
                    }
                }
            };
            send(&mut ws, FromApp::Respond { id: id(), thread, request_id, allow: command == "allow" });
            wait_done(&mut ws);
        }
        "withdraw" | "send-now" => {
            let (Some(thread), Some(message)) = (rest.first().cloned(), rest.get(1).cloned()) else { usage() };
            let m = if command == "withdraw" { FromApp::Withdraw { id: id(), thread, message } } else { FromApp::SendNow { id: id(), thread, message } };
            send(&mut ws, m);
            wait_done(&mut ws);
        }
        // Any message, as JSON: `yh <url> raw '{"type":"pin","id":"x","thread":"t","on":true}'`;
        // prints what comes back for two seconds.
        "raw" => {
            let Some(json) = rest.first() else { usage() };
            ws.send(Message::text(json.clone())).unwrap();
            // `YH_WAIT=<s>`: longer, for answers that take a while (a project's PRs).
            let wait = std::env::var("YH_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
            let until = Instant::now() + Duration::from_secs(wait);
            while Instant::now() < until {
                if let Some(m) = next(&mut ws) {
                    print(&m, &mut began);
                }
            }
        }
        // What a client with the list open (and a thread, if named) is sent over a while, by
        // message type: to check nothing streams that the client didn't ask for.
        "listen" => {
            let seconds: u64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(30);
            send(&mut ws, FromApp::SubscribeApp);
            if let Some(t) = rest.get(1) {
                send(&mut ws, FromApp::SubscribeThread { thread: t.clone(), limit: Some(1), known: None });
            }
            let mut counts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
            let until = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < until {
                match ws.read() {
                    Ok(Message::Text(t)) => {
                        let kind = serde_json::from_str::<serde_json::Value>(&t).ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default();
                        let thread = serde_json::from_str::<serde_json::Value>(&t).ok().and_then(|v| v["thread"].as_str().or(v["thread"]["id"].as_str()).map(str::to_string)).unwrap_or_default();
                        let e = counts.entry(format!("{kind} {}", thread.chars().take(8).collect::<String>())).or_default();
                        e.0 += 1;
                        e.1 += t.len();
                    }
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(e) => {
                        eprintln!("closed: {e}");
                        break;
                    }
                }
            }
            for (k, (n, bytes)) in counts {
                println!("LISTEN {k}: {n} messages, {bytes} bytes");
            }
        }
        // A thread's browser as the phone sees it: each frame acked as soon as it comes (the
        // last saved to `out.jpg`), and inputs (`{"kind":"tap","x":10,"y":20}`) sent a second apart.
        "browser" => {
            let (Some(thread), Some(seconds)) = (rest.first().cloned(), rest.get(1).and_then(|s| s.parse::<u64>().ok())) else { usage() };
            let out = rest.get(2).filter(|p| !p.starts_with('{')).cloned();
            let mut inputs: std::collections::VecDeque<String> = rest.iter().skip(2).filter(|a| a.starts_with('{')).cloned().collect();
            send(&mut ws, FromApp::WatchBrowser { thread: thread.clone(), width: Some(1200) });
            let started = Instant::now();
            let mut next_input = started + Duration::from_secs(2);
            let mut frames = 0;
            while started.elapsed() < Duration::from_secs(seconds) {
                if Instant::now() >= next_input {
                    if let Some(json) = inputs.pop_front() {
                        let input: yh_server::wire::BrowserInput = serde_json::from_str(&json).unwrap_or_else(|e| {
                            eprintln!("bad input {json}: {e}");
                            std::process::exit(2)
                        });
                        println!("INPUT {json}");
                        send(&mut ws, FromApp::BrowserInput { id: id(), thread: thread.clone(), input });
                    }
                    next_input = Instant::now() + Duration::from_secs(1);
                }
                let Some(m) = next(&mut ws) else { continue };
                print(&m, &mut began);
                if let ToApp::BrowserFrame { seq, data, .. } = &m {
                    frames += 1;
                    send(&mut ws, FromApp::BrowserAck { seq: *seq });
                    if let Some(path) = &out {
                        let bytes = base64_decode(data);
                        std::fs::write(path, bytes).unwrap();
                    }
                }
            }
            println!("BROWSER {frames} frames in {seconds} s");
        }
        _ => usage(),
    }
}

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        (match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        }) as u32
    };
    let digits: Vec<u8> = text.bytes().filter(|b| *b != b'=').collect();
    let mut out = vec![];
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= value(*c) << (18 - 6 * i);
        }
        let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&b[..chunk.len().saturating_sub(1)]);
    }
    out
}

fn wait_done(ws: &mut Socket) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if let Some(m) = next(ws) {
            let done = matches!(m, ToApp::Done { .. } | ToApp::Error { .. });
            print(&m, &mut false);
            if done {
                return;
            }
        }
    }
    eprintln!("no answer");
    std::process::exit(1)
}
