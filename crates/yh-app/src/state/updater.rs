//! Keeps the app current (`yh_core::updates`): looks for a newer release a little after launch
//! and every six hours, downloads it when you ask, checks it and puts the new app next to
//! this one; Restart swaps them. The sidebar's footer says when there's one.
//!
//! Builds that aren't releases (`.next`, `.dev`) never look on GitHub: a
//! release there would replace them with the main app. `YH_UPDATE_FEED` makes them look at a
//! file instead, for testing the whole flow; `YH_UPDATE_DOWNLOAD=1` then downloads what it
//! finds and `YH_UPDATE_RESTART=1` restarts into it, printing each step.
//!
//! Install Automatically (Settings → Updates, off unless you turn it on, `YH_AUTO_UPDATE=1`
//! for a run) downloads what it finds and restarts into it once nothing has run for two
//! minutes (`YH_UPDATE_IDLE` seconds): no thread working, waiting for you or with messages
//! queued, no background jobs. First a countdown shows for two minutes (`YH_UPDATE_COUNTDOWN`)
//! with Later (fifteen minutes, `YH_UPDATE_SNOOZE`) and Restart Now, and a notification when the
//! window is out of sight; anything starting, or a key typed, calls it off. A locked screen
//! restarts at once: nobody would see it.

use gpui_kit::{App, Context, Task};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yh_core::library::Library;
use yh_core::updates::{self, Notes, Release, Version};

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
    /// The bundle's build number (`CFBundleVersion`).
    pub build: Option<String>,
    feed: Option<String>,
    /// A check you asked for (the menu, a button) also says when there's nothing new.
    pub announces: bool,
    /// When it last heard from GitHub (seconds since 1970).
    pub last_checked: Option<f64>,
    /// The version this copy ran as last time, when it's older: "Updated to X" until dismissed.
    pub updated_from: Option<Version>,
    /// Every release's notes once read (kept ten minutes), and why reading them failed.
    pub notes: Option<(Vec<Notes>, Instant)>,
    pub notes_loading: bool,
    pub notes_error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
    _timer: Option<Task<()>>,
    /// Install Automatically is on.
    pub auto: bool,
    /// When the automatic restart happens, while its countdown shows.
    pub countdown: Option<Instant>,
    /// Later was pressed: no countdown before this.
    snoozed_until: Option<Instant>,
    /// Nothing has run since then.
    idle_since: Option<Instant>,
    /// The countdown ran out: a last look for a newer release, then the restart.
    applying: bool,
    /// Why the app can't restart now (set by the window: threads, jobs).
    busy: Option<Rc<dyn Fn(&App) -> Option<String>>>,
    _watch: Option<Task<()>>,
}

fn seconds(var: &str, default: u64) -> Duration {
    Duration::from_secs(std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default))
}

fn log(text: String) {
    if std::env::var("YH_UPDATE_FEED").is_ok() {
        eprintln!("UPDATE {text}");
    }
}

