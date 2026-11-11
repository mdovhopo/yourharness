//! The environment `claude` and the tools it runs get: the login shell's, as a fresh Terminal
//! window has it. An app opened from the Dock inherits only
//! launchd's minimal `PATH`, so without this `claude` and everything it runs (`npm`, `cargo`,
//! `gh`…) would be missing. Read once, aside from the main thread, with a timeout, and kept.
//!
//! The shell starts from a minimal environment, so nothing the app inherited leaks into its
//! agents; and Claude Code's own session variables are never passed on, so an app launched from
//! inside a Claude Code session doesn't tie every thread to that session.
//!
//! `YH_PATH` (test runs) skips the shell entirely: a test must never start the user's login
//! shell, whose rc files can reach into `~/Downloads` and put a privacy prompt on the screen.
//!
//! The Claude account in use (`accounts.rs`) is a `CLAUDE_CONFIG_DIR` on top: every `claude`
//! started from then on, and every command a thread runs, gets the active account's folder.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MARKER: &str = "__YOURHARNESS_ENV__";

/// Set by a running Claude Code session for its own children. Never passed on.
const SESSION_VARIABLES: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// What a login shell gets from launchd; everything else comes from the user's profile.
const BASE_VARIABLES: &[&str] = &["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "SSH_AUTH_SOCK", "__CF_USER_TEXT_ENCODING"];

struct Loaded {
    env: Mutex<Option<HashMap<String, String>>>,
    ready: Condvar,
}

fn state() -> &'static Loaded {
    static STATE: OnceLock<Loaded> = OnceLock::new();
    STATE.get_or_init(|| Loaded { env: Mutex::new(None), ready: Condvar::new() })
}

/// Starts reading the environment on a thread of its own (once; later calls do nothing).
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| {
            let env = load(&std::env::vars().collect());
            let s = state();
            *s.env.lock().unwrap() = Some(env);
            s.ready.notify_all();
        });
    });
}

fn account_cell() -> &'static Mutex<Option<String>> {
    static ACCOUNT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ACCOUNT.get_or_init(|| Mutex::new(None))
}

/// The Claude Code config folder of the account in use; `None`: Claude Code's own (the shell's
/// `CLAUDE_CONFIG_DIR`, else `~/.claude`).
pub fn set_claude_config(dir: Option<String>) {
    *account_cell().lock().unwrap() = dir;
}

pub fn claude_config() -> Option<String> {
    account_cell().lock().unwrap().clone()
}

/// The environment, with the account in use. Waits for the read when it's under way (at most
/// the shell's timeouts, seconds after launch at worst); starts it when nobody has.
pub fn environment() -> HashMap<String, String> {
    let mut env = shell_environment();
    if let Some(dir) = claude_config() {
        env.insert("CLAUDE_CONFIG_DIR".into(), dir);
    }
    env
}

/// A command for one account in particular (`None`: Claude Code's own folder), whichever is in use.
pub fn apply_claude_config(command: &mut Command, dir: Option<&std::path::Path>) {
    match dir.map(|d| d.as_os_str().to_owned()).or_else(|| shell_environment().get("CLAUDE_CONFIG_DIR").map(Into::into)) {
        Some(d) => command.env("CLAUDE_CONFIG_DIR", d),
        None => command.env_remove("CLAUDE_CONFIG_DIR"),
    };
}

/// The environment as the shell gave it, no account on top.
fn shell_environment() -> HashMap<String, String> {
    start();
    let s = state();
    let mut env = s.env.lock().unwrap();
    let deadline = Instant::now() + Duration::from_secs(9);
    while env.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return without_session_variables(std::env::vars().collect());
        }
        env = s.ready.wait_timeout(env, left).unwrap().0;
    }
    env.clone().unwrap_or_default()
}

/// Its `PATH` if it's been read, without waiting (for finding executables).
pub fn loaded_path() -> Option<String> {
    state().env.lock().unwrap().as_ref().and_then(|e| e.get("PATH").cloned())
}

