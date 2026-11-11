//! Claude accounts: more than one Claude subscription on this Mac, one used at a time, switched
//! by hand. Each account is a Claude Code config folder (`CLAUDE_CONFIG_DIR`, as Claude Code's
//! docs suggest for several accounts), signed in through `claude auth login` itself: the app
//! never sees a credential. The main account is Claude Code's own folder; the others are made
//! by the app in its data folder, with the main folder's settings, instructions, skills,
//! plugins and conversation history linked in, so a thread goes on where it was on any of them.
//! Only the sign-in, and the MCP servers you added for yourself (`.claude.json`), stay apart.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// The main account: Claude Code's own folder.
pub const MAIN: &str = "main";

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub id: String,
    /// Its config folder; `None` for the main account (Claude Code's own).
    pub dir: Option<PathBuf>,
    /// Off: kept, signed in, but never used until it's on again.
    pub enabled: bool,
}

impl Account {
    pub fn main() -> Account {
        Account { id: MAIN.into(), dir: None, enabled: true }
    }

    pub fn is_main(&self) -> bool {
        self.id == MAIN
    }
}

/// Saved as `claudeAccounts`: `[{id, dir, enabled}]`, the main account first, always there.
pub fn to_saved(accounts: &[Account]) -> Value {
    Value::Array(accounts.iter().map(|a| json!({"id": a.id, "dir": a.dir.as_ref().map(|d| d.to_string_lossy()), "enabled": a.enabled})).collect())
}

pub fn from_saved(saved: Option<&Value>) -> Vec<Account> {
    let mut list: Vec<Account> = saved
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    let id = v["id"].as_str().filter(|s| !s.is_empty())?.to_string();
                    let dir = v["dir"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
                    // Another account's folder is what it is: without one it can't be used.
                    if id != MAIN && dir.is_none() {
                        return None;
                    }
                    Some(Account { dir: if id == MAIN { None } else { dir }, enabled: v["enabled"].as_bool().unwrap_or(true), id })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    list.retain(|a| seen.insert(a.id.clone()));
    match list.iter().position(Account::is_main) {
        Some(0) => {}
        Some(i) => {
            let main = list.remove(i);
            list.insert(0, main);
        }
        None => list.insert(0, Account::main()),
    }
    // At least one is on.
    if !list.iter().any(|a| a.enabled) {
        list[0].enabled = true;
    }
    list
}

/// What's shared with the main folder: settings, instructions, skills, agents, commands,
/// plugins, and the conversation history (sessions, checkpoints, plans, tasks), so a thread's
/// session resumes on any account. Not the sign-in, `.claude.json` (your own MCP servers,
/// onboarding), caches or telemetry.
pub const SHARED: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "settings.local.json",
    "keybindings.json",
    "agents",
    "commands",
    "skills",
    "plugins",
    "output-styles",
    "hooks",
    "projects",
    "file-history",
    "plans",
    "tasks",
    "todos",
    "history.jsonl",
    "paste-cache",
];

/// Made in the main folder when missing, so the history is shared from the first session on.
const HISTORY: &[&str] = &["projects", "file-history", "plans", "tasks"];

/// An account's folder made (or brought up to date), its shared parts linked to `main`'s.
/// What it already has of its own stays.
pub fn prepare(dir: &Path, main: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for name in SHARED {
        let link = dir.join(name);
        if link.symlink_metadata().is_ok() {
            continue;
        }
        let target = main.join(name);
        if !target.exists() {
            if !HISTORY.contains(name) {
                continue;
            }
            std::fs::create_dir_all(&target)?;
        }
        std::os::unix::fs::symlink(&target, &link)?;
    }
    Ok(())
}