impl Updater {
    pub fn new(library: &Library, cx: &mut Context<Self>) -> Self {
        let bundle = std::env::current_exe().ok().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app"));
        let info = bundle.as_deref().and_then(updates::bundle_info);
        let (bundle_id, current) = match info {
            Some((id, v)) => (id, Some(v)),
            None => (String::new(), None),
        };
        let build = bundle.as_ref().and_then(|b| {
            let out = std::process::Command::new("/usr/bin/plutil").args(["-extract", "CFBundleVersion", "raw", "-o", "-"]).arg(b.join("Contents/Info.plist")).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|b| !b.is_empty())
        });
        // The version it ran as last time, in the app's own settings (0.0.x kept it in
        // its defaults): newer now means it was just updated.
        let last = library.setting("lastRunVersion").and_then(|v| v.as_str().and_then(Version::parse));
        let updated_from = match (&last, &current) {
            (Some(l), Some(c)) if l < c => Some(l.clone()),
            _ => None,
        };
        if let Some(c) = &current {
            library.set_setting("lastRunVersion", &serde_json::Value::from(c.to_string()));
        }
        if let Some(from) = &updated_from {
            log(format!("updated from {from}"));
        }
        let feed = std::env::var("YH_UPDATE_FEED").ok().filter(|f| !f.is_empty());
        let auto = std::env::var("YH_AUTO_UPDATE").as_deref() == Ok("1") || library.setting("autoUpdate").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut updater = Updater {
            phase: Phase::Idle,
            bundle,
            bundle_id,
            current,
            build,
            feed,
            announces: false,
            last_checked: None,
            updated_from,
            notes: None,
            notes_loading: false,
            notes_error: None,
            cancel: None,
            _timer: None,
            auto,
            countdown: None,
            snoozed_until: None,
            idle_since: None,
            applying: false,
            busy: None,
            _watch: None,
        };
        updater.restore_staged();
        updater.watch(cx);
        // The countdown's notification: Later puts it off, a click shows the window.
        let answers = crate::platform::update_notification_answers();
        cx.spawn(async move |this, cx| {
            while let Ok(answer) = answers.recv_async().await {
                let done = this.update(cx, |u, cx| {
                    if answer == "later" {
                        u.later(cx);
                    } else {
                        crate::platform::bring_to_front();
                    }
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
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

    /// "0.1.0", or what this copy is when it isn't a bundle.
    pub fn version_label(&self) -> String {
        self.current.as_ref().map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), |v| v.to_string())
    }

    /// Why it doesn't look for updates, when it doesn't.
    pub fn why_not(&self) -> Option<&'static str> {
        if self.looks() {
            None
        } else if self.current.is_none() {
            Some("Updates work in the app bundle, not a bare build.")
        } else {
            Some("Development copies don't look for updates.")
        }
    }

    /// Looks now, saying so when there's nothing new (the menu and the buttons).
    pub fn check_now(&mut self, cx: &mut Context<Self>) {
        if !self.looks() {
            return;
        }
        self.announces = true;
        if matches!(self.phase, Phase::Failed(_, None) | Phase::UpToDate) {
            self.phase = Phase::Idle;
        }
        self.check(cx);
    }

    /// Stops a download under way: back to offering it.
    pub fn cancel_download(&mut self, cx: &mut Context<Self>) {
        if let (Some(cancel), Phase::Downloading(r, _)) = (self.cancel.take(), self.phase.clone()) {
            cancel.store(true, Ordering::SeqCst);
            self.set(Phase::Available(r), cx);
        }
    }

    /// A failed check's note goes.
    pub fn dismiss_failure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Failed(_, None)) {
            self.announces = false;
            self.set(Phase::Idle, cx);
        }
    }

    /// "Updated to X" goes.
    pub fn dismiss_updated(&mut self, cx: &mut Context<Self>) {
        if self.updated_from.take().is_some() {
            cx.notify();
        }
    }

    /// "X is the latest" goes (a while after a check you asked for).
    pub fn dismiss_latest(&mut self, cx: &mut Context<Self>) {
        if self.announces && self.phase == Phase::UpToDate {
            self.announces = false;
            cx.notify();
        }
    }

