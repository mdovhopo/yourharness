//! `cargo run -p yh-devserver --example watch [ws://127.0.0.1:7878/] [thread id]`: prints what the
//! phone would get, one line a message. Without a thread, follows the first one that turns active.

use std::time::Duration;

use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;
use yh_devserver::wire::{FromApp, Row, ToApp};

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "ws://127.0.0.1:7878/".into());
    let mut thread = args.next();
    let (mut ws, _) = tungstenite::connect(&url).expect("connect");
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    }
    let send = |ws: &mut tungstenite::WebSocket<_>, m: FromApp| ws.send(Message::text(serde_json::to_string(&m).unwrap())).unwrap();
    send(&mut ws, FromApp::SubscribeApp);
    if let Some(t) = &thread {
        send(&mut ws, FromApp::SubscribeThread { thread: t.clone(), limit: Some(5) });
    }
    let line = |r: &Row| format!("#{} {} {:?}", r.seq, r.kind, r.text.chars().take(60).collect::<String>() + r.tool_name.as_deref().unwrap_or(""));
    loop {
        let text = match ws.read() {
            Ok(Message::Text(t)) => t,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return eprintln!("closed: {e}"),
        };
        match serde_json::from_str::<ToApp>(&text).unwrap() {
            ToApp::Hello { server, data, .. } => println!("HELLO {server} · {data}"),
            ToApp::Threads { projects, threads } => println!("THREADS {} projects, {} threads", projects.len(), threads.len()),
            ToApp::ThreadUpserted { thread: t } => {
                println!("THREAD {} {:?} active={}", t.id, t.title, t.active);
                if thread.is_none() && t.active {
                    thread = Some(t.id.clone());
                    send(&mut ws, FromApp::SubscribeThread { thread: t.id, limit: Some(5) });
                }
            }
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
            ToApp::Error { message } => println!("ERROR {message}"),
        }
    }
}
