//! Claude's usage limits on the workspace: kept between launches (`rateLimit`,
//! `rateLimitUpdatedAt`, the keys 0.0.x saved) and refreshed from the sidebar with a
//! tiny Haiku request through the official `claude` (never its credentials). They belong to
//! the subscription `claude` is signed in with (`claudeSubscription`), checked at launch and on
//! each refresh: a different account's reading is dropped, not shown as this one's.

use crate::state::workspace::Workspace;
use gpui_kit::Context;
use serde_json::Value;

impl Workspace {
    /// The last reading saved, at launch.
    pub fn load_saved_limits(&mut self) {
        if let Some(info) = self.library.setting("rateLimit").and_then(|v| yh_core::limits::from_saved(&v)).filter(|i| !yh_core::limits::windows(i).is_empty()) {
            self.limits = Some(info);
        }
        self.limits_updated_at = self.limits.as_ref().and_then(|_| self.library.setting("rateLimitUpdatedAt")).and_then(|v| v.as_str().and_then(yh_core::library::parse_iso));
        self.subscription = self.library.setting("claudeSubscription").and_then(|v| yh_core::limits::Subscription::from_saved(&v));
    }

    /// Asks `claude auth status` who's signed in, off the main thread (no request to Claude).
    pub fn check_subscription(&mut self, cx: &mut Context<Self>) {
        let Some(claude) = yh_core::claude::find_claude() else { return };
        let path = std::env::var("YH_PATH").ok();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::limits::subscription(&claude, path.as_deref()) }).await;
            // Couldn't tell: what was known stays.
            let Ok(found) = result else { return };
            let _ = this.update(cx, |ws, cx| ws.note_subscription(found, cx));
        })
        .detach();
    }

    /// Who `claude` is signed in as now: kept, saved, and another account's reading dropped.
    pub fn note_subscription(&mut self, found: Option<yh_core::limits::Subscription>, cx: &mut Context<Self>) {
        if self.subscription == found {
            return;
        }
        if std::env::var("YH_DATA_DIR").is_ok() {
            eprintln!("LIMITS subscription: {}", found.as_ref().map(|s| format!("{} · {}", s.plan_name(), s.who().unwrap_or("?"))).unwrap_or_else(|| "signed out".into()));
        }
        let switched = match (&self.subscription, &found) {
            (Some(old), Some(new)) => !old.same_account(new),
            (Some(_), None) => true,
            _ => false,
        };
        if switched {
            self.limits = None;
            self.limits_updated_at = None;
            // 0.0.x's shape with no windows, so a downgrade still reads it.
            self.library.set_setting("rateLimit", &serde_json::json!({"status": "unknown", "windows": []}));
        }
        self.library.set_setting("claudeSubscription", &found.as_ref().map(|s| s.to_saved()).unwrap_or(Value::Null));
        self.subscription = found;
        cx.notify();
    }

    /// A reading from the CLI (a turn's, or a refresh's): kept, saved, and any error cleared.
    pub fn note_limits(&mut self, info: &Value, cx: &mut Context<Self>) {
        if yh_core::limits::windows(info).is_empty() {
            return;
        }
        let now = yh_core::library::now_ms() / 1000.0;
        let changed = self.limits.as_ref() != Some(info);
        // Saved when it changed, or once a minute: the "Updated …" in the tooltip.
        if changed || self.limits_updated_at.is_none_or(|t| now - t > 60.) {
            self.library.set_setting("rateLimit", &yh_core::limits::to_saved(info));
            self.library.set_setting("rateLimitUpdatedAt", &Value::from(yh_core::library::iso(now as i64)));
            self.limits_updated_at = Some(now);
        }
        self.limits = Some(info.clone());
        if changed || self.limits_error.take().is_some() {
            cx.notify();
        }
    }

    /// Asks Claude for a fresh reading (about 600 tokens of Haiku), off the main thread.
    pub fn refresh_limits(&mut self, cx: &mut Context<Self>) {
        if self.limits_refreshing {
            return;
        }
        let Some(claude) = yh_core::claude::find_claude() else {
            self.limits_error = Some("Claude Code isn't installed, or isn't on your PATH.".into());
            cx.notify();
            return;
        };
        self.limits_refreshing = true;
        self.check_subscription(cx);
        cx.notify();
        let path = std::env::var("YH_PATH").ok();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { yh_core::limits::probe(&claude, path.as_deref()) }).await;
            if std::env::var("YH_DATA_DIR").is_ok() {
                match &result {
                    Ok(info) => eprintln!("LIMITS refreshed: {}", yh_core::limits::to_saved(info)),
                    Err(e) => eprintln!("LIMITS refresh failed: {e}"),
                }
            }
            let _ = this.update(cx, |ws, cx| {
                ws.limits_refreshing = false;
                match result {
                    Ok(info) => {
                        // A refresh always says when: even an unchanged reading is new.
                        ws.limits_updated_at = None;
                        ws.note_limits(&info, cx);
                    }
                    Err(e) => ws.limits_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
