//! The server end to end: a database written through yh-core's own writer, as the app writes
//! it, and a WebSocket client that should see what's there and what's written next.

use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use yh_core::db::{Database, Row, StateRows, ThreadRecord};
use yh_devserver::wire::{FromApp, ToApp};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yh-devserver-{name}-{}", std::process::id()));
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
}

fn start(dir: &std::path::Path, token: Option<&str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (dir, token) = (dir.to_path_buf(), token.map(str::to_string));
    std::thread::spawn(move || yh_devserver::serve(listener, dir, yh_devserver::Options { token, name: "test".into() }).unwrap());
    format!("ws://127.0.0.1:{port}/")
}

#[test]
fn serves_threads_and_rows_live() {
    let dir = temp_dir("live");
    let db = Database::open(&dir).unwrap();
    db.save_state(state("Fix checkout"));
    db.save_rows("t1", 2, vec![(0, row("u1", "user", "hi")), (1, row("a1", "assistant", "Hello"))]);
    db.flush();

    let mut c = Client::connect(&start(&dir, None)).unwrap();
    c.expect("hello", |m| matches!(m, ToApp::Hello { protocol: 1, .. }));

    c.send(FromApp::SubscribeApp);
    let ToApp::Threads { projects, threads } = c.expect("threads", |m| matches!(m, ToApp::Threads { .. })) else { unreachable!() };
    assert_eq!(projects[0].name, "shop");
    assert_eq!((threads[0].title.as_str(), threads[0].active), ("Fix checkout", false));

    c.send(FromApp::SubscribeThread { thread: "t1".into(), limit: Some(1) });
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
