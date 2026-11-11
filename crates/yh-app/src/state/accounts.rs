//! Claude accounts (`yh_core::accounts`): added, signed in through `claude auth login`,
//! turned off, removed, and one per thread. A thread keeps its account (its prompt cache warm
//! there) until it's moved, which starts its `claude` over on the other account, an idle one now
//! and a busy one once its turn is over, resuming the same session (the history is shared). New
//! threads take the account chosen for them. Saved as `claudeAccounts`, `claudeActiveAccount`
//! (new threads') and `claudeThreadAccounts` (`{thread: account}`).

use crate::state::limits::AccountState;
use crate::state::workspace::Workspace;
use gpui_kit::Context;
use serde_json::Value;
use yh_core::accounts::Account;
use yh_engine::Command;

fn log(line: String) {
    if std::env::var("YH_DATA_DIR").is_ok() {
        eprintln!("ACCOUNT {line}");
    }
}

impl Workspace {
    fn save_accounts(&self) {
        let list: Vec<Account> = self.accounts.iter().map(|a| a.account.clone()).collect();
        self.library.set_setting("claudeAccounts", &yh_core::accounts::to_saved(&list));
        self.library.set_setting("claudeActiveAccount", &Value::from(self.active_account.clone()));
    }

    /// Where the app keeps the folders of the accounts it adds.
    pub fn accounts_root(&self) -> std::path::PathBuf {
        self.data_dir.join("claude-accounts")
    }

    /// New threads use this account from now on; threads that have one keep theirs.
    pub fn switch_account(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == self.active_account || !self.account(id).is_some_and(|a| a.account.enabled) {
            return;
        }
        self.active_account = id.to_string();
        log(format!("new threads: {id} ({})", self.account_label(id)));
        yh_core::shell_env::set_claude_config(self.active().account.dir.as_ref().map(|d| d.to_string_lossy().into_owned()));
        self.save_accounts();
        self.save_readings();
        cx.notify();
    }

    /// The account a thread runs on: its own while that's on, else the one new threads use. A
    /// side chat is its thread's.
    pub fn account_of(&self, thread: &str) -> &AccountState {
        let thread = thread.strip_prefix(yh_engine::ids::SIDE).unwrap_or(thread);
        self.thread_accounts.get(thread).and_then(|id| self.account(id)).filter(|a| a.account.enabled).unwrap_or_else(|| self.active())
    }

    fn save_thread_accounts(&self) {
        let map: serde_json::Map<String, Value> = self.thread_accounts.iter().map(|(t, a)| (t.clone(), Value::from(a.clone()))).collect();
        self.library.set_setting("claudeThreadAccounts", &Value::Object(map));
    }

    /// The thread moves to this account: its `claude` starts over there (an idle one now, a busy
    /// one once its turn is over), the conversation kept.
    pub fn move_thread(&mut self, thread: &str, id: &str, cx: &mut Context<Self>) {
        if !self.account(id).is_some_and(|a| a.account.enabled) || self.account_of(thread).id() == id {
            return;
        }
        log(format!("thread {thread} moves to {id} ({})", self.account_label(id)));
        self.thread_accounts.insert(thread.to_string(), id.to_string());
        self.save_thread_accounts();
        self.relaunch_if_elsewhere(thread, cx);
        cx.notify();
    }

    /// Its `claude` starts over when it runs on another account than the thread's now.
    fn relaunch_if_elsewhere(&mut self, thread: &str, cx: &mut Context<Self>) {
        let now = self.account_of(thread).id().to_string();
        if self.runtime_accounts.get(thread).is_some_and(|a| *a != now) {
            self.command(thread, Command::Relaunch { thread: thread.to_string() }, cx);
        }
    }

    /// Threads on an account that's going (off, removed) go to the one new threads use. Side
    /// chats and memory specialists finish where they began: nothing of theirs is saved to resume.
    fn move_threads_off(&mut self, id: &str, cx: &mut Context<Self>) {
        self.thread_accounts.retain(|_, a| a != id);
        self.save_thread_accounts();
        let running: Vec<String> = self.runtime_accounts.iter().filter(|(t, a)| a.as_str() == id && !yh_engine::ids::aside(t)).map(|(t, _)| t.clone()).collect();
        for thread in running {
            self.relaunch_if_elsewhere(&thread, cx);
        }
    }

    /// The account a thread's `claude` starts on, noted and kept as the thread's; a line in its
    /// transcript when that's another than it last ran on here.
    pub(crate) fn note_launch_account(&mut self, thread: &str) {
        let id = self.account_of(thread).id().to_string();
        if yh_engine::ids::saved(thread) && self.thread_accounts.get(thread) != Some(&id) {
            self.thread_accounts.insert(thread.to_string(), id.clone());
            self.save_thread_accounts();
        }
        let before = self.runtime_accounts.insert(thread.to_string(), id.clone());
        if before.is_some_and(|b| b != id) && yh_engine::ids::saved(thread) {
            let text = format!("Continues on {}. Claude reads the conversation again on this account, so the first turn uses more of its limits.", self.account_label(&id));
            self.engine_send(Command::Notice { thread: thread.to_string(), text, error: false });
        }
    }

