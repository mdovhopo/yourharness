//! `yh`: YourHarness from the command line (docs/cli.md). It talks to the running app over its
//! local socket (`yh_proto::local_socket`, only you can open it) in the protocol the phone
//! speaks, and starts the app when it isn't running. Everything a person does from a terminal
//! another app or an agent can do too: `--json` prints what each command says as JSON.

use std::collections::{BTreeMap, VecDeque};
use std::io::{IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};
use yh_proto::{FromApp, Live, Project, Row, Thread, ToApp};

const HELP: &str = "\
yh: YourHarness from the command line

Usage:
  yh open [FOLDER] [--draft TEXT] [-f FILE]...
                         Bring the app forward; with FOLDER, a new thread there (the folder
                         becomes a project if it isn't one), DRAFT typed and FILEs attached
  yh new PROMPT [-p PROJECT] [-f FILE]... [-m MODEL] [-e EFFORT] [--mode MODE]
         [--title TITLE] [--show] [--wait | --follow]
                         Start a thread with PROMPT (\"-\": read from stdin). PROJECT is a
                         folder, a project's name or ID; without it, the current folder's
  yh send THREAD PROMPT [-f FILE]... [--now] [--wait | --follow]
                         Send to a thread: it steers into a running turn (--now: interrupts)
  yh wait THREAD [--timeout SECONDS]
                         Wait until the thread's turn ends or it needs you; print its reply
  yh read THREAD [-n ROWS]
                         Print the thread's last rows (40)
  yh watch THREAD        Follow the thread as it works (Ctrl-C to stop)
  yh stop THREAD         Stop the thread's turn
  yh allow THREAD [REQUEST]   Allow what the thread waits on (the first request)
  yh deny THREAD [REQUEST]    Deny it
  yh show THREAD         Open the thread in the app's window
  yh threads [-p PROJECT] [--all]
                         List threads (--all: settled ones too)
  yh projects            List projects
  yh add FOLDER          Add a folder as a project
  yh rename THREAD TITLE | pin THREAD | unpin THREAD | settle THREAD | unsettle THREAD
  yh status              Whether the app is running, and which

THREAD is an ID, the start of one, a title, or \"last\" (the latest one worked on).

Options:
  --json                 Print JSON (one object; with --follow, one per line)
  --wait                 After sending, wait for the turn to end and print Claude's reply
  --follow               After sending, print Claude's reply as it's written; tools on stderr
  --timeout SECONDS      How long --wait, --follow and wait may take (no limit)
  --no-launch            Fail instead of starting the app

Exit codes: 0 done, 1 failed, 2 wrong usage, 3 the thread waits on you (a permission, a
question or a plan), 4 timed out, 5 the app isn't running (--no-launch, status).

Environment: YH_DATA_DIR (another data folder), YH_APP (the app to start).
";

/// Flags that take a value, and their short forms.
const VALUED: &[(&str, &str)] = &[("project", "p"), ("file", "f"), ("model", "m"), ("effort", "e"), ("mode", ""), ("title", ""), ("draft", ""), ("rows", "n"), ("timeout", ""), ("request", "")];
const SWITCHES: &[(&str, &str)] = &[("json", ""), ("wait", "w"), ("follow", ""), ("now", ""), ("show", ""), ("all", "a"), ("no-launch", ""), ("help", "h"), ("version", "")];
const MODES: &[&str] = &["auto", "ask", "acceptEdits", "plan", "bypass"];
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

struct Fail(i32, String);

impl From<String> for Fail {
    fn from(s: String) -> Fail {
        Fail(1, s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Fail {
        Fail(1, s.to_string())
    }
}

fn usage(why: impl Into<String>) -> Fail {
    Fail(2, format!("{}\nSee yh --help.", why.into()))
}

#[derive(Default)]
struct Args {
    words: Vec<String>,
    values: BTreeMap<&'static str, Vec<String>>,
    switches: Vec<&'static str>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Args, Fail> {
        let mut a = Args::default();
        let mut it = raw.into_iter();
        let mut words_only = false;
        while let Some(arg) = it.next() {
            if words_only || arg == "-" || !arg.starts_with('-') {
                a.words.push(arg);
                continue;
            }
            if arg == "--" {
                words_only = true;
                continue;
            }
            let (name, inline) = match arg.strip_prefix("--") {
                Some(rest) => match rest.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (rest.to_string(), None),
                },
                None => (arg[1..].to_string(), None),
            };
            let long = |table: &[(&'static str, &'static str)]| table.iter().find(|(l, s)| *l == name || (!s.is_empty() && *s == name)).map(|(l, _)| *l);
            if let Some(flag) = long(VALUED) {
                let value = match inline {
                    Some(v) => v,
                    None => it.next().ok_or_else(|| usage(format!("--{flag} needs a value.")))?,
                };
                a.values.entry(flag).or_default().push(value);
            } else if let Some(flag) = long(SWITCHES) {
                a.switches.push(flag);
            } else {
                return Err(usage(format!("No option {arg}.")));
            }
        }
        Ok(a)
    }

    fn on(&self, flag: &str) -> bool {
        self.switches.contains(&flag)
    }

    fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).and_then(|v| v.last()).map(String::as_str)
    }

    fn all(&self, flag: &str) -> &[String] {
        self.values.get(flag).map(Vec::as_slice).unwrap_or(&[])
    }

    fn timeout(&self) -> Result<Option<Duration>, Fail> {
        self.value("timeout").map(|t| t.parse::<f64>().ok().filter(|t| *t > 0.0).map(Duration::from_secs_f64).ok_or_else(|| usage("--timeout is a number of seconds."))).transpose()
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let json = raw.iter().any(|a| a == "--json");
    let code = match run(raw) {
        Ok(()) => 0,
        Err(Fail(code, message)) => {
            if json {
                println!("{}", json!({ "error": message, "code": code }));
            } else {
                eprintln!("yh: {message}");
            }
            code
        }
    };
    std::process::exit(code);
}

fn run(raw: Vec<String>) -> Result<(), Fail> {
    let mut args = Args::parse(raw)?;
    if args.on("version") {
        println!("yh {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.on("help") || args.words.is_empty() {
        print!("{HELP}");
        return Ok(());
    }
    let mut command = args.words.remove(0);
    // `yh .`, `yh ~/dev/app`: as `code .` opens a folder.
    if command.contains('/') || command == "." || command == ".." {
        args.words.insert(0, command);
        command = "open".into();
    }
    match command.as_str() {
        "open" => open(&args),
        "new" => new(&args),
        "send" => send(&args),
        "wait" => wait(&args),
        "read" => read(&args),
        "watch" => watch(&args),
        "stop" => simple(&args, |id, t| FromApp::Stop { id, thread: t }),
        "allow" => respond(&args, true),
        "deny" => respond(&args, false),
        "show" => show(&args),
        "threads" | "list" | "ls" => threads(&args),
        "projects" => projects(&args),
        "add" => add(&args),
        "rename" => rename(&args),
        "pin" => simple(&args, |id, t| FromApp::Pin { id, thread: t, on: true }),
        "unpin" => simple(&args, |id, t| FromApp::Pin { id, thread: t, on: false }),
        "settle" => simple(&args, |id, t| FromApp::Settle { id, thread: t, on: true }),
        "unsettle" => simple(&args, |id, t| FromApp::Settle { id, thread: t, on: false }),
        "status" => status(&args),
        "help" => {
            print!("{HELP}");
            Ok(())
        }
        other => Err(usage(format!("No command {other}."))),
    }
}

// MARK: The app

/// The data folder the app uses: `YH_DATA_DIR`, else the usual one.
fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    match std::env::var("YH_DATA_DIR").ok().filter(|d| !d.is_empty()) {
        Some(d) => match d.strip_prefix("~/") {
            Some(rest) => Path::new(&home).join(rest),
            None => PathBuf::from(d),
        },
        None => Path::new(&home).join("Library/Application Support/YourHarness"),
    }
}

/// The app to start: `YH_APP`, else the one this `yh` lives in, else the installed one.
fn app_bundle() -> Option<PathBuf> {
    if let Some(app) = std::env::var("YH_APP").ok().filter(|a| !a.is_empty()) {
        return Some(PathBuf::from(app));
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Starts the app (`foreground`: brought forward too), passing on `YH_*` settings.
fn launch(foreground: bool) -> Result<(), Fail> {
    let mut open = std::process::Command::new("/usr/bin/open");
    if !foreground {
        open.arg("-g");
    }
    for (k, v) in std::env::vars() {
        if k.starts_with("YH_") && k != "YH_APP" {
            open.arg("--env").arg(format!("{k}={v}"));
        }
    }
    // A copy named by `YH_APP` (a test build) starts as one of its own, never bringing forward
    // another copy with its bundle ID.
    if std::env::var("YH_APP").is_ok_and(|a| !a.is_empty()) {
        open.arg("-n");
    }
    match app_bundle() {
        Some(app) => open.arg("-a").arg(app),
        None => open.args(["-b", "com.mdovhopo.yourharness"]),
    };
    let status = open.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map_err(|e| format!("Can't start YourHarness: {e}"))?;
    if !status.success() {
        return Err("Can't start YourHarness: is it installed?".into());
    }
    Ok(())
}

struct App {
    ws: WebSocket<UnixStream>,
    hello: ToApp,
    /// What came while waiting for something else, in order.
    held: VecDeque<ToApp>,
    next: u32,
    listing: Option<(Vec<Project>, Vec<Thread>)>,
}

/// The running app, started first when it isn't (unless `--no-launch`).
fn connect(args: &Args, foreground: bool) -> Result<App, Fail> {
    let path = yh_proto::local_socket(&data_dir());
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) if args.on("no-launch") => return Err(Fail(5, "YourHarness isn't running.".into())),
        Err(_) => {
            launch(foreground)?;
            let started = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(s) = UnixStream::connect(&path) {
                    break s;
                }
                if started.elapsed() > Duration::from_secs(60) {
                    return Err(Fail(5, format!("YourHarness didn't answer at {} (it takes the command line from 0.3 on).", path.display())));
                }
            }
        }
    };
    let (ws, _) = tungstenite::client("ws://localhost/", stream).map_err(|e| format!("Can't talk to YourHarness: {e}"))?;
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
    let mut app = App { ws, hello: ToApp::Error { message: String::new() }, held: VecDeque::new(), next: 0, listing: None };
    app.hello = app.expect(Some(Duration::from_secs(10)), |m| matches!(m, ToApp::Hello { .. }).then(|| m.clone()))?;
    Ok(app)
}

