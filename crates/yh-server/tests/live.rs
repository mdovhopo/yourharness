//! The server end to end: a database written through yh-core's own writer, as the app writes
//! it, and a WebSocket client that should see what's there and what's written next.

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use yh_core::db::{Database, Row, StateRows, ThreadRecord};
use yh_server::wire::{FromApp, ToApp};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yh-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn row(id: &str, kind: &str, text: &str) -> Row {
    Row { id: id.into(), kind: kind.into(), text: text.into(), date: Some(1.0), ..Default::default() }
}

fn state(title: &str) -> StateRows {
    StateRows {
        projects: vec![("p1".into(), br#"{"name":"shop","path":"/tmp/shop"}"#.to_vec())],
        threads: vec![ThreadRecord {
            id: "t1".into(),
            project_id: "p1".into(),
            updated_at: Some(1_700_000_000.0),
            session_id: None,
            json: format!(r#"{{"title":"{title}","createdAt":"2026-11-09T10:00:00Z"}}"#).into_bytes(),
        }],
        ..Default::default()
    }
}

struct Client(WebSocket<MaybeTlsStream<TcpStream>>);

impl Client {
    fn connect(url: &str) -> Result<Client, tungstenite::Error> {
        let (ws, _) = tungstenite::connect(url)?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        }
        Ok(Client(ws))
    }

    fn send(&mut self, m: FromApp) {
        self.0.send(Message::text(serde_json::to_string(&m).unwrap())).unwrap();
    }

    /// The next message `want` picks, skipping others, within 5 s.
    fn expect(&mut self, what: &str, want: impl Fn(&ToApp) -> bool) -> ToApp {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            match self.0.read() {
                Ok(Message::Text(t)) => {
                    let m: ToApp = serde_json::from_str(&t).unwrap();
                    if want(&m) {
                        return m;
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => panic!("waiting for {what}: {e}"),
            }
        }
        panic!("no {what} within 5 s");
    }

    /// The server closes the connection within `time`.
    fn closed_within(&mut self, time: Duration) -> bool {
        let until = Instant::now() + time;
        while Instant::now() < until {
            match self.0.read() {
                Ok(Message::Close(_)) => return true,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }
}

fn start(dir: &std::path::Path, token: Option<&str>) -> String {
    start_with(dir, token, None)
}

fn start_with(dir: &std::path::Path, token: Option<&str>, links: Option<yh_server::Links>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (dir, token) = (dir.to_path_buf(), token.map(str::to_string));
    std::thread::spawn(move || yh_server::serve(listener, dir, yh_server::Options { token, name: "test".into(), pairing: None, stop: None, mac_id: None, ..Default::default() }, links).unwrap());
    format!("ws://127.0.0.1:{port}/")
}

fn delta(message: &str, text: &str) -> yh_core::claude::Event {
    yh_core::claude::Event::Delta { message: message.into(), index: 0, parent: None, kind: yh_core::claude::DeltaKind::Text, text: text.into() }
}

/// Through a real engine: a reply streams to the client as Claude writes it, whole rows each
/// time; a client that starts watching mid-reply gets the engine's snapshot and the rest, the
/// text exactly once; a request waiting on you shows, and the client's answer reaches the app.
#[test]
fn streams_what_claude_writes_and_takes_actions() {
    use yh_engine::Command;
    let dir = temp_dir("stream");
    let db = std::sync::Arc::new(Database::open(&dir).unwrap());
    db.save_state(state("Fix checkout"));
    db.save_rows("t1", 1, vec![(0, row("u0", "user", "earlier"))]);
    db.flush();
    let engine = yh_engine::start(Some(db.clone()));
    let (actions, asked) = flume::unbounded::<yh_server::Asked>();
    // The app: takes each action and says how it went; a new thread is named "t2".
    let (seen, app) = flume::unbounded();
    std::thread::spawn(move || {
        for a in asked {
            let made = matches!(a.action, yh_server::Action::NewThread { .. }).then(|| "t2".to_string());
            let answer = if a.action == (yh_server::Action::Delete { thread: "nope".into() }) { Err("No such thread.".to_string()) } else { Ok(made) };
            seen.send(a.action).unwrap();
            a.reply.send(answer).unwrap();
        }
    });
    let url = start_with(&dir, None, Some(yh_server::Links { engine: engine.clone(), actions }));
    let mut c = Client::connect(&url).unwrap();
    c.expect("hello", |m| matches!(m, ToApp::Hello { .. }));
    c.send(FromApp::SubscribeApp);
    c.expect("threads", |m| matches!(m, ToApp::Threads { .. }));
    c.expect("options", |m| matches!(m, ToApp::Options { modes, .. } if modes.len() == 5));
    c.send(FromApp::SubscribeThread { thread: "t1".into(), limit: None, known: None });
    let ToApp::Snapshot { count, rows, .. } = c.expect("snapshot", |m| matches!(m, ToApp::Snapshot { .. })) else { unreachable!() };
    assert_eq!((count, rows.len(), rows[0].text.as_str()), (1, 1, "earlier"));
    c.expect("idle", |m| matches!(m, ToApp::Live { live, .. } if !live.running));

    let t = || "t1".to_string();
    engine.send(Command::Send { thread: t(), text: "go".into(), attachments: vec![], message_id: "u1".into(), now: false, effort: None, deliver: None });
    c.expect("working", |m| matches!(m, ToApp::Live { live, .. } if live.running));
    c.expect("your message", |m| matches!(m, ToApp::RowsChanged { rows, .. } if rows.iter().any(|r| r.seq == 1 && r.text == "go")));
    let start = yh_core::claude::Event::BlockStart { message: "m".into(), index: 0, parent: None, kind: yh_core::claude::BlockKind::Text };
    engine.send(Command::Events { thread: t(), events: vec![start, delta("m", "Hel")] });
    c.expect("the reply begins", |m| matches!(m, ToApp::RowsChanged { rows, .. } if rows.iter().any(|r| r.seq == 2 && r.text == "Hel")));

    // A second client starts watching mid-reply.
    let mut d = Client::connect(&url).unwrap();
    d.expect("hello", |m| matches!(m, ToApp::Hello { .. }));
    d.send(FromApp::SubscribeThread { thread: "t1".into(), limit: None, known: None });
    let ToApp::Snapshot { rows, .. } = d.expect("mid-reply snapshot", |m| matches!(m, ToApp::Snapshot { .. })) else { unreachable!() };
    let snapshot_rev = rows.iter().map(|r| r.rev).max().unwrap();
    assert_eq!(rows.last().map(|r| r.text.as_str()), Some("Hel"));

    engine.send(Command::Events { thread: t(), events: vec![delta("m", "lo, wor")] });
    engine.send(Command::Events { thread: t(), events: vec![delta("m", "ld")] });
    for (name, client) in [("first", &mut c), ("second", &mut d)] {
        let ToApp::RowsChanged { rows, .. } = client.expect(name, |m| matches!(m, ToApp::RowsChanged { rows, .. } if rows.iter().any(|r| r.seq == 2 && r.text == "Hello, world"))) else { unreachable!() };
        assert!(rows[0].rev > snapshot_rev, "streamed rows are newer than the snapshot");
    }

    let ask = yh_core::claude::Event::Permission { request_id: "r1".into(), tool: "Write".into(), description: "a.txt".into(), input: "{}".into() };
    engine.send(Command::Events { thread: t(), events: vec![ask] });
    c.expect("the request", |m| matches!(m, ToApp::Live { live, .. } if live.requests.first().is_some_and(|r| r.id == "r1")));
    c.expect("waiting", |m| matches!(m, ToApp::ThreadUpserted { thread } if thread.waiting));
    c.send(FromApp::Respond { id: Some("a1".into()), thread: t(), request_id: "r1".into(), allow: true });
    c.expect("done", |m| matches!(m, ToApp::Done { id, error: None, .. } if id == "a1"));
    assert_eq!(app.recv_timeout(Duration::from_secs(1)).unwrap(), yh_server::Action::Respond { thread: t(), request_id: "r1".into(), allow: true });

    // Protocol 5: a side chat watched beside the thread; both go on streaming.
    c.send(FromApp::SubscribeThread { thread: "side:t1".into(), limit: None, known: None });
    c.expect("side snapshot", |m| matches!(m, ToApp::Snapshot { thread, count: 0, .. } if thread == "side:t1"));
    engine.send(Command::Send { thread: "side:t1".into(), text: "why?".into(), attachments: vec![], message_id: "s1".into(), now: false, effort: None, deliver: None });
    c.expect("the side question", |m| matches!(m, ToApp::RowsChanged { thread, rows, .. } if thread == "side:t1" && rows[0].text == "why?"));
    engine.send(Command::Events { thread: t(), events: vec![delta("m", "!")] });
    c.expect("the thread still streams", |m| matches!(m, ToApp::RowsChanged { thread, rows, .. } if thread == "t1" && rows.iter().any(|r| r.text == "Hello, world!")));
    // The side chat closed: its rows stop, the thread's go on.
    c.send(FromApp::UnsubscribeThread { thread: Some("side:t1".into()) });
    std::thread::sleep(Duration::from_millis(100));
    engine.send(Command::Send { thread: "side:t1".into(), text: "and?".into(), attachments: vec![], message_id: "s2".into(), now: false, effort: None, deliver: None });
    engine.send(Command::Events { thread: t(), events: vec![delta("m", "?")] });
    let m = c.expect("only the thread", |m| matches!(m, ToApp::RowsChanged { .. }));
    assert!(matches!(&m, ToApp::RowsChanged { thread, .. } if thread == "t1"), "{m:?}");

    // Protocol 3: an answer to Claude's question, a new thread (named in `done`), a picture with
    // a message, an action the app refuses, a mode it doesn't know.
    let answers: std::collections::BTreeMap<String, String> = [("Which?".to_string(), "Rust".to_string())].into();
    c.send(FromApp::Answer { id: Some("a4".into()), thread: t(), request_id: "r2".into(), answers: answers.clone() });
    c.expect("answered", |m| matches!(m, ToApp::Done { id, error: None, .. } if id == "a4"));
    assert_eq!(app.recv_timeout(Duration::from_secs(1)).unwrap(), yh_server::Action::Answer { thread: t(), request_id: "r2".into(), answers });
    c.send(FromApp::NewThread { id: Some("a5".into()), project: "p1".into(), text: Some("hi".into()), model: None, effort: None, mode: None });
    c.expect("made", |m| matches!(m, ToApp::Done { id, error: None, thread: Some(t) } if id == "a5" && t == "t2"));
    app.recv_timeout(Duration::from_secs(1)).unwrap();
    c.send(FromApp::Send { id: Some("a6".into()), thread: t(), text: "look".into(), now: false, images: vec![yh_server::wire::Image { media_type: "image/png".into(), data: "aGVsbG8=".into() }] });
    c.expect("sent", |m| matches!(m, ToApp::Done { id, error: None, .. } if id == "a6"));
    assert_eq!(app.recv_timeout(Duration::from_secs(1)).unwrap(), yh_server::Action::Send { thread: t(), text: "look".into(), now: false, images: vec![("image/png".into(), b"hello".to_vec())] });
    c.send(FromApp::Delete { id: Some("a7".into()), thread: "nope".into() });
    c.expect("refused", |m| matches!(m, ToApp::Done { id, error: Some(e), .. } if id == "a7" && e == "No such thread."));
    app.recv_timeout(Duration::from_secs(1)).unwrap();
    c.send(FromApp::SetMode { id: Some("a8".into()), thread: t(), mode: "yolo".into() });
    c.expect("unknown mode", |m| matches!(m, ToApp::Done { id, error: Some(_), .. } if id == "a8"));

    engine.send(Command::Events { thread: t(), events: vec![yh_core::claude::Event::Done { is_error: false, text: String::new() }] });
    c.expect("over", |m| matches!(m, ToApp::Live { live, .. } if !live.running));

    // Read-only without links: an action is refused.
    let mut r = Client::connect(&start_with(&dir, None, None)).unwrap();
    r.expect("hello", |m| matches!(m, ToApp::Hello { .. }));
    r.send(FromApp::Stop { id: Some("a3".into()), thread: t() });
    r.expect("refused", |m| matches!(m, ToApp::Done { id, error: Some(_), .. } if id == "a3"));
}

#[test]
fn serves_threads_and_rows_live() {
    let dir = temp_dir("live");
    let db = Database::open(&dir).unwrap();
    db.save_state(state("Fix checkout"));
    db.save_rows("t1", 2, vec![(0, row("u1", "user", "hi")), (1, row("a1", "assistant", "Hello"))]);
    db.flush();

    let mut c = Client::connect(&start(&dir, None)).unwrap();
    c.expect("hello", |m| matches!(m, ToApp::Hello { protocol: yh_server::wire::PROTOCOL, .. }));

    c.send(FromApp::SubscribeApp);
    let ToApp::Threads { projects, threads } = c.expect("threads", |m| matches!(m, ToApp::Threads { .. })) else { unreachable!() };
    assert_eq!(projects[0].name, "shop");
    assert_eq!((threads[0].title.as_str(), threads[0].active), ("Fix checkout", false));

    c.send(FromApp::SubscribeThread { thread: "t1".into(), limit: Some(1), known: None });
    let ToApp::Snapshot { first_seq, count, rows, .. } = c.expect("snapshot", |m| matches!(m, ToApp::Snapshot { .. })) else { unreachable!() };
    assert_eq!((first_seq, count, rows.len(), rows[0].text.as_str()), (1, 2, 1, "Hello"));

    // A reply streaming in: the last row grows, a tool call follows.
    db.save_rows("t1", 3, vec![(1, row("a1", "assistant", "Hello there")), (2, Row { tool_name: Some("Bash".into()), ..row("x1", "tool", "") })]);
    db.flush();
    let ToApp::RowsChanged { count, rows, .. } = c.expect("rows_changed", |m| matches!(m, ToApp::RowsChanged { .. })) else { unreachable!() };
    assert_eq!(count, 3);
    assert_eq!(rows.iter().map(|r| (r.seq, r.text.as_str())).collect::<Vec<_>>(), vec![(1, "Hello there"), (2, "")]);
    assert!(rows[0].rev > 0 && rows[1].tool_name.as_deref() == Some("Bash"));
    let ToApp::ThreadUpserted { thread } = c.expect("thread_upserted", |m| matches!(m, ToApp::ThreadUpserted { .. })) else { unreachable!() };
    assert!(thread.active);

    // Renamed on the Mac.
    db.save_state(state("Fix the checkout"));
    db.flush();
    c.expect("rename", |m| matches!(m, ToApp::ThreadUpserted { thread } if thread.title == "Fix the checkout"));

    c.send(FromApp::Page { thread: "t1".into(), before_seq: 1, limit: None });
    let ToApp::PageRows { rows, .. } = c.expect("page_rows", |m| matches!(m, ToApp::PageRows { .. })) else { unreachable!() };
    assert_eq!(rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["hi"]);
}

#[test]
fn a_token_is_required_when_set() {
    let dir = temp_dir("token");
    let db = Database::open(&dir).unwrap();
    db.save_state(state("t"));
    db.flush();
    let url = start(&dir, Some("s3cret"));
    assert!(Client::connect(&url).is_err());
    assert!(Client::connect(&format!("{url}?token=nope")).is_err());
    let mut c = Client::connect(&format!("{url}?token=s3cret")).unwrap();
    c.expect("hello", |m| matches!(m, ToApp::Hello { .. }));
}

/// A phone pairs with a code (once), connects with its token after, is cut off when forgotten,
/// and everyone is when serving stops.
#[test]
fn a_phone_pairs_with_a_code_and_is_cut_off_when_forgotten() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dir = temp_dir("pair");
    let db = Database::open(&dir).unwrap();
    db.save_state(state("t"));
    db.flush();
    let pairing = Arc::new(yh_server::pairing::Pairing::new(vec![], Box::new(|_| {})));
    let stop = Arc::new(AtomicBool::new(false));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let options = yh_server::Options { token: None, name: "test".into(), pairing: Some(pairing.clone()), stop: Some(stop.clone()), mac_id: Some("mac-1".into()), ..Default::default() };
    let served = std::thread::spawn(move || yh_server::serve(listener, dir, options, None));
    assert!(Client::connect(&url).is_err(), "nothing without a token");
    assert!(Client::connect(&format!("{url}?pair=nope&device=x")).is_err(), "a wrong code");
    let code = pairing.new_code();
    let mut c = Client::connect(&format!("{url}?pair={code}&device=Max%27s%20iPhone")).unwrap();
    c.expect("hello", |m| matches!(m, ToApp::Hello { .. }));
    let ToApp::Paired { device, token } = c.expect("paired", |m| matches!(m, ToApp::Paired { .. })) else { unreachable!() };
    assert_eq!(pairing.devices()[0].name, "Max's iPhone");
    assert!(Client::connect(&format!("{url}?pair={code}&device=y")).is_err(), "the code works once");
    let mut d = Client::connect(&format!("{url}?token={token}")).unwrap();
    d.expect("hello again", |m| matches!(m, ToApp::Hello { .. }));
    pairing.forget(&device);
    assert!(d.closed_within(Duration::from_secs(2)), "forgotten: its connection ends");
    assert!(Client::connect(&format!("{url}?token={token}")).is_err(), "and its token no longer works");
    stop.store(true, Ordering::Relaxed);
    assert!(served.join().unwrap().is_ok(), "serving stops");
}