    /// A new account: its folder made (sharing the main one's settings and history), then its
    /// sign-in started.
    pub fn add_account(&mut self, cx: &mut Context<Self>) -> Result<String, String> {
        let (id, dir) = yh_core::accounts::new_folder(&self.accounts_root());
        yh_core::accounts::prepare(&dir, &yh_core::history::config_directory()).map_err(|e| format!("Couldn't make the account's folder: {e}"))?;
        log(format!("added: {id} at {}", dir.display()));
        self.accounts.push(AccountState::new(Account { id: id.clone(), dir: Some(dir), enabled: true }));
        self.save_accounts();
        self.save_readings();
        self.sign_in(&id, cx);
        cx.notify();
        Ok(id)
    }

    /// `claude auth login` in the account's folder: the browser opens on Claude's sign-in page,
    /// and the account is checked again once it's done.
    pub fn sign_in(&mut self, id: &str, cx: &mut Context<Self>) {
        let claude = yh_core::claude::find_claude();
        let Some(a) = self.account_mut(id).filter(|a| a.login.is_none()) else { return };
        let Some(claude) = claude else {
            a.error = Some(yh_core::claude::CLAUDE_NOT_FOUND.into());
            cx.notify();
            return;
        };
        let path = std::env::var("YH_PATH").ok();
        let login = match yh_core::accounts::Login::start(&claude, path.as_deref(), a.account.dir.as_deref()) {
            Ok(l) => l,
            Err(e) => {
                a.error = Some(format!("Couldn't start signing in: {e}"));
                cx.notify();
                return;
            }
        };
        let waiting = login.handle();
        a.login = Some(login);
        a.error = None;
        log(format!("sign-in started: {id}"));
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let ok = cx.background_executor().spawn(async move { waiting.wait() }).await;
            let _ = this.update(cx, |ws, cx| {
                log(format!("sign-in over: {id}, {}", if ok { "signed in" } else { "not signed in" }));
                if let Some(a) = ws.account_mut(&id) {
                    a.login = None;
                }
                ws.check_account(&id, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn cancel_sign_in(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(login) = self.account_mut(id).and_then(|a| a.login.take()) {
            login.cancel();
            cx.notify();
        }
    }

    /// An added account signed out of (`claude auth logout` in its folder). The main account's
    /// sign-in is Claude Code's own, used in Terminal too: it's left alone.
    pub fn sign_out(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(dir) = self.account(id).and_then(|a| a.account.dir.clone()) else { return };
        let Some(claude) = yh_core::claude::find_claude() else { return };
        let path = std::env::var("YH_PATH").ok();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::accounts::logout(&claude, path.as_deref(), &dir) }).await;
            let _ = this.update(cx, |ws, cx| {
                log(format!("signed out: {id} {result:?}"));
                if let (Err(e), Some(a)) = (result, ws.account_mut(&id)) {
                    a.error = Some(e);
                }
                ws.check_account(&id, cx);
            });
        })
        .detach();
    }

    /// On or off. One account is always on; turning off the active one switches to the next.
    pub fn set_account_enabled(&mut self, id: &str, on: bool, cx: &mut Context<Self>) -> Result<(), String> {
        if !on && !self.accounts.iter().any(|a| a.id() != id && a.account.enabled) {
            return Err("One account always stays on.".into());
        }
        let Some(a) = self.account_mut(id) else { return Err("No such account.".into()) };
        a.account.enabled = on;
        log(format!("{id} {}", if on { "on" } else { "off" }));
        if !on && self.active_account == id {
            if let Some(next) = self.accounts.iter().find(|a| a.account.enabled).map(|a| a.id().to_string()) {
                self.switch_account(&next, cx);
            }
        }
        if !on {
            self.move_threads_off(id, cx);
        }
        self.save_accounts();
        cx.notify();
        Ok(())
    }

    /// An added account forgotten: signed out, its folder deleted (only links to the main
    /// folder's settings and history, and its own sign-in state). The main account stays.
    pub fn remove_account(&mut self, id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(a) = self.account(id) else { return Err("No such account.".into()) };
        if a.account.is_main() {
            return Err("The main account is Claude Code's own; it can be turned off, not removed.".into());
        }
        if !self.accounts.iter().any(|a| a.id() != id && a.account.enabled) {
            return Err("One account always stays on: turn another on first.".into());
        }
        if self.active_account == id {
            if let Some(next) = self.accounts.iter().find(|a| a.id() != id && a.account.enabled).map(|a| a.id().to_string()) {
                self.switch_account(&next, cx);
            }
        }
        self.cancel_sign_in(id, cx);
        let i = self.accounts.iter().position(|a| a.id() == id).expect("found above");
        let dir = self.accounts.remove(i).account.dir;
        self.move_threads_off(id, cx);
        self.save_accounts();
        self.save_readings();
        log(format!("removed: {id}"));
        if let (Some(dir), Some(claude)) = (dir, yh_core::claude::find_claude()) {
            let root = self.accounts_root();
            let path = std::env::var("YH_PATH").ok();
            cx.background_executor()
                .spawn(async move {
                    let _ = yh_core::accounts::logout(&claude, path.as_deref(), &dir);
                    if let Err(e) = yh_core::accounts::remove_folder(&dir, &root) {
                        log(format!("couldn't delete {}: {e}", dir.display()));
                    }
                })
                .detach();
        }
        cx.notify();
        Ok(())
    }
}