impl App {
    fn send(&mut self, m: &FromApp) -> Result<(), Fail> {
        let text = serde_json::to_string(m).expect("messages serialize");
        self.ws.send(Message::text(text)).map_err(|e| Fail(1, format!("YourHarness went away: {e}")))
    }

    /// The next message, or None after a moment.
    fn next(&mut self) -> Result<Option<ToApp>, Fail> {
        if let Some(m) = self.held.pop_front() {
            return Ok(Some(m));
        }
        match self.ws.read() {
            Ok(Message::Text(t)) => Ok(serde_json::from_str(&t).ok()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(Fail(1, format!("YourHarness went away: {e}"))),
        }
    }

    /// Reads until `pick` takes a message, holding the rest for later; an `error` fails.
    fn expect<T>(&mut self, within: Option<Duration>, mut pick: impl FnMut(&ToApp) -> Option<T>) -> Result<T, Fail> {
        if let Some(at) = self.held.iter().position(|m| pick(m).is_some()) {
            let m = self.held.remove(at).expect("found");
            return Ok(pick(&m).expect("picked"));
        }
        let started = Instant::now();
        let mut skipped = VecDeque::new();
        let result = loop {
            if within.is_some_and(|w| started.elapsed() > w) {
                break Err(Fail(1, "YourHarness didn't answer.".into()));
            }
            // Only what wasn't held already, so held messages aren't read twice.
            let m = match self.ws.read() {
                Ok(Message::Text(t)) => serde_json::from_str::<ToApp>(&t).ok(),
                Ok(_) => None,
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => None,
                Err(e) => break Err(Fail(1, format!("YourHarness went away: {e}"))),
            };
            let Some(m) = m else { continue };
            if let Some(t) = pick(&m) {
                break Ok(t);
            }
            if let ToApp::Error { message } = &m {
                break Err(Fail(1, message.clone()));
            }
            skipped.push_back(m);
        };
        self.held.extend(skipped);
        result
    }

    /// An action, answered with its `done`: the thread or project it made, or why it failed.
    fn act(&mut self, make: impl FnOnce(Option<String>) -> FromApp) -> Result<(Option<String>, Option<String>), Fail> {
        self.next += 1;
        let id = format!("yh{}", self.next);
        self.send(&make(Some(id.clone())))?;
        let (error, thread, project) = self.expect(Some(Duration::from_secs(30)), |m| match m {
            ToApp::Done { id: done, error, thread, project } if *done == id => Some((error.clone(), thread.clone(), project.clone())),
            _ => None,
        })?;
        match error {
            Some(e) => Err(Fail(1, e)),
            None => Ok((thread, project)),
        }
    }

    /// Every project and thread, in the sidebar's order.
    fn listing(&mut self) -> Result<(Vec<Project>, Vec<Thread>), Fail> {
        if let Some(l) = &self.listing {
            return Ok(l.clone());
        }
        self.send(&FromApp::SubscribeApp)?;
        let l = self.expect(Some(Duration::from_secs(10)), |m| match m {
            ToApp::Threads { projects, threads } => Some((projects.clone(), threads.clone())),
            _ => None,
        })?;
        self.listing = Some(l.clone());
        Ok(l)
    }

    /// Watches `thread`: its last `limit` rows, how many it has, and what runs.
    fn subscribe(&mut self, thread: &str, limit: i64) -> Result<(i64, Vec<Row>, Live), Fail> {
        self.send(&FromApp::SubscribeThread { thread: thread.to_string(), limit: Some(limit), known: None })?;
        let (count, rows) = self.expect(Some(Duration::from_secs(15)), |m| match m {
            ToApp::Snapshot { thread: t, count, rows, .. } if t == thread => Some((*count, rows.clone())),
            _ => None,
        })?;
        let live = self.expect(Some(Duration::from_secs(5)), |m| match m {
            ToApp::Live { thread: t, live } if t == thread => Some(live.clone()),
            _ => None,
        })?;
        Ok((count, rows, live))
    }
}

// MARK: Finding things

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// How much of each ID tells them apart: 8 characters, more when two start alike.
fn unique_len<'a>(ids: impl Iterator<Item = &'a str> + Clone) -> usize {
    (8..=36).find(|n| {
        let mut seen = std::collections::HashSet::new();
        ids.clone().all(|id| seen.insert(&id[..id.len().min(*n)]))
    }).unwrap_or(36)
}

