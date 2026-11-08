//! The menu bar, the app's commands plus what every Mac app has: About, Services, Hide; File with Close Window; Edit; View with Enter Full
//! Screen; Thread; Window; Help; and Debug in development builds. Titles that follow state
//! ("Hide Usage", "Hide Right Sidebar") and items that can't do anything now are set again
//! when what they show changes (`MenuState`), not on every change.

use crate::state::workspace::{Page, Workspace};
use crate::ui::root::*;
use gpui_kit::{App, Entity, Menu, MenuItem, OsAction, SystemMenuType};

/// What the menus show that changes.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct MenuState {
    pub usage_open: bool,
    pub inspector_open: bool,
    /// A thread is on screen (not a page): Find, the message jumps, the right sidebar.
    pub thread_visible: bool,
    pub running: bool,
    pub finds: bool,
    pub waiting: bool,
}

impl MenuState {
    pub fn of(ws: &Workspace, finds: bool, cx: &App) -> MenuState {
        let thread = if ws.page.is_none() { ws.selected_thread() } else { None };
        MenuState {
            usage_open: ws.page == Some(Page::Usage),
            inspector_open: ws.shows_inspector,
            thread_visible: thread.is_some(),
            running: thread.as_ref().is_some_and(|t| t.read(cx).running),
            waiting: thread.as_ref().is_some_and(|t| t.read(cx).waiting_for_you()),
            finds: finds && thread.is_some(),
        }
    }
}

/// Development builds (`.next`, `.dev`, or not a bundle) get a Debug menu.
pub fn is_development() -> bool {
    let exe = std::env::current_exe().ok();
    let bundle = exe.as_ref().and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf())).filter(|b| b.extension().is_some_and(|e| e == "app"));
    match bundle.as_deref().and_then(yh_core::updates::bundle_info) {
        Some((id, _)) => id.ends_with(".next") || id.ends_with(".dev"),
        None => true,
    }
}

pub fn set(state: &MenuState, cx: &mut App) {
    let s = state;
    let mut menus = vec![
        Menu {
            name: "YourHarness".into(),
            items: vec![
                MenuItem::action("About YourHarness", About),
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide YourHarness", HideApp),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit YourHarness", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Thread", NewThread),
                MenuItem::action("Add Project…", AddProject),
                MenuItem::separator(),
                MenuItem::action("Resume Claude Code Session…", ResumeSession),
                MenuItem::separator(),
                MenuItem::action("Export State…", ExportState),
                MenuItem::action("Import State…", ImportState),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", gpui_kit::component::input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", gpui_kit::component::input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", gpui_kit::component::input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", gpui_kit::component::input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", gpui_kit::component::input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", gpui_kit::component::input::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Search Everything…", OpenSpotlight),
                MenuItem::action("Find in Thread…", Find).disabled(!s.thread_visible),
                MenuItem::action("Find Next", FindNext).disabled(!s.finds),
                MenuItem::action("Find Previous", FindPrevious).disabled(!s.finds),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Memory", OpenMemory),
                MenuItem::action(if s.usage_open { "Hide Usage" } else { "Usage" }, ToggleUsage),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action(if s.inspector_open { "Hide Right Sidebar" } else { "Show Right Sidebar" }, ToggleInspector).disabled(!s.thread_visible),
                MenuItem::separator(),
                MenuItem::action("Enter Full Screen", ToggleFullScreen),
            ],
            disabled: false,
        },
        Menu {
            name: "Thread".into(),
            items: vec![
                MenuItem::action("Previous Thread", PreviousThread),
                MenuItem::action("Next Thread", NextThread),
                MenuItem::separator(),
                MenuItem::action("Previous Message", PreviousMessage).disabled(!s.thread_visible),
                MenuItem::action("Next Message", NextMessage).disabled(!s.thread_visible),
                MenuItem::action("Top of Thread", TopOfThread).disabled(!s.thread_visible),
                MenuItem::action("End of Thread", EndOfThread).disabled(!s.thread_visible),
                MenuItem::separator(),
                MenuItem::action("Stop", Stop).disabled(!s.running),
                MenuItem::action("Allow", Allow).disabled(!s.waiting),
                MenuItem::action("Deny", Deny).disabled(!s.waiting),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Bring All to Front", BringAllToFront),
            ],
            disabled: false,
        },
    ];
    if is_development() {
        menus.push(Menu {
            name: "Debug".into(),
            items: vec![MenuItem::action("Capture Transcript State", CaptureTranscriptState).disabled(!s.thread_visible), MenuItem::action("Show Data Folder", ShowDataFolder)],
            disabled: false,
        });
    }
    menus.push(Menu { name: "Help".into(), items: vec![MenuItem::action("YourHarness Help", OpenHelp)], disabled: false });
    cx.set_menus(menus);
    if std::env::var("YH_MENU_LOG").is_ok() {
        eprintln!("MENUS {s:?}");
    }
}

/// Sets the menus now, and again whenever what they show changes.
pub fn follow(workspace: Entity<Workspace>, find: Entity<crate::ui::find::FindBar>, cx: &mut App) {
    let state = MenuState::of(workspace.read(cx), find.read(cx).has_hits(), cx);
    set(&state, cx);
    let last = std::rc::Rc::new(std::cell::RefCell::new(state));
    let refresh = {
        let (workspace, find, last) = (workspace.clone(), find.clone(), last.clone());
        move |cx: &mut App| {
            let state = MenuState::of(workspace.read(cx), find.read(cx).has_hits(), cx);
            if *last.borrow() != state {
                set(&state, cx);
                *last.borrow_mut() = state;
            }
        }
    };
    let r = refresh.clone();
    cx.observe(&workspace, move |_, cx| r(cx)).detach();
    cx.observe(&find, move |_, cx| refresh(cx)).detach();
}

/// The help page: the project's README on GitHub.
pub const HELP_URL: &str = "https://github.com/mdovhopo/yourharness#readme";
