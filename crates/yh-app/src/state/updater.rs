//! Keeps the app current (`yh_core::updates`): looks for a newer release a little after launch
//! and every six hours, downloads it when you ask, checks it and puts the new app next to
//! this one; Restart swaps them. The sidebar's footer says when there's one.
//!
//! Builds that aren't releases (`.next` until the switch, `.dev`) never look on GitHub: a
//! release there would replace them with the main app. `YH_UPDATE_FEED` makes them look at a
//! file instead, for testing the whole flow; `YH_UPDATE_DOWNLOAD=1` then downloads what it
//! finds and `YH_UPDATE_RESTART=1` restarts into it, printing each step.

use gpui_kit::{Context, Task};
use std::path::PathBuf;
use std::time::Duration;
use yh_core::updates::{self, Release, Version};

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, f32),
    Preparing(Release),
    Ready(Version),
    Failed(String, Option<Release>),
}

pub struct Updater {
    pub phase: Phase,
    /// This app's bundle, when it runs as one.
    bundle: Option<PathBuf>,
    bundle_id: String,
    pub current: Option<Version>,
    feed: Option<String>,
    _timer: Option<Task<()>>,
}

fn log(text: String) {
    if std::env::var("YH_UPDATE_FEED").is_ok() {
        eprintln!("UPDATE {text}");
    }
}

