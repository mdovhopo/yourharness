//! Claude's usage limits, one reading per Claude account (`state/accounts.rs`): kept between
//! launches and refreshed from the sidebar with a tiny Haiku request through the official
//! `claude` (never its credentials). Each belongs to the subscription its account is signed in
//! with, checked at launch and on each refresh: a different subscription's reading is dropped,
//! not shown as this one's.
//!
//! Saved per account as `claudeAccountReadings` (`{id: {rateLimit, rateLimitUpdatedAt,
//! subscription}}`), and the active account's also under the keys 0.0.x saved (`rateLimit`,
//! `rateLimitUpdatedAt`, `claudeSubscription`), which the phone and a downgrade read.

use crate::state::workspace::Workspace;
use gpui_kit::Context;
use serde_json::{json, Value};
use yh_core::accounts::Account;
use yh_core::limits::Subscription;

/// A Claude account as the app knows it.
pub struct AccountState {
    pub account: Account,
    /// Its usage limits as Claude last reported them.
    pub limits: Option<Value>,
    /// When (seconds).
    pub limits_updated_at: Option<f64>,
    /// Who it's signed in as; `None` signed out, or not known until `checked`.
    pub subscription: Option<Subscription>,
    pub checked: bool,
    /// A refresh asked of Claude under way, and why the last failed.
    pub refreshing: bool,
    pub error: Option<String>,
    /// `claude auth login` under way (its browser page open).
    pub login: Option<yh_core::accounts::Login>,
}

impl AccountState {
    pub fn new(account: Account) -> AccountState {
        AccountState { account, limits: None, limits_updated_at: None, subscription: None, checked: false, refreshing: false, error: None, login: None }
    }

    pub fn id(&self) -> &str {
        &self.account.id
    }

    /// What it is, never who: its plan ("Claude Max"), else "Main account", "New account". The
    /// email stays in tooltips and Settings (`Workspace::account_label` tells same plans apart).
    pub fn label(&self) -> String {
        match &self.subscription {
            Some(s) => s.plan_name(),
            None if self.account.is_main() => "Main account".into(),
            None => "New account".into(),
        }
    }

    /// Checked and nobody's signed in.
    pub fn signed_out(&self) -> bool {
        self.checked && self.subscription.is_none()
    }

    fn reading_to_saved(&self) -> Value {
        json!({
            "rateLimit": self.limits.as_ref().map(yh_core::limits::to_saved),
            "rateLimitUpdatedAt": self.limits_updated_at.map(|t| yh_core::library::iso(t as i64)),
            "subscription": self.subscription.as_ref().map(Subscription::to_saved),
        })
    }
}

impl Workspace {
    /// The accounts and their last readings, at launch.
    pub fn load_saved_limits(&mut self) {
        let accounts = yh_core::accounts::from_saved(self.library.setting("claudeAccounts").as_ref());
        let readings = self.library.setting("claudeAccountReadings");
        let legacy = readings.is_none();
        self.accounts = accounts
            .into_iter()
            .map(|account| {
                let mut a = AccountState::new(account);
                // Before accounts: the one reading there was is the main account's.
                let saved = if legacy && a.account.is_main() {
                    json!({"rateLimit": self.library.setting("rateLimit"), "rateLimitUpdatedAt": self.library.setting("rateLimitUpdatedAt"), "subscription": self.library.setting("claudeSubscription")})
                } else {
                    readings.as_ref().map(|r| r[a.id()].clone()).unwrap_or(Value::Null)
                };
                a.limits = yh_core::limits::from_saved(&saved["rateLimit"]).filter(|i| !yh_core::limits::windows(i).is_empty());
                a.limits_updated_at = a.limits.as_ref().and_then(|_| saved["rateLimitUpdatedAt"].as_str().and_then(yh_core::library::parse_iso));
                a.subscription = Subscription::from_saved(&saved["subscription"]);
                a
            })
            .collect();
        let active = self.library.setting("claudeActiveAccount").and_then(|v| v.as_str().map(str::to_string));
        self.active_account = active.filter(|id| self.accounts.iter().any(|a| a.id() == id && a.account.enabled)).unwrap_or_else(|| self.accounts.iter().find(|a| a.account.enabled).map(|a| a.id().to_string()).unwrap_or_default());
        yh_core::shell_env::set_claude_config(self.active().account.dir.as_ref().map(|d| d.to_string_lossy().into_owned()));
    }

    /// The account new turns use.
    pub fn active(&self) -> &AccountState {
        self.accounts.iter().find(|a| a.id() == self.active_account).unwrap_or(&self.accounts[0])
    }

    /// An account's name on screen: its plan, numbered when another has the same ("Claude Max 2").
    pub fn account_label(&self, id: &str) -> String {
        let Some(i) = self.accounts.iter().position(|a| a.id() == id) else { return String::new() };
        let label = self.accounts[i].label();
        let same = self.accounts.iter().filter(|a| a.label() == label).count();
        let nth = self.accounts[..i].iter().filter(|a| a.label() == label).count();
        if same > 1 && nth > 0 { format!("{label} {}", nth + 1) } else { label }
    }

    pub fn account(&self, id: &str) -> Option<&AccountState> {
        self.accounts.iter().find(|a| a.id() == id)
    }

    pub(crate) fn account_mut(&mut self, id: &str) -> Option<&mut AccountState> {
        self.accounts.iter_mut().find(|a| a.id() == id)
    }

