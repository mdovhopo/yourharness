//! Claude's usage limits on the workspace: kept between launches (`rateLimit`,
//! `rateLimitUpdatedAt`, as the Swift app saves them) and refreshed from the sidebar with a
//! tiny Haiku request through the official `claude` (never its credentials).

use crate::state::workspace::Workspace;
use gpui_kit::Context;
use serde_json::Value;

impl Workspace {
    /// The last reading saved, at launch.
    pub fn load_saved_limits(&mut self) {
        if let Some(info) = self.library.setting("rateLimit").and_then(|v| yh_core::limits::from_saved(&v)) {
            self.limits = Some(info);
        }
        self.limits_updated_at = self.library.setting("rateLimitUpdatedAt").and_then(|v| v.as_str().and_then(yh_core::library::parse_iso));
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