/// Gives a command the environment (and only it).
pub fn apply(command: &mut Command) {
    command.env_clear().envs(environment());
}

/// The environment from `inherited` (the app's own): `YH_PATH`'s when set, else the login shell's.
pub fn load(inherited: &HashMap<String, String>) -> HashMap<String, String> {
    // Development: what the test run was given, with its PATH, no shell started.
    if let Some(path) = inherited.get("YH_PATH").filter(|p| !p.is_empty()) {
        set_outcome("YH_PATH given (a development run): the login shell isn't read.".into());
        let mut env = without_session_variables(inherited.clone());
        env.insert("PATH".into(), path.clone());
        return env;
    }
    let shell = inherited.get("SHELL").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "/bin/zsh".into());
    let mut base: HashMap<String, String> =
        inherited.iter().filter(|(k, _)| BASE_VARIABLES.contains(&k.as_str()) || k.starts_with("LC_")).map(|(k, v)| (k.clone(), v.clone())).collect();
    base.insert("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let command = format!("printf '{MARKER}'; env -0");
    // Interactive first (PATH is often set in .zshrc), then login only if the rc files hang or
    // fail. Killed with SIGKILL on timeout: interactive shells ignore SIGTERM.
    for (flags, timeout) in [("-ilc", 4.0), ("-lc", 3.0)] {
        let began = Instant::now();
        if let Some(out) = run(&shell, flags, &command, &base, Duration::from_secs_f64(timeout)) {
            if let Some(env) = parse(&out) {
                set_outcome(format!("Read from your shell ({shell} {flags}) in {:.1} s.", began.elapsed().as_secs_f64()));
                return without_session_variables(env);
            }
        }
    }
    // The shell didn't answer in time (a slow rc file) or printed no environment: the app's own,
    // whose PATH from launchd has none of the usual tool folders, so `gh`, `glab` and Homebrew's
    // tools would go missing. The usual folders are added.
    set_outcome(format!("Your shell ({shell}) didn't give its environment within 7 s, so the app's own is used, with the usual tool folders added to PATH."));
    let mut env = without_session_variables(inherited.clone());
    let path = env.get("PATH").cloned().unwrap_or_default();
    env.insert("PATH".into(), with_usual_folders(&path, inherited.get("HOME").map(String::as_str).unwrap_or("")));
    env
}

/// `path` with the folders tools are usually installed in appended, when missing.
pub fn with_usual_folders(path: &str, home: &str) -> String {
    let mut parts: Vec<String> = path.split(':').filter(|p| !p.is_empty()).map(str::to_string).collect();
    for extra in ["/opt/homebrew/bin".to_string(), "/opt/homebrew/sbin".into(), "/usr/local/bin".into(), format!("{home}/.local/bin"), format!("{home}/.claude/local"), "/usr/bin".into(), "/bin".into(), "/usr/sbin".into(), "/sbin".into()] {
        if !parts.contains(&extra) {
            parts.push(extra);
        }
    }
    parts.join(":")
}

fn outcome_cell() -> &'static Mutex<Option<String>> {
    static OUTCOME: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    OUTCOME.get_or_init(|| Mutex::new(None))
}

fn set_outcome(text: String) {
    *outcome_cell().lock().unwrap() = Some(text);
}

/// How the environment was read, for Settings → Agents; None until it has been.
pub fn outcome() -> Option<String> {
    outcome_cell().lock().unwrap().clone()
}

fn run(shell: &str, flags: &str, command: &str, env: &HashMap<String, String>, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = Command::new(shell)
        .args([flags, command])
        .env_clear()
        .envs(env)
        .current_dir(env.get("HOME").map(String::as_str).unwrap_or("/"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = vec![];
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // What it printed may still hold the environment; a child of the shell keeping
                // the pipe open would hold the reader, so it isn't waited for.
                return None;
            }
        }
    }
    reader.join().ok()
}