/// A new account's folder under `root` (the app's `claude-accounts`), and its id.
pub fn new_folder(root: &Path) -> (String, PathBuf) {
    let base = format!("account-{}", (crate::library::now_ms() / 1000.) as i64);
    let mut id = base.clone();
    let mut n = 2;
    while root.join(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    (id.clone(), root.join(id))
}

/// An account's folder deleted, only when the app made it (under `root`): the links go, never
/// what they point to.
pub fn remove_folder(dir: &Path, root: &Path) -> std::io::Result<()> {
    if !dir.starts_with(root) || dir == root {
        return Err(std::io::Error::other("Not a folder YourHarness made."));
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = path.symlink_metadata()?;
        if meta.file_type().is_symlink() || meta.is_file() {
            std::fs::remove_file(&path)?;
        } else if meta.is_dir() {
            // Its own: never one of the shared names, which are only ever links here.
            if SHARED.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            std::fs::remove_dir_all(&path)?;
        }
    }
    let _ = std::fs::remove_dir(dir);
    Ok(())
}

/// A sign-in under way: `claude auth login` for one folder, which opens the browser and waits.
pub struct Login {
    child: Arc<Mutex<Child>>,
    /// The sign-in page, once printed: to open again if the browser didn't.
    pub url: Arc<Mutex<Option<String>>>,
}

impl Login {
    /// Starts it (`None`: Claude Code's own folder). Signing in finishes in the browser.
    pub fn start(executable: &Path, path_env: Option<&str>, dir: Option<&Path>) -> Result<Login, String> {
        let mut command = Command::new(executable);
        command.args(["auth", "login", "--claudeai"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        crate::shell_env::apply(&mut command);
        crate::shell_env::apply_claude_config(&mut command, dir);
        if let Some(path) = path_env {
            command.env("PATH", path);
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().unwrap();
        let url = Arc::new(Mutex::new(None));
        let found = url.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(at) = line.find("https://") {
                    let link = line[at..].split_whitespace().next().unwrap_or_default().to_string();
                    found.lock().unwrap().get_or_insert(link);
                }
            }
        });
        Ok(Login { child: Arc::new(Mutex::new(child)), url })
    }

    /// Blocks until it's over (10 minutes at most): whether it says it signed in.
    pub fn wait(&self) -> bool {
        let started = std::time::Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.lock().unwrap().try_wait() {
                return status.success();
            }
            if started.elapsed() > std::time::Duration::from_secs(600) {
                self.cancel();
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    pub fn cancel(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A handle to cancel it from elsewhere while `wait` blocks.
    pub fn handle(&self) -> Login {
        Login { child: self.child.clone(), url: self.url.clone() }
    }
}

/// `claude auth logout` for one folder: its sign-in removed from this Mac. Blocks (20 s at most).
pub fn logout(executable: &Path, path_env: Option<&str>, dir: &Path) -> Result<(), String> {
    let mut command = Command::new(executable);
    command.args(["auth", "logout"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::shell_env::apply(&mut command);
    crate::shell_env::apply_claude_config(&mut command, Some(dir));
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return Ok(());
        }
        if started.elapsed() > std::time::Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Claude Code didn't sign out in time.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_account_is_always_first_and_one_is_on() {
        assert_eq!(from_saved(None), vec![Account::main()]);
        let saved = json!([
            {"id": "account-1", "dir": "/x/account-1", "enabled": false},
            {"id": "main", "enabled": false},
            {"id": "broken"},
            {"id": "account-1", "dir": "/dup"},
            {"newField": 1}
        ]);
        let list = from_saved(Some(&saved));
        assert_eq!(list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["main", "account-1"]);
        assert!(list[0].enabled, "none on: the main account is");
        assert_eq!(list[1].dir.as_deref(), Some(Path::new("/x/account-1")));
        assert_eq!(from_saved(Some(&to_saved(&list))), list);
    }

    #[test]
    fn a_folder_links_the_shared_parts_and_removing_it_keeps_them() {
        let root = std::env::temp_dir().join(format!("yh-accounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("main");
        std::fs::create_dir_all(main.join("skills/one")).unwrap();
        std::fs::write(main.join("settings.json"), "{}").unwrap();
        std::fs::write(main.join("skills/one/SKILL.md"), "x").unwrap();
        let accounts = root.join("claude-accounts");
        let (id, dir) = new_folder(&accounts);
        assert!(id.starts_with("account-"));
        prepare(&dir, &main).unwrap();
        assert!(dir.join("settings.json").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(dir.join("skills/one/SKILL.md")).unwrap(), "x");
        // The history is made in the main folder, so it's shared from the start.
        assert!(main.join("projects").is_dir() && dir.join("projects").symlink_metadata().unwrap().file_type().is_symlink());
        // What's missing in the main folder, and not history, isn't linked.
        assert!(dir.join("CLAUDE.md").symlink_metadata().is_err());
        // Its own files stay on a second prepare.
        std::fs::write(dir.join(".claude.json"), "{}").unwrap();
        prepare(&dir, &main).unwrap();
        assert!(dir.join(".claude.json").is_file());
        // Never outside the accounts folder.
        assert!(remove_folder(&main, &accounts).is_err());
        remove_folder(&dir, &accounts).unwrap();
        assert!(!dir.exists());
        assert_eq!(std::fs::read_to_string(main.join("skills/one/SKILL.md")).unwrap(), "x");
        assert!(main.join("settings.json").is_file() && main.join("projects").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }
}