/// A thread by ID, the start of one, its title, or `last`.
fn find_thread(app: &mut App, spec: &str) -> Result<Thread, Fail> {
    let (_, threads) = app.listing()?;
    if spec == "last" || spec == "-" {
        return threads.iter().filter(|t| !t.settled).max_by(|a, b| a.updated_at.total_cmp(&b.updated_at)).cloned().ok_or_else(|| "There are no threads.".into());
    }
    let lower = spec.to_lowercase();
    if let Some(t) = threads.iter().find(|t| t.id.to_lowercase() == lower) {
        return Ok(t.clone());
    }
    let by_id: Vec<&Thread> = if spec.len() >= 4 { threads.iter().filter(|t| t.id.to_lowercase().starts_with(&lower)).collect() } else { vec![] };
    let by_title: Vec<&Thread> = threads.iter().filter(|t| t.title.to_lowercase() == lower).collect();
    let found = if by_id.is_empty() { by_title } else { by_id };
    match found.as_slice() {
        [t] => Ok((*t).clone()),
        [] => Err(Fail(1, format!("No thread {spec}. See yh threads."))),
        many => Err(Fail(1, format!("{spec} could be any of: {}", many.iter().map(|t| format!("{} “{}”", short(&t.id), t.title)).collect::<Vec<_>>().join(", ")))),
    }
}