/// Without Claude Code's session variables.
pub fn without_session_variables(env: HashMap<String, String>) -> HashMap<String, String> {
    env.into_iter()
        .filter(|(k, _)| !SESSION_VARIABLES.contains(&k.as_str()) && !k.starts_with("CLAUDE_CODE_SESSION") && !k.starts_with("CLAUDE_CODE_MESSAGING"))
        .collect()
}

/// `env -0`'s output after the marker (rc files may print before it): None without the marker
/// or any variable.
pub fn parse(output: &[u8]) -> Option<HashMap<String, String>> {
    let text = String::from_utf8_lossy(output);
    let at = text.find(MARKER)?;
    let env: HashMap<String, String> = text[at + MARKER.len()..]
        .split('\0')
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect();
    (!env.is_empty()).then_some(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usual_folders_are_added_once() {
        let p = with_usual_folders("/usr/bin:/bin:/opt/homebrew/bin", "/Users/x");
        assert!(p.starts_with("/usr/bin:/bin:/opt/homebrew/bin:"));
        assert_eq!(p.matches("/opt/homebrew/bin").count(), 1);
        assert!(p.contains("/usr/local/bin") && p.contains("/Users/x/.local/bin"));
    }

    #[test]
    fn parses_after_the_marker() {
        let out = format!("Welcome to zsh!\n{MARKER}PATH=/opt/homebrew/bin:/usr/bin\0HOME=/Users/a\0MULTI=one\ntwo\0EQ=a=b\0\0");
        let env = parse(out.as_bytes()).unwrap();
        assert_eq!(env["PATH"], "/opt/homebrew/bin:/usr/bin");
        assert_eq!(env["MULTI"], "one\ntwo");
        assert_eq!(env["EQ"], "a=b");
        assert!(!env.contains_key("Welcome to zsh!\n"));
        assert_eq!(env.len(), 4);
    }

    #[test]
    fn no_marker_or_nothing_after_it_is_none() {
        assert!(parse(b"PATH=/usr/bin\0").is_none());
        assert!(parse(format!("{MARKER}").as_bytes()).is_none());
        assert!(parse(format!("{MARKER}garbage\0").as_bytes()).is_none());
    }

    #[test]
    fn strips_session_variables() {
        let env: HashMap<String, String> = [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_NEW_THING",
            "CLAUDE_CODE_MESSAGING_X",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_ENABLE_TODO_TOOLS",
            "PATH",
        ]
        .iter()
        .map(|k| (k.to_string(), "1".to_string()))
        .collect();
        let mut kept: Vec<String> = without_session_variables(env).into_keys().collect();
        kept.sort();
        assert_eq!(kept, ["CLAUDE_CODE_ENABLE_TODO_TOOLS", "CLAUDE_CONFIG_DIR", "PATH"]);
    }

    #[test]
    fn yh_path_never_starts_a_shell() {
        // A shell that doesn't exist: reaching for it would fall back to `inherited` whole.
        let inherited: HashMap<String, String> =
            [("YH_PATH", "/x/bin:/usr/bin"), ("SHELL", "/nonexistent/shell"), ("CLAUDECODE", "1"), ("HOME", "/h")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = load(&inherited);
        assert_eq!(env["PATH"], "/x/bin:/usr/bin");
        assert!(!env.contains_key("CLAUDECODE"));
        assert_eq!(env["HOME"], "/h");
    }

    #[test]
    fn reads_a_shell() {
        // /bin/sh as the login shell, with a minimal environment: never the user's own shell.
        let inherited: HashMap<String, String> =
            [("SHELL", "/bin/sh"), ("HOME", "/tmp"), ("CLAUDECODE", "1")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = load(&inherited);
        assert!(env.get("PATH").is_some_and(|p| p.contains("/usr/bin")));
        assert!(!env.contains_key("CLAUDECODE"));
    }
}