    /// Every account's reading saved, and the active one's under 0.0.x's keys too.
    pub(crate) fn save_readings(&self) {
        let readings: serde_json::Map<String, Value> = self.accounts.iter().map(|a| (a.id().to_string(), a.reading_to_saved())).collect();
        self.library.set_setting("claudeAccountReadings", &Value::Object(readings));
        let active = self.active();
        // 0.0.x's shape with no windows, so a downgrade still reads it.
        self.library.set_setting("rateLimit", &active.limits.as_ref().map(yh_core::limits::to_saved).unwrap_or_else(|| json!({"status": "unknown", "windows": []})));
        self.library.set_setting("rateLimitUpdatedAt", &active.limits_updated_at.map(|t| Value::from(yh_core::library::iso(t as i64))).unwrap_or(Value::Null));
        self.library.set_setting("claudeSubscription", &active.subscription.as_ref().map(Subscription::to_saved).unwrap_or(Value::Null));
    }

    /// Asks `claude auth status` who each account is signed in as, off the main thread (no
    /// request to Claude).
    pub fn check_subscription(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.accounts.iter().map(|a| a.id().to_string()).collect();
        for id in ids {
            self.check_account(&id, cx);
        }
    }

    pub fn check_account(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(claude) = yh_core::claude::find_claude() else { return };
        let Some(dir) = self.account(id).map(|a| a.account.dir.clone()) else { return };
        let path = std::env::var("YH_PATH").ok();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::limits::subscription(&claude, path.as_deref(), dir.as_deref()) }).await;
            // Couldn't tell: what was known stays.
            let Ok(found) = result else { return };
            let _ = this.update(cx, |ws, cx| ws.note_subscription(&id, found, cx));
        })
        .detach();
    }

    /// Who an account is signed in as now: kept, saved, and another subscription's reading dropped.
    pub fn note_subscription(&mut self, id: &str, found: Option<Subscription>, cx: &mut Context<Self>) {
        let Some(a) = self.account_mut(id) else { return };
        let first = !std::mem::replace(&mut a.checked, true);
        if a.subscription == found {
            if first {
                cx.notify();
            }
            return;
        }
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("LIMITS subscription {id}: {}", found.as_ref().map(|s| format!("{} · {}", s.plan_name(), s.who().unwrap_or("?"))).unwrap_or_else(|| "signed out".into()));
        }
        let switched = match (&a.subscription, &found) {
            (Some(old), Some(new)) => !old.same_account(new),
            (Some(_), None) => true,
            _ => false,
        };
        if switched {
            a.limits = None;
            a.limits_updated_at = None;
        }
        a.subscription = found;
        self.save_readings();
        cx.notify();
    }

    /// A reading from the CLI (a turn's, or a refresh's) for an account: kept, saved, and any
    /// error cleared.
    pub fn note_limits(&mut self, id: &str, info: &Value, cx: &mut Context<Self>) {
        if yh_core::limits::windows(info).is_empty() {
            return;
        }
        let now = yh_core::library::now_ms() / 1000.0;
        let Some(a) = self.account_mut(id) else { return };
        let changed = a.limits.as_ref() != Some(info);
        // Saved when it changed, or once a minute: the "Updated …" in the tooltip.
        let save = changed || a.limits_updated_at.is_none_or(|t| now - t > 60.);
        if save {
            a.limits_updated_at = Some(now);
        }
        a.limits = Some(info.clone());
        let cleared = a.error.take().is_some();
        if save {
            self.save_readings();
        }
        if changed || cleared {
            cx.notify();
        }
    }

    /// Asks Claude for a fresh reading of every account that's on (about 600 tokens of Haiku
    /// each), off the main thread.
    pub fn refresh_limits(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.accounts.iter().filter(|a| a.account.enabled).map(|a| a.id().to_string()).collect();
        for id in ids {
            self.refresh_account(&id, cx);
        }
    }

    /// Whether any account's refresh is under way.
    pub fn limits_refreshing(&self) -> bool {
        self.accounts.iter().any(|a| a.refreshing)
    }

    pub fn refresh_account(&mut self, id: &str, cx: &mut Context<Self>) {
        let claude = yh_core::claude::find_claude();
        let Some(a) = self.account_mut(id).filter(|a| !a.refreshing) else { return };
        let Some(claude) = claude else {
            a.error = Some("Claude Code isn't installed, or isn't on your PATH.".into());
            cx.notify();
            return;
        };
        a.refreshing = true;
        let dir = a.account.dir.clone();
        self.check_account(id, cx);
        cx.notify();
        let path = std::env::var("YH_PATH").ok();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::limits::probe(&claude, path.as_deref(), dir.as_deref()) }).await;
            if std::env::var("YH_DATA_DIR").is_ok() {
                match &result {
                    Ok(info) => eprintln!("LIMITS refreshed {id}: {}", yh_core::limits::to_saved(info)),
                    Err(e) => eprintln!("LIMITS refresh failed {id}: {e}"),
                }
            }
            let _ = this.update(cx, |ws, cx| {
                let Some(a) = ws.account_mut(&id) else { return };
                a.refreshing = false;
                match result {
                    Ok(info) => {
                        // A refresh always says when: even an unchanged reading is new.
                        a.limits_updated_at = None;
                        ws.note_limits(&id, &info, cx);
                    }
                    Err(e) => a.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
