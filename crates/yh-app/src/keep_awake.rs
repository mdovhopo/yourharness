//! Keeps the Mac from sleeping on its own while Claude works, as `caffeinate` does, with the
//! same power assertion and no process of its own. Nothing ticks: it changes only when work
//! starts or stops, when a setting changes, and at the one deadline it has, if any. Settings
//! (`keepAwake`, as 0.0.x saved it): on/off, while working or always, the display too, and
//! minutes to stay awake after the last turn. The sidebar's cup asks for 1, 2 or 4 hours, or
//! until you stop it.

#![allow(non_upper_case_globals)]

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use serde_json::Value;
use std::time::{Duration, Instant};

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(kind: core_foundation::string::CFStringRef, level: u32, name: core_foundation::string::CFStringRef, id: *mut u32) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// `whileWorking` or `always`.
    pub mode: String,
    pub keeps_display_on: bool,
    pub after_work_minutes: u64,
}

impl Settings {
    pub fn from(v: Option<Value>) -> Settings {
        let v = v.unwrap_or_default();
        Settings {
            enabled: v["enabled"].as_bool().unwrap_or(true),
            mode: v["mode"].as_str().unwrap_or("whileWorking").to_string(),
            keeps_display_on: v["keepsDisplayOn"].as_bool().unwrap_or(false),
            after_work_minutes: v["afterWorkMinutes"].as_u64().unwrap_or(0),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({ "enabled": self.enabled, "mode": self.mode, "keepsDisplayOn": self.keeps_display_on, "afterWorkMinutes": self.after_work_minutes })
    }
}

/// Why the Mac is being kept awake.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    Working,
    AfterWork(Instant),
    Always,
    /// Asked for from the cup; None until you stop it.
    Manual(Option<Instant>),
}

pub struct KeepAwake {
    pub settings: Settings,
    working: bool,
    after_work_until: Option<Instant>,
    manual_until: Option<Instant>,
    manual_forever: bool,
    pub reason: Option<Reason>,
    assertion: Option<(u32, bool)>,
}

impl KeepAwake {
    pub fn new(settings: Settings) -> KeepAwake {
        let mut k = KeepAwake { settings, working: false, after_work_until: None, manual_until: None, manual_forever: false, reason: None, assertion: None };
        k.update();
        k
    }

    pub fn configure(&mut self, settings: Settings) {
        self.settings = settings;
        self.update();
    }

    /// Claude started or stopped working in any thread.
    pub fn set_working(&mut self, working: bool) {
        if self.working && !working && self.settings.after_work_minutes > 0 {
            self.after_work_until = Some(Instant::now() + Duration::from_secs(self.settings.after_work_minutes * 60));
        }
        self.working = working;
        self.update();
    }

    /// Awake for this long, or until stopped with None.
    pub fn keep_awake(&mut self, duration: Option<Duration>) {
        match duration {
            Some(d) => {
                self.manual_until = Some(Instant::now() + d);
                self.manual_forever = false;
            }
            None => {
                self.manual_forever = true;
                self.manual_until = None;
            }
        }
        self.update();
    }

    pub fn stop_manual(&mut self) {
        self.manual_forever = false;
        self.manual_until = None;
        self.update();
    }

    pub fn has_manual(&self) -> bool {
        self.manual_forever || self.manual_until.is_some()
    }

    /// The next moment something changes, for the caller's one timer.
    pub fn deadline(&self) -> Option<Instant> {
        [self.manual_until, self.after_work_until].into_iter().flatten().min()
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        if self.manual_until.is_some_and(|u| u <= now) {
            self.manual_until = None;
        }
        if self.after_work_until.is_some_and(|u| u <= now) {
            self.after_work_until = None;
        }
        self.reason = if self.manual_forever {
            Some(Reason::Manual(None))
        } else if let Some(u) = self.manual_until {
            Some(Reason::Manual(Some(u)))
        } else if !self.settings.enabled {
            None
        } else if self.settings.mode == "always" {
            Some(Reason::Always)
        } else if self.working {
            Some(Reason::Working)
        } else {
            self.after_work_until.map(Reason::AfterWork)
        };
        // Tests never hold the Mac awake.
        if std::env::var("YH_DATA_DIR").is_ok() && std::env::var("YH_KEEP_AWAKE").is_err() {
            return;
        }
        self.hold(self.reason.is_some(), self.settings.keeps_display_on);
    }

    fn hold(&mut self, on: bool, display: bool) {
        if let Some((id, kept_display)) = self.assertion {
            if !on || display != kept_display {
                unsafe { IOPMAssertionRelease(id) };
                self.assertion = None;
            }
        }
        if !on || self.assertion.is_some() {
            return;
        }
        let kind = CFString::new(if display { "PreventUserIdleDisplaySleep" } else { "PreventUserIdleSystemSleep" });
        let name = CFString::new("YourHarness: keeping the Mac awake while Claude works");
        let mut id = 0u32;
        // kIOPMAssertionLevelOn = 255
        if unsafe { IOPMAssertionCreateWithName(kind.as_concrete_TypeRef(), 255, name.as_concrete_TypeRef(), &mut id) } == 0 {
            self.assertion = Some((id, display));
        }
    }

    /// The cup's words: why it's awake, or that it isn't.
    pub fn describe(&self) -> String {
        match &self.reason {
            None => "The Mac may sleep".into(),
            Some(Reason::Working) => "Awake while Claude works".into(),
            Some(Reason::Always) => "Awake while YourHarness is open".into(),
            Some(Reason::AfterWork(u)) | Some(Reason::Manual(Some(u))) => {
                let mins = u.saturating_duration_since(Instant::now()).as_secs().div_ceil(60);
                if mins >= 60 { format!("Awake for {}h {}m more", mins / 60, mins % 60) } else { format!("Awake for {mins}m more") }
            }
            Some(Reason::Manual(None)) => "Awake until you stop it".into(),
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Some((id, _)) = self.assertion.take() {
            unsafe { IOPMAssertionRelease(id) };
        }
    }
}
