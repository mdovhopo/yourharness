//! The agent CLIs Settings → Agents lists: where each one is, whether it
//! answers `--version`, and for `claude` the account it's signed in with (`claude auth status
//! --json`, the official binary's own answer; nothing of its credentials is read). `gh` and `glab`
//! are probed the same way.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub struct Cli {
    pub id: &'static str,
    pub name: &'static str,
    pub executable: &'static str,
    pub supported: bool,
    pub install_url: &'static str,
}

pub const ALL: [Cli; 5] = [
    Cli { id: "claude", name: "Claude Code", executable: "claude", supported: true, install_url: "https://docs.claude.com/en/docs/claude-code/setup" },
    Cli { id: "codex", name: "Codex", executable: "codex", supported: false, install_url: "https://github.com/openai/codex" },
    Cli { id: "cursor", name: "Cursor Agent", executable: "cursor-agent", supported: false, install_url: "https://cursor.com/cli" },
    Cli { id: "gemini", name: "Gemini CLI", executable: "gemini", supported: false, install_url: "https://github.com/google-gemini/gemini-cli" },
    Cli { id: "opencode", name: "OpenCode", executable: "opencode", supported: false, install_url: "https://opencode.ai" },
];

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    NotFound,
    Broken(String),
    Ready(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub path: Option<PathBuf>,
    pub state: State,
}

impl Status {
    pub fn is_ready(&self) -> bool {
        matches!(self.state, State::Ready(_))
    }

    pub fn describe(&self) -> String {
        match &self.state {
            State::NotFound => "Not found".into(),
            State::Broken(reason) => format!("Not working: {reason}"),
            State::Ready(version) => version.clone(),
        }
    }
}

/// An executable on `YH_PATH` (as test runs pass it), else the login shell's `PATH` once read,
/// else ours, and the usual places.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var("YH_PATH").ok().or_else(crate::shell_env::loaded_path).or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    let extra = [format!("{home}/.local/bin"), format!("{home}/.claude/local"), "/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    path.split(':').map(str::to_string).chain(extra).map(|d| Path::new(&d).join(name)).find(|p| p.is_file())
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// Runs a command for at most 8 seconds: (succeeded, stdout, stderr), or None when it didn't answer.
fn run(executable: &Path, args: &[&str]) -> Option<(bool, String, String)> {
    let mut child = Command::new(executable).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    Some((output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned()))
}

/// Finds an executable (or takes the chosen one) and asks it for its version. Blocks: call aside.
pub fn probe(executable: &str, chosen: Option<&Path>) -> Status {
    let Some(path) = chosen.map(Path::to_path_buf).or_else(|| which(executable)) else {
        return Status { path: None, state: State::NotFound };
    };
    let state = match run(&path, &["--version"]) {
        None => State::Broken("Didn't respond to --version within 8 seconds.".into()),
        Some((true, out, _)) if first_line(&out).is_some() => State::Ready(first_line(&out).unwrap()),
        Some((_, out, err)) => State::Broken(first_line(&err).or_else(|| first_line(&out)).unwrap_or_else(|| "Exited with an error.".into())),
    };
    Status { path: Some(path), state }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Account {
    pub logged_in: bool,
    pub auth_method: Option<String>,
    pub email: Option<String>,
    pub org_name: Option<String>,
    pub subscription_type: Option<String>,
}

impl Account {
    pub fn from_json(v: &Value) -> Option<Account> {
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        Some(Account { logged_in: v["loggedIn"].as_bool()?, auth_method: s("authMethod"), email: s("email"), org_name: s("orgName"), subscription_type: s("subscriptionType") })
    }

    pub fn plan_name(&self) -> Option<String> {
        let t = self.subscription_type.as_deref()?.to_lowercase();
        Some(match t.as_str() {
            "max" => "Claude Max".into(),
            "pro" => "Claude Pro".into(),
            "team" => "Claude Team".into(),
            "enterprise" => "Claude Enterprise".into(),
            other => other[..1].to_uppercase() + &other[1..],
        })
    }

    pub fn sign_in_method(&self) -> Option<String> {
        Some(match self.auth_method.as_deref()? {
            "claude.ai" => "Claude subscription".into(),
            "api_key" | "apiKey" => "API key".into(),
            other => other.into(),
        })
    }
}

/// What `claude auth status --json` says. Blocks: call aside.
pub fn account(claude: &Path) -> Option<Account> {
    let (_, out, _) = run(claude, &["auth", "status", "--json"])?;
    Account::from_json(&serde_json::from_str(&out).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_accounts() {
        let a = Account::from_json(&serde_json::json!({"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.c", "subscriptionType": "max"})).unwrap();
        assert_eq!(a.plan_name().as_deref(), Some("Claude Max"));
        assert_eq!(a.sign_in_method().as_deref(), Some("Claude subscription"));
        assert!(Account::from_json(&serde_json::json!({"nope": 1})).is_none());
    }

    #[test]
    fn probes_a_missing_executable() {
        assert_eq!(probe("yh-no-such-tool", None).state, State::NotFound);
        let s = probe("sh", Some(Path::new("/usr/bin/true")));
        assert!(matches!(s.state, State::Broken(_)), "{s:?}");
    }
}