/// The folder as the app would name it: whole, links followed.
fn real(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// A project by folder (added when it isn't one; `None`: the current folder, or the project
/// it's inside), ID, the start of one, or name.
fn find_project(app: &mut App, spec: Option<&str>) -> Result<Project, Fail> {
    let (projects, _) = app.listing()?;
    let folder = |p: &Project| p.path.as_deref().and_then(|f| real(Path::new(f)));
    if let Some(spec) = spec {
        let lower = spec.to_lowercase();
        if let Some(p) = projects.iter().find(|p| p.id.to_lowercase() == lower || p.name.to_lowercase() == lower) {
            return Ok(p.clone());
        }
        let as_path = expand(spec);
        if !as_path.is_dir() {
            let by_id: Vec<&Project> = if spec.len() >= 4 { projects.iter().filter(|p| p.id.to_lowercase().starts_with(&lower)).collect() } else { vec![] };
            return match by_id.as_slice() {
                [p] => Ok((*p).clone()),
                _ => Err(Fail(1, format!("No project or folder {spec}. See yh projects."))),
            };
        }
        let path = real(&as_path).ok_or_else(|| format!("Can't read {spec}."))?;
        if let Some(p) = projects.iter().find(|p| folder(p).as_ref() == Some(&path)) {
            return Ok(p.clone());
        }
        return add_project(app, &path);
    }
    let here = std::env::current_dir().ok().and_then(|d| real(&d)).ok_or("Can't read the current folder.")?;
    // The project the current folder is, or is inside (the deepest); a project of the whole
    // home folder (or the disk) counts only for itself.
    let home = std::env::var("HOME").ok().and_then(|h| real(Path::new(&h)));
    let inside = projects
        .iter()
        .filter_map(|p| folder(p).map(|f| (p, f)))
        .filter(|(_, f)| *f == here || (here.starts_with(f) && Some(f) != home.as_ref() && f.parent().is_some()))
        .max_by_key(|(_, f)| f.as_os_str().len());
    match inside {
        Some((p, _)) => Ok(p.clone()),
        None => add_project(app, &here),
    }
}

fn add_project(app: &mut App, path: &Path) -> Result<Project, Fail> {
    let (_, made) = app.act(|id| FromApp::AddProject { id, path: path.to_string_lossy().into_owned() })?;
    let id = made.ok_or("The app didn't say which project.")?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // The list as it is now, with the new project.
    app.listing = None;
    Ok(Project { id, name, mode: None, path: Some(path.to_string_lossy().into_owned()) })
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => Path::new(&std::env::var("HOME").unwrap_or_default()).join(rest),
        None if path == "~" => PathBuf::from(std::env::var("HOME").unwrap_or_default()),
        None => PathBuf::from(path),
    }
}

/// `-f` files as whole paths; each must be there.
fn files(args: &Args) -> Result<Vec<String>, Fail> {
    args.all("file").iter().map(|f| real(&expand(f)).map(|p| p.to_string_lossy().into_owned()).ok_or_else(|| Fail(1, format!("No such file: {f}")))).collect()
}