impl Updater {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let bundle = std::env::current_exe().ok().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app"));
        let info = bundle.as_deref().and_then(updates::bundle_info);
        let (bundle_id, current) = match info {
            Some((id, v)) => (id, Some(v)),
            None => (String::new(), None),
        };
        let feed = std::env::var("YH_UPDATE_FEED").ok().filter(|f| !f.is_empty());
        let mut updater = Updater { phase: Phase::Idle, bundle, bundle_id, current, feed, _timer: None };
        updater.restore_staged();
        if updater.looks() {
            updater._timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(if std::env::var("YH_UPDATE_FEED").is_ok() { 1 } else { 10 })).await;
                loop {
                    if this.update(cx, |u, cx| u.check(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(6 * 3600)).await;
                }
            }));
        }
        updater
    }

    /// Whether this copy looks for updates at all.
    fn looks(&self) -> bool {
        self.current.is_some() && (self.feed.is_some() || !(self.bundle_id.ends_with(".next") || self.bundle_id.ends_with(".dev")))
    }

    fn set(&mut self, phase: Phase, cx: &mut Context<Self>) {
        log(match &phase {
            Phase::Available(r) => format!("available {}", r.version),
            Phase::Downloading(r, p) => format!("downloading {} {:.0}%", r.version, p * 100.),
            Phase::Preparing(r) => format!("preparing {}", r.version),
            Phase::Ready(v) => format!("ready {v}"),
            Phase::Failed(e, _) => format!("failed: {e}"),
            other => format!("{other:?}"),
        });
        self.phase = phase;
        cx.notify();
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Checking | Phase::Downloading(..) | Phase::Preparing(_) | Phase::Ready(_)) {
            return;
        }
        let Some(current) = self.current.clone() else { return };
        let previous = self.phase.clone();
        self.set(Phase::Checking, cx);
        let feed = self.feed.clone();
        cx.spawn(async move |this, cx| {
            let c = current.to_string();
            let result = cx.background_executor().spawn(async move { updates::fetch_latest(feed.as_deref(), &c) }).await;
            let _ = this.update(cx, |u, cx| match result {
                Ok(r) if r.version > current => {
                    u.set(Phase::Available(r), cx);
                    if std::env::var("YH_UPDATE_DOWNLOAD").as_deref() == Ok("1") {
                        u.download(cx);
                    }
                }
                Ok(_) => u.set(Phase::UpToDate, cx),
                // A background check that fails keeps what it knew.
                Err(e) => {
                    log(format!("check failed: {e}"));
                    u.set(previous, cx);
                }
            });
        })
        .detach();
    }

    pub fn download(&mut self, cx: &mut Context<Self>) {
        let release = match &self.phase {
            Phase::Available(r) | Phase::Failed(_, Some(r)) => r.clone(),
            _ => return,
        };
        let Some(app) = self.bundle.clone() else { return };
        let writable = app.parent().map(|p| std::fs::metadata(p).map(|m| !m.permissions().readonly()).unwrap_or(false)).unwrap_or(false);
        if !writable {
            return self.set(Phase::Failed(format!("YourHarness can't write to {}. Download {} from GitHub instead.", app.parent().unwrap_or(&app).display(), release.version), Some(release)), cx);
        }
        self.set(Phase::Downloading(release.clone(), 0.), cx);
        let scratch = std::env::temp_dir().join(format!("YourHarness-update-{}", yh_core::library::new_uuid()));
        let image = scratch.join("YourHarness.dmg");
        let id = self.bundle_id.clone();
        cx.spawn(async move |this, cx| {
            let _ = std::fs::create_dir_all(&scratch);
            let (url, img) = (release.disk_image.clone(), image.clone());
            let download = cx.background_executor().spawn(async move { updates::download(&url, &img) });
            // Progress: what's on disk of the size the release gives.
            let total = release.disk_image_size.unwrap_or(0) as f32;
            let watch = {
                let (image, release) = (image.clone(), release.clone());
                let this = this.clone();
                cx.spawn(async move |cx| loop {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let got = std::fs::metadata(&image).map(|m| m.len() as f32).unwrap_or(0.);
                    let more = this.update(cx, |u, cx| {
                        if matches!(u.phase, Phase::Downloading(..)) && total > 0. {
                            u.phase = Phase::Downloading(release.clone(), (got / total).min(1.));
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    });
                    if !more.unwrap_or(false) {
                        break;
                    }
                })
            };
            let downloaded = download.await;
            drop(watch);
            let _ = this.update(cx, |u, cx| u.set(Phase::Preparing(release.clone()), cx));
            let staged = updates::staged_path(&app);
            let (r, s) = (release.clone(), scratch.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    downloaded?;
                    if let Some(sum) = &r.checksum {
                        updates::verify_checksum(&image, sum)?;
                    }
                    updates::stage(&image, &r.version, &id, &app, &staged, &s)
                })
                .await;
            let _ = std::fs::remove_dir_all(&scratch);
            let _ = this.update(cx, |u, cx| match result {
                Ok(()) => {
                    u.set(Phase::Ready(release.version.clone()), cx);
                    if std::env::var("YH_UPDATE_RESTART").as_deref() == Ok("1") {
                        u.restart_now(cx);
                    }
                }
                Err(e) => u.set(Phase::Failed(e, Some(release)), cx),
            });
        })
        .detach();
    }

    /// A version downloaded before the app last quit is still ready.
    fn restore_staged(&mut self) {
        let (Some(app), Some(current)) = (&self.bundle, &self.current) else { return };
        let staged = updates::staged_path(app);
        if !staged.exists() {
            return;
        }
        match updates::bundle_info(&staged) {
            Some((id, v)) if id == self.bundle_id && &v > current => self.phase = Phase::Ready(v),
            _ => {
                let _ = std::fs::remove_dir_all(&staged);
            }
        }
    }

    /// Quits; once this process is gone, a small script swaps in the new app and opens it.
    fn restart_now(&mut self, cx: &mut Context<Self>) {
        let Some(app) = self.bundle.clone() else { return };
        // A development run comes back the way it was started: same scratch state, in the background.
        let mut open: Vec<String> = vec![];
        if std::env::var("YH_DATA_DIR").is_ok() {
            open.push("-g".into());
        }
        let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k.starts_with("YH_") && k != "YH_UPDATE_RESTART" && k != "YH_UPDATE_DOWNLOAD").collect();
        vars.sort();
        for (k, v) in vars {
            open.push("--env".into());
            open.push(format!("{k}={v}"));
        }
        match updates::swap_after_quit(&app, &open) {
            Ok(()) => cx.quit(),
            Err(e) => self.set(Phase::Failed(e, None), cx),
        }
    }

    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Ready(_)) {
            self.restart_now(cx);
        }
    }
}