    /// Reads every release's notes from GitHub (or the feed), unless read in the last ten minutes.
    pub fn load_notes(&mut self, cx: &mut Context<Self>) {
        if self.notes_loading || self.notes.as_ref().is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(600)) {
            return;
        }
        self.notes_loading = true;
        let (feed, current) = (self.feed.clone(), self.version_label());
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { updates::fetch_notes(feed.as_deref(), &current) }).await;
            let _ = this.update(cx, |u, cx| {
                u.notes_loading = false;
                match result {
                    Ok(n) => {
                        log(format!("notes {}", n.iter().map(|n| n.version.to_string()).collect::<Vec<_>>().join(",")));
                        u.notes = Some((n, Instant::now()));
                        u.notes_error = None;
                    }
                    Err(e) => {
                        log(format!("notes failed: {e}"));
                        u.notes_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Restart, after saying what it does to threads that are working (`working` of them).
    pub fn restart_message(version: &Version, working: usize) -> String {
        let reopen = format!("YourHarness quits and opens again on {version}; your threads and drafts carry over.");
        match working {
            0 => reopen,
            1 => format!("One thread is working: restarting stops it, and you can send again after. {reopen}"),
            n => format!("{n} threads are working: restarting stops them, and you can send again after. {reopen}"),
        }
    }

    /// Whether this copy looks for updates at all.
    fn looks(&self) -> bool {
        self.current.is_some() && (self.feed.is_some() || !(self.bundle_id.ends_with(".next") || self.bundle_id.ends_with(".dev")))
    }

    /// Install Automatically turned on or off (Settings saves it).
    pub fn set_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.auto = on;
        if !on {
            self.stop_countdown(None, cx);
        } else if matches!(self.phase, Phase::Available(_)) {
            self.download(cx);
        }
        self.watch(cx);
        cx.notify();
    }

    /// What says the app is busy: asked once a second while an update waits.
    pub fn set_busy_check(&mut self, check: Rc<dyn Fn(&App) -> Option<String>>) {
        self.busy = Some(check);
    }

    /// Once a second while an update is ready and installs by itself: is it time?
    fn watch(&mut self, cx: &mut Context<Self>) {
        if !self.auto || !matches!(self.phase, Phase::Ready(_)) || self._watch.is_some() {
            return;
        }
        log("waiting for the app to be idle".into());
        self._watch = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            match this.update(cx, |u, cx| u.tick(cx)) {
                Ok(true) => {}
                _ => break,
            }
        }));
    }

    /// One look: false once there's nothing more to wait for.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.auto || !matches!(self.phase, Phase::Ready(_)) {
            self.stop_countdown(None, cx);
            self._watch = None;
            return false;
        }
        if self.applying {
            return true;
        }
        let now = Instant::now();
        if let Some(why) = self.busy.as_ref().and_then(|b| b(cx)) {
            self.idle_since = None;
            if self.countdown.is_some() {
                self.stop_countdown(Some(&why), cx);
            }
            return true;
        }
        let since = *self.idle_since.get_or_insert(now);
        if let Some(ends) = self.countdown {
            if now >= ends {
                self.apply(cx);
            }
            return true;
        }
        if self.snoozed_until.is_some_and(|t| now < t) || now.duration_since(since) < seconds("YH_UPDATE_IDLE", 120) {
            return true;
        }
        if crate::platform::screen_off() {
            log("idle, screen off: restarting".into());
            self.apply(cx);
            return true;
        }
        let wait = seconds("YH_UPDATE_COUNTDOWN", 120);
        self.countdown = Some(now + wait);
        log(format!("idle: restarting in {}s", wait.as_secs()));
        if !crate::platform::app_is_active() {
            if let Phase::Ready(v) = &self.phase {
                crate::platform::notify_update(&format!("Restarting to update to {v}"), &format!("YourHarness restarts in {} once nothing's running. Choose Later to wait 15 minutes.", countdown_words(wait)));
            }
        }
        // The first countdown only, so a test sees the next one run out.
        static ANSWERED: AtomicBool = AtomicBool::new(false);
        if let Ok(answer) = std::env::var("YH_UPDATE_TOAST_ANSWER").map_err(|_| ()).and_then(|a| if ANSWERED.swap(true, Ordering::SeqCst) { Err(()) } else { Ok(a) }) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |u, cx| {
                    eprintln!("UPDATE toast {answer}");
                    if answer == "now" { u.restart_from_countdown(cx) } else { u.later(cx) }
                });
            })
            .detach();
        }
        cx.notify();
        true
    }

    /// The countdown goes (`why`: something started), back to waiting for two quiet minutes.
    fn stop_countdown(&mut self, why: Option<&str>, cx: &mut Context<Self>) {
        if self.countdown.take().is_some() {
            log(format!("countdown called off{}", why.map(|w| format!(": {w}")).unwrap_or_default()));
            crate::platform::clear_update_notification();
            cx.notify();
        }
    }

    /// A key typed: you're here, so not now.
    pub fn typed(&mut self, cx: &mut Context<Self>) {
        if self.countdown.is_some() {
            self.idle_since = None;
            self.stop_countdown(Some("a key typed"), cx);
        }
    }

    /// Later: the countdown goes, and none comes for fifteen minutes.
    pub fn later(&mut self, cx: &mut Context<Self>) {
        let wait = seconds("YH_UPDATE_SNOOZE", 15 * 60);
        self.snoozed_until = Some(Instant::now() + wait);
        self.stop_countdown(Some(&format!("later, for {}s", wait.as_secs())), cx);
    }

    /// "15:42": when the countdown ends, as the clock says it.
    pub fn restart_at(&self) -> Option<String> {
        let left = self.countdown?.saturating_duration_since(Instant::now()).as_secs_f64();
        let now = yh_core::library::now_ms() / 1000.;
        Some(yh_core::clock::message_time(now + left, now))
    }

    /// Restart Now, on the countdown.
    pub fn restart_from_countdown(&mut self, cx: &mut Context<Self>) {
        if self.countdown.is_some() {
            self.apply(cx);
        }
    }

    /// The automatic restart: a last look for a newer release first (which replaces this one
    /// and starts over), then, if still nothing runs, the restart.
    fn apply(&mut self, cx: &mut Context<Self>) {
        self.applying = true;
        self.check(cx);
    }

    /// The last look found nothing newer.
    fn finish_apply(&mut self, cx: &mut Context<Self>) {
        self.applying = false;
        if let Some(why) = self.busy.as_ref().and_then(|b| b(cx)) {
            self.idle_since = None;
            return self.stop_countdown(Some(&why), cx);
        }
        crate::platform::clear_update_notification();
        log("restarting by itself".into());
        self.restart_now(!crate::platform::app_is_active(), cx);
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
        if matches!(self.phase, Phase::Ready(_)) {
            self.watch(cx);
        } else {
            self.applying = false;
            self.stop_countdown(None, cx);
        }
        cx.notify();
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Checking | Phase::Downloading(..) | Phase::Preparing(_)) {
            return;
        }
        let Some(current) = self.current.clone() else { return };
        let previous = self.phase.clone();
        // A version already downloaded stays ready (Restart shown) while it looks for a newer one.
        let staged = match &previous {
            Phase::Ready(v) => Some(v.clone()),
            _ => None,
        };
        if staged.is_none() {
            self.set(Phase::Checking, cx);
        }
        let feed = self.feed.clone();
        let announced = self.announces;
        cx.spawn(async move |this, cx| {
            let c = current.to_string();
            let result = cx.background_executor().spawn(async move { updates::fetch_latest(feed.as_deref(), &c) }).await;
            let _ = this.update(cx, |u, cx| match result {
                // Released since that one was downloaded: the newest replaces it, so a copy
                // several versions behind lands on the latest in one restart.
                Ok(r) if staged.as_ref().is_some_and(|s| r.version > *s) => {
                    log(format!("replacing {} with {}", staged.as_ref().map(|s| s.to_string()).unwrap_or_default(), r.version));
                    if let Some(app) = &u.bundle {
                        let _ = std::fs::remove_dir_all(updates::staged_path(app));
                    }
                    u.set(Phase::Available(r), cx);
                    u.download(cx);
                }
                Ok(_) | Err(_) if staged.is_some() => {
                    if u.applying {
                        u.finish_apply(cx);
                    }
                }
                Ok(r) if r.version > current => {
                    u.set(Phase::Available(r), cx);
                    if u.auto || std::env::var("YH_UPDATE_DOWNLOAD").as_deref() == Ok("1") {
                        u.download(cx);
                    }
                }
                Ok(_) => {
                    u.last_checked = Some(yh_core::library::now_ms() / 1000.);
                    u.set(Phase::UpToDate, cx);
                    // "X is the latest" for a while, then it goes.
                    if announced {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_secs(8)).await;
                            let _ = this.update(cx, |u, cx| u.dismiss_latest(cx));
                        })
                        .detach();
                    }
                }
                // A background check that fails keeps what it knew; one you asked for says why.
                Err(e) => {
                    log(format!("check failed: {e}"));
                    u.set(if announced { Phase::Failed(e, None) } else { previous }, cx);
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
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let scratch = std::env::temp_dir().join(format!("YourHarness-update-{}", yh_core::library::new_uuid()));
        let image = scratch.join("YourHarness.dmg");
        let id = self.bundle_id.clone();
        cx.spawn(async move |this, cx| {
            let _ = std::fs::create_dir_all(&scratch);
            let (url, img) = (release.disk_image.clone(), image.clone());
            let stop = cancel.clone();
            let download = cx.background_executor().spawn(async move { updates::download_cancellable(&url, &img, &stop) });
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
            if cancel.load(Ordering::SeqCst) {
                let _ = std::fs::remove_dir_all(&scratch);
                log("download cancelled".into());
                return;
            }
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
                    u.cancel = None;
                    u.set(Phase::Ready(release.version.clone()), cx);
                    if std::env::var("YH_UPDATE_RESTART").as_deref() == Ok("1") {
                        u.restart_now(false, cx);
                    } else {
                        // One may have come out while this one downloaded.
                        u.check(cx);
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

    /// Quits; once this process is gone, a small script swaps in the new app and opens it
    /// (`quiet`: behind the app you're using, as it was).
    fn restart_now(&mut self, quiet: bool, cx: &mut Context<Self>) {
        let Some(app) = self.bundle.clone() else { return };
        // A development run comes back the way it was started: same scratch state, in the background.
        let mut open: Vec<String> = vec![];
        if quiet || std::env::var("YH_DATA_DIR").is_ok() {
            open.push("-g".into());
        }
        let mut vars: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k.starts_with("YH_") && k != "YH_UPDATE_RESTART" && k != "YH_UPDATE_DOWNLOAD").collect();
        vars.sort();
        for (k, v) in vars {
            open.push("--env".into());
            open.push(format!("{k}={v}"));
        }
        // The quit question first (background jobs stop with the app), then the swap.
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            crate::ui::jobs::ask_then(
                Box::new(move |cx| {
                    let swapped = updates::swap_after_quit(&app, &open);
                    match swapped {
                        Ok(()) => crate::ui::jobs::quit_now(cx),
                        Err(e) => {
                            let _ = this.update(cx, |u, cx| u.set(Phase::Failed(e, None), cx));
                        }
                    }
                }),
                cx,
            )
        });
    }

    /// Restart now, as Restart's question answered yes does.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Ready(_)) {
            self.restart_now(false, cx);
        }
    }

    /// Restart, asked first: "Restart to update to X?", saying what happens to threads working
    /// (`YH_RESTART_ANSWER=restart|later` answers it without showing it, printing it).
    pub fn confirm_restart(&mut self, working: usize, window: &mut gpui_kit::Window, cx: &mut Context<Self>) {
        let Phase::Ready(version) = self.phase.clone() else { return };
        let title = format!("Restart to update to {version}?");
        let message = Self::restart_message(&version, working);
        if let Ok(answer) = std::env::var("YH_RESTART_ANSWER") {
            eprintln!("UPDATE asked: {title} / {message} -> {answer}");
            if answer == "restart" {
                self.restart(cx);
            }
            return;
        }
        let answer = window.prompt(gpui_kit::PromptLevel::Info, &title, Some(&message), &["Restart", "Later"], cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |u, cx| u.restart(cx));
            }
        })
        .detach();
    }
}

/// "2 minutes", "1 minute", "45 seconds".
pub fn countdown_words(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} second{}", if s == 1 { "" } else { "s" }),
        _ => {
            let m = (s + 30) / 60;
            format!("{m} minute{}", if m == 1 { "" } else { "s" })
        }
    }
}