/// The prompt: the words given, or stdin for `-` (or when it's piped and no words are).
fn prompt(words: &[String]) -> Result<String, Fail> {
    if words.is_empty() && std::io::stdin().is_terminal() {
        return Ok(String::new());
    }
    if words.is_empty() || words == ["-"] {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("Can't read stdin: {e}"))?;
        return Ok(text.trim().to_string());
    }
    Ok(words.join(" "))
}

// MARK: Commands

fn open(args: &Args) -> Result<(), Fail> {
    let mut app = connect(args, true)?;
    let draft = args.value("draft").map(str::to_string);
    let files = files(args)?;
    let project = match args.words.first() {
        Some(folder) => Some(find_project(&mut app, Some(folder))?),
        None if draft.is_some() || !files.is_empty() => Some(find_project(&mut app, None)?),
        None => None,
    };
    app.act(|id| FromApp::Show { id, thread: None, project: project.as_ref().map(|p| p.id.clone()), draft, files })?;
    if args.on("json") {
        println!("{}", json!({ "project": project.map(|p| json!({ "id": p.id, "name": p.name, "path": p.path })) }));
    }
    Ok(())
}

fn new(args: &Args) -> Result<(), Fail> {
    let text = prompt(&args.words)?;
    let files = files(args)?;
    if text.is_empty() && files.is_empty() {
        return Err(usage("Say what to start the thread with: yh new \"…\" (or yh open for an empty one)."));
    }
    let mode = args.value("mode").map(str::to_string);
    if let Some(m) = &mode {
        if !MODES.contains(&m.as_str()) {
            return Err(usage(format!("--mode is one of {}.", MODES.join(", "))));
        }
    }
    let effort = args.value("effort").map(str::to_string);
    if let Some(e) = &effort {
        if !EFFORTS.contains(&e.as_str()) {
            return Err(usage(format!("--effort is one of {}.", EFFORTS.join(", "))));
        }
    }
    let timeout = args.timeout()?;
    let mut app = connect(args, args.on("show"))?;
    let project = find_project(&mut app, args.value("project"))?;
    let model = args.value("model").map(str::to_string);
    let (made, _) = app.act(|id| FromApp::NewThread { id, project: project.id.clone(), text: Some(text), model, effort, mode, files })?;
    let thread = made.ok_or("The app didn't say which thread it made.")?;
    if let Some(title) = args.value("title") {
        app.act(|id| FromApp::Rename { id, thread: thread.clone(), title: title.to_string() })?;
    }
    if args.on("show") {
        app.act(|id| FromApp::Show { id, thread: Some(thread.clone()), project: None, draft: None, files: vec![] })?;
    }
    let head = json!({ "thread": thread, "project": { "id": project.id, "name": project.name, "path": project.path } });
    if args.on("wait") || args.on("follow") {
        let (_, rows, live) = app.subscribe(&thread, 200)?;
        let turn = follow(&mut app, &thread, 0, rows, live, false, args.on("follow"), args.on("json"), timeout)?;
        return finish(args, head, turn);
    }
    if args.on("json") {
        println!("{head}");
    } else {
        println!("{thread}");
    }
    Ok(())
}

fn send(args: &Args) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage("Which thread? yh send THREAD \"…\""))?;
    let text = prompt(&args.words[1..])?;
    let files = files(args)?;
    if text.is_empty() && files.is_empty() {
        return Err(usage("Say what to send: yh send THREAD \"…\""));
    }
    let timeout = args.timeout()?;
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    let waits = args.on("wait") || args.on("follow");
    // Watched first, so nothing of the turn is missed.
    let before = if waits { Some(app.subscribe(&thread.id, 1)?) } else { None };
    app.act(|id| FromApp::Send { id, thread: thread.id.clone(), text, now: args.on("now"), images: vec![], files })?;
    let head = json!({ "thread": thread.id, "title": thread.title });
    match before {
        Some((count, _, live)) => {
            // Running already: the message steers into that turn, and its end is ours.
            let running = live.running;
            let turn = follow(&mut app, &thread.id, count, vec![], live, running, args.on("follow"), args.on("json"), timeout)?;
            finish(args, head, turn)
        }
        None => {
            if args.on("json") {
                println!("{head}");
            }
            Ok(())
        }
    }
}

fn wait(args: &Args) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage("Which thread? yh wait THREAD"))?;
    let timeout = args.timeout()?;
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    let (count, rows, live) = app.subscribe(&thread.id, 200)?;
    // Its last turn: from your last message on.
    let from = rows.iter().rev().find(|r| r.kind == "user").map(|r| r.seq).unwrap_or(count - rows.len() as i64);
    let rows: Vec<Row> = rows.into_iter().filter(|r| r.seq >= from).collect();
    let turn = follow(&mut app, &thread.id, from, rows, live, true, false, args.on("json"), timeout)?;
    finish(args, json!({ "thread": thread.id, "title": thread.title }), turn)
}

fn watch(args: &Args) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage("Which thread? yh watch THREAD"))?;
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    let (count, _, mut live) = app.subscribe(&thread.id, 1)?;
    let json = args.on("json");
    if !json {
        eprintln!("Watching “{}”{}. Ctrl-C to stop.", thread.title, if live.running { ", working" } else { "" });
    }
    let mut printer = Printer::new(count, json, true);
    loop {
        match app.next()? {
            Some(ToApp::RowsChanged { thread: t, rows, .. }) if t == thread.id => rows.iter().for_each(|r| printer.row(r)),
            Some(ToApp::Live { thread: t, live: l }) if t == thread.id => {
                printer.live(&live, &l);
                live = l;
            }
            _ => {}
        }
    }
}

fn read(args: &Args) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage("Which thread? yh read THREAD"))?;
    let limit = args.value("rows").map(|n| n.parse::<i64>().map_err(|_| usage("-n is a number of rows."))).transpose()?.unwrap_or(40);
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    let (count, rows, live) = app.subscribe(&thread.id, limit.clamp(1, 1000))?;
    if args.on("json") {
        println!("{}", json!({ "thread": thread.id, "title": thread.title, "count": count, "running": live.running, "waiting": waiting_json(&live), "rows": rows }));
        return Ok(());
    }
    println!("“{}” · {} rows{}", thread.title, count, if live.running { " · working" } else { "" });
    for r in &rows {
        match r.kind.as_str() {
            "user" => println!("\n── you\n{}", r.text),
            "assistant" => println!("\n── claude\n{}", r.text),
            "tool" => println!("· {}", tool_line(r)),
            "notice" | "commandOutput" if !r.text.is_empty() => println!("· {}", r.text.lines().next().unwrap_or("")),
            _ => {}
        }
    }
    print_waiting(&live);
    Ok(())
}

fn respond(args: &Args, allow: bool) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage(format!("Which thread? yh {} THREAD", if allow { "allow" } else { "deny" })))?;
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    let request = match args.words.get(1).map(String::as_str).or(args.value("request")) {
        Some(r) => r.to_string(),
        None => {
            let (_, _, live) = app.subscribe(&thread.id, 1)?;
            live.requests.first().map(|r| r.id.clone()).ok_or("Nothing waits on you there.")?
        }
    };
    app.act(|id| FromApp::Respond { id, thread: thread.id.clone(), request_id: request.clone(), allow })?;
    if args.on("json") {
        println!("{}", json!({ "thread": thread.id, "request": request, "allowed": allow }));
    }
    Ok(())
}

fn show(args: &Args) -> Result<(), Fail> {
    let mut app = connect(args, true)?;
    let thread = match args.words.first() {
        Some(spec) => Some(find_thread(&mut app, spec)?.id),
        None => None,
    };
    app.act(|id| FromApp::Show { id, thread: thread.clone(), project: None, draft: None, files: vec![] })?;
    Ok(())
}

/// An action on one thread that answers with nothing but whether it went.
fn simple(args: &Args, make: impl FnOnce(Option<String>, String) -> FromApp) -> Result<(), Fail> {
    let spec = args.words.first().ok_or_else(|| usage("Which thread?"))?;
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    app.act(|id| make(id, thread.id.clone()))?;
    if args.on("json") {
        println!("{}", json!({ "thread": thread.id }));
    }
    Ok(())
}

fn rename(args: &Args) -> Result<(), Fail> {
    let [spec, words @ ..] = args.words.as_slice() else { return Err(usage("yh rename THREAD TITLE")) };
    let title = words.join(" ");
    if title.trim().is_empty() {
        return Err(usage("yh rename THREAD TITLE"));
    }
    let mut app = connect(args, false)?;
    let thread = find_thread(&mut app, spec)?;
    app.act(|id| FromApp::Rename { id, thread: thread.id.clone(), title })?;
    Ok(())
}

fn add(args: &Args) -> Result<(), Fail> {
    let folder = args.words.first().ok_or_else(|| usage("Which folder? yh add FOLDER"))?;
    if !expand(folder).is_dir() {
        return Err(Fail(1, format!("No such folder: {folder}")));
    }
    let mut app = connect(args, false)?;
    let p = find_project(&mut app, Some(folder))?;
    if args.on("json") {
        println!("{}", json!({ "id": p.id, "name": p.name, "path": p.path }));
    } else {
        println!("{}", p.id);
    }
    Ok(())
}

fn projects(args: &Args) -> Result<(), Fail> {
    let mut app = connect(args, false)?;
    let (projects, threads) = app.listing()?;
    if args.on("json") {
        let list: Vec<Value> = projects
            .iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "path": p.path, "mode": p.mode, "threads": threads.iter().filter(|t| t.project_id == p.id && !t.settled).count() }))
            .collect();
        println!("{}", Value::Array(list));
        return Ok(());
    }
    let width = projects.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
    let n = unique_len(projects.iter().map(|p| p.id.as_str()));
    for p in &projects {
        println!("{}  {:width$}  {}", &p.id[..p.id.len().min(n)], p.name, p.path.as_deref().unwrap_or(""));
    }
    Ok(())
}

fn threads(args: &Args) -> Result<(), Fail> {
    let mut app = connect(args, false)?;
    let project = match args.value("project") {
        Some(p) => Some(find_project(&mut app, Some(p))?),
        None => None,
    };
    let (projects, threads) = app.listing()?;
    let name = |id: &str| projects.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
    let shown: Vec<&Thread> = threads.iter().filter(|t| (args.on("all") || !t.settled) && project.as_ref().is_none_or(|p| p.id == t.project_id)).collect();
    if args.on("json") {
        let list: Vec<Value> = shown
            .iter()
            .map(|t| {
                let mut v = serde_json::to_value(t).expect("threads serialize");
                v["project_name"] = Value::from(name(&t.project_id));
                v["state"] = Value::from(state(t));
                v
            })
            .collect();
        println!("{}", Value::Array(list));
        return Ok(());
    }
    let width = shown.iter().map(|t| name(&t.project_id).chars().count()).max().unwrap_or(0);
    let n = unique_len(threads.iter().map(|t| t.id.as_str()));
    for t in shown {
        println!("{}  {:8}  {:width$}  {}", &t.id[..t.id.len().min(n)], state(t), name(&t.project_id), t.title);
    }
    Ok(())
}

fn state(t: &Thread) -> &'static str {
    if t.waiting {
        "waiting"
    } else if t.running {
        "working"
    } else if t.settled {
        "settled"
    } else {
        "idle"
    }
}

fn status(args: &Args) -> Result<(), Fail> {
    let socket = yh_proto::local_socket(&data_dir());
    let mut quiet = Args::default();
    quiet.switches.push("no-launch");
    let app = connect(&quiet, false);
    let (running, server, version, protocol) = match &app {
        Ok(App { hello: ToApp::Hello { server, version, protocol, .. }, .. }) => (true, Some(server.clone()), version.clone(), Some(*protocol)),
        _ => (false, None, None, None),
    };
    if args.on("json") {
        println!("{}", json!({ "running": running, "mac": server, "version": version, "protocol": protocol, "data": data_dir(), "socket": socket }));
    } else if running {
        println!("YourHarness {} is running on {} · data {}", version.unwrap_or_default(), server.unwrap_or_default(), data_dir().display());
    } else {
        println!("YourHarness isn't running · data {}", data_dir().display());
    }
    if !running {
        std::process::exit(5);
    }
    Ok(())
}

// MARK: Following a turn

/// How a turn followed ended.
enum Ended {
    Done,
    Waiting(Live),
    TimedOut,
}

struct Turn {
    ended: Ended,
    reply: String,
}

/// Follows `thread` from row `from` until its turn is over or it waits on you; `rows` and
/// `live`, what's already known.
/// `began`: a turn is known to be under way, or over, so the thread's standing idle ends it.
fn follow(app: &mut App, thread: &str, from: i64, rows: Vec<Row>, mut live: Live, began: bool, print: bool, json: bool, timeout: Option<Duration>) -> Result<Turn, Fail> {
    let started = Instant::now();
    let mut held: BTreeMap<i64, Row> = rows.into_iter().filter(|r| r.seq >= from).map(|r| (r.seq, r)).collect();
    let mut printer = Printer::new(from, json, print);
    held.values().for_each(|r| printer.row(r));
    let mut began = began || live.running;
    let reply = |held: &BTreeMap<i64, Row>| held.values().filter(|r| r.kind == "assistant" && !r.text.trim().is_empty()).map(|r| r.text.trim().to_string()).collect::<Vec<_>>().join("\n\n");
    loop {
        let waits = !live.requests.is_empty() || !live.questions.is_empty();
        if waits && !print {
            return Ok(Turn { ended: Ended::Waiting(live), reply: reply(&held) });
        }
        // Over: it ran and stopped, or it finished before we looked (a row ends the turn).
        let ended_row = held.values().any(|r| r.turn_ended_at.is_some() && r.kind != "user");
        if !live.running && !waits && (began || ended_row) {
            return Ok(Turn { ended: Ended::Done, reply: reply(&held) });
        }
        if timeout.is_some_and(|t| started.elapsed() > t) {
            return Ok(Turn { ended: Ended::TimedOut, reply: reply(&held) });
        }
        match app.next()? {
            Some(ToApp::RowsChanged { thread: t, rows, .. }) if t == thread => {
                for r in rows.into_iter().filter(|r| r.seq >= from) {
                    printer.row(&r);
                    held.insert(r.seq, r);
                }
            }
            Some(ToApp::Live { thread: t, live: l }) if t == thread => {
                printer.live(&live, &l);
                began |= l.running;
                live = l;
            }
            Some(ToApp::ThreadRemoved { id }) if id == thread => return Err("The thread was deleted.".into()),
            _ => {}
        }
    }
}

/// What a followed turn came to: the reply printed (unless it streamed), and the exit code.
fn finish(args: &Args, mut head: Value, turn: Turn) -> Result<(), Fail> {
    let (state, code) = match &turn.ended {
        Ended::Done => ("done", 0),
        Ended::Waiting(_) => ("waiting", 3),
        Ended::TimedOut => ("timeout", 4),
    };
    if args.on("json") {
        head["state"] = Value::from(state);
        head["reply"] = Value::from(turn.reply.clone());
        if let Ended::Waiting(live) = &turn.ended {
            head["waiting"] = waiting_json(live);
        }
        println!("{head}");
    } else {
        if !args.on("follow") && !turn.reply.is_empty() {
            println!("{}", turn.reply);
        }
        if let Ended::Waiting(live) = &turn.ended {
            print_waiting(live);
        }
        if let Ended::TimedOut = turn.ended {
            eprintln!("yh: still working (timed out); yh wait {} picks it up.", short(head["thread"].as_str().unwrap_or("")));
        }
    }
    match code {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

fn waiting_json(live: &Live) -> Value {
    let requests: Vec<Value> = live.requests.iter().map(|r| json!({ "id": r.id, "tool": r.tool, "description": r.description, "input": r.input })).collect();
    let questions: Vec<Value> = live.questions.iter().map(|q| json!({ "id": q.id, "server": q.server, "message": q.message })).collect();
    json!({ "requests": requests, "questions": questions })
}

fn print_waiting(live: &Live) {
    for r in &live.requests {
        let what = if r.description.is_empty() { r.tool.clone() } else { format!("{}: {}", r.tool, r.description) };
        eprintln!("Waiting on you: {what} (request {}). yh allow / yh deny, or answer in the app.", r.id);
    }
    for q in &live.questions {
        eprintln!("Waiting on you: {} asks “{}”. Answer in the app.", q.server, q.message);
    }
}

/// A tool call in a line: its name and what it's about.
fn tool_line(r: &Row) -> String {
    let name = r.tool_name.as_deref().unwrap_or("tool");
    let input: Value = r.tool_input.as_deref().and_then(|i| serde_json::from_str(i).ok()).unwrap_or_default();
    let about = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"].iter().find_map(|k| input[k].as_str()).unwrap_or("");
    let about: String = about.lines().next().unwrap_or("").chars().take(100).collect();
    let failed = if r.tool_is_error == Some(true) { " (failed)" } else { "" };
    if about.is_empty() { format!("{name}{failed}") } else { format!("{name} {about}{failed}") }
}

/// Prints a turn as it comes: Claude's words to stdout as they're written, tools and what
/// waits on you to stderr; with `--json`, one object a line.
struct Printer {
    from: i64,
    json: bool,
    on: bool,
    /// How much of each reply row is printed.
    printed: BTreeMap<i64, usize>,
    tools: std::collections::BTreeSet<i64>,
    last: Option<i64>,
}

impl Printer {
    fn new(from: i64, json: bool, on: bool) -> Printer {
        Printer { from, json, on, printed: BTreeMap::new(), tools: Default::default(), last: None }
    }

    fn row(&mut self, r: &Row) {
        if !self.on || r.seq < self.from {
            return;
        }
        match r.kind.as_str() {
            "assistant" => {
                let done = self.printed.get(&r.seq).copied().unwrap_or(0);
                let Some(delta) = r.text.get(done..).filter(|d| !d.is_empty()) else { return };
                if self.json {
                    println!("{}", json!({ "event": "text", "seq": r.seq, "delta": delta }));
                } else {
                    if self.last.is_some_and(|l| l != r.seq) && done == 0 {
                        println!("\n");
                    }
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
                self.printed.insert(r.seq, r.text.len());
                self.last = Some(r.seq);
            }
            "tool" if self.tools.insert(r.seq) => {
                if self.json {
                    println!("{}", json!({ "event": "tool", "seq": r.seq, "name": r.tool_name, "input": r.tool_input }));
                } else {
                    if self.last.is_some() && self.printed.contains_key(&self.last.unwrap()) {
                        println!();
                    }
                    eprintln!("· {}", tool_line(r));
                    self.last = Some(r.seq);
                }
            }
            _ => {}
        }
    }

    fn live(&mut self, was: &Live, now: &Live) {
        if !self.on {
            return;
        }
        let ids = |l: &Live| l.requests.iter().map(|r| r.id.clone()).chain(l.questions.iter().map(|q| q.id.clone())).collect::<Vec<_>>();
        if ids(was) != ids(now) && !ids(now).is_empty() {
            if self.json {
                println!("{}", json!({ "event": "waiting", "waiting": waiting_json(now) }));
            } else {
                println!();
                print_waiting(now);
            }
        }
        if was.running != now.running {
            if self.json {
                println!("{}", json!({ "event": if now.running { "working" } else { "idle" } }));
            } else if !now.running {
                println!();
            }
        }
    }
}
