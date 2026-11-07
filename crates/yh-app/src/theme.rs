//! The web POC's palette (web/ui/src/styles.css), light and dark.

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{rgb, rgba, App, Global, Hsla, Window, WindowAppearance};

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Theme {
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    pub separator: Hsla,
    pub card: Hsla,
    pub bubble: Hsla,
    pub code: Hsla,
    pub accent: Hsla,
    pub selected: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub comment: Hsla,
    /// Text that matters least (placeholders, disabled), and a row under the pointer.
    pub tertiary: Hsla,
    pub hover: Hsla,
    /// The accent as a quiet background (selected chips, tags), and done/success.
    pub accent_soft: Hsla,
    pub success: Hsla,
}

impl Global for Theme {}

/// The type scale, after macOS's text styles (points): use these, not sizes of your own.
#[allow(dead_code)]
pub mod text {
    /// Labels under and beside things: times, counts, hints.
    pub const CAPTION: f32 = 11.;
    /// Secondary lines: subtitles, footers, metadata.
    pub const FOOTNOTE: f32 = 12.;
    /// Everything else: rows, messages, controls.
    pub const BODY: f32 = 13.;
    /// Code and tool output, a little smaller than body text.
    pub const CODE: f32 = 12.;
    /// Section and card titles (semibold).
    pub const HEADLINE: f32 = 13.;
    /// A view's own title (sheets, panels).
    pub const TITLE: f32 = 15.;
    /// A page's title (Memory, Usage, the project page).
    pub const LARGE_TITLE: f32 = 22.;
    /// Line heights that go with them.
    pub const BODY_LINE: f32 = 19.;
    pub const CODE_LINE: f32 = 17.;
}

/// Corner radii: small for chips and inline code, medium for buttons and rows, large for cards
/// and the composer.
#[allow(dead_code)]
pub mod radius {
    pub const SMALL: f32 = 5.;
    pub const MEDIUM: f32 = 7.;
    pub const LARGE: f32 = 12.;
}

/// Spacing on a 4-point grid.
#[allow(dead_code)]
pub mod space {
    pub const XS: f32 = 4.;
    pub const S: f32 = 8.;
    pub const M: f32 = 12.;
    pub const L: f32 = 16.;
    pub const XL: f32 = 24.;
}

/// SF Mono, the system's monospaced font, by the name Core Text gives it.
pub const MONO: &str = ".AppleSystemUIFontMonospaced";
pub const UI: &str = ".SystemUIFont";

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme {
                canvas: rgb(0x1e1e1e).into(),
                sidebar: rgb(0x252527).into(),
                text: rgb(0xe8e8ea).into(),
                secondary: rgb(0x98989f).into(),
                separator: rgba(0xffffff17).into(),
                card: rgb(0x2a2a2c).into(),
                bubble: rgb(0x333336).into(),
                code: rgb(0x262628).into(),
                accent: rgb(0x0a84ff).into(),
                selected: rgba(0xffffff17).into(),
                error: rgb(0xff6961).into(),
                warning: rgb(0xff9f0a).into(),
                keyword: rgb(0xfc5fa3).into(),
                string: rgb(0xfc6a5d).into(),
                number: rgb(0xd0bf69).into(),
                comment: rgb(0x6c7986).into(),
                tertiary: rgb(0x6a6a70).into(),
                hover: rgba(0xffffff0d).into(),
                accent_soft: rgba(0x0a84ff33).into(),
                success: rgb(0x32d74b).into(),
            },
            _ => Theme {
                canvas: rgb(0xffffff).into(),
                sidebar: rgb(0xf2f2f4).into(),
                text: rgb(0x1d1d1f).into(),
                secondary: rgb(0x6e6e73).into(),
                separator: rgba(0x00000017).into(),
                card: rgb(0xf5f5f7).into(),
                bubble: rgb(0xe9e9ee).into(),
                code: rgb(0xf6f6f8).into(),
                accent: rgb(0x0a84ff).into(),
                selected: rgba(0x00000014).into(),
                error: rgb(0xd70015).into(),
                warning: rgb(0xff9f0a).into(),
                keyword: rgb(0xad3da4).into(),
                string: rgb(0xd12f1b).into(),
                number: rgb(0x272ad8).into(),
                comment: rgb(0x707f8c).into(),
                tertiary: rgb(0xa1a1a6).into(),
                hover: rgba(0x0000000a).into(),
                accent_soft: rgba(0x0a84ff1f).into(),
                success: rgb(0x248a3d).into(),
            },
        }
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }
}

thread_local! {
    /// The appearance chosen in Settings (`system`, `light`, `dark`).
    static APPEARANCE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub fn set_chosen_appearance(value: Option<String>) {
    APPEARANCE.with(|a| *a.borrow_mut() = value);
}

/// The app's palette and gpui-component's (menus, dialogs, inputs) follow light and dark.
pub fn apply_appearance(appearance: WindowAppearance, window: &mut Window, cx: &mut App) {
    // Settings → Appearance (`YH_APPEARANCE` for test runs) over the system's.
    let chosen = std::env::var("YH_APPEARANCE").ok().or_else(|| APPEARANCE.with(|a| a.borrow().clone()));
    let appearance = match chosen.as_deref() {
        Some("light") => WindowAppearance::Light,
        Some("dark") => WindowAppearance::Dark,
        _ => appearance,
    };
    cx.set_global(Theme::for_appearance(appearance));
    let mode = match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        _ => ThemeMode::Light,
    };
    ComponentTheme::change(mode, Some(window), cx);
    // Its primary buttons in the app's accent, as macOS's default buttons are.
    let accent = Theme::get(cx).accent;
    let theme = ComponentTheme::global_mut(cx);
    let colors = &mut theme.colors;
    colors.primary = accent;
    colors.primary_hover = accent.opacity(0.9);
    colors.primary_active = accent.opacity(0.8);
    colors.primary_foreground = gpui_kit::white();
    colors.button_primary = accent;
    colors.button_primary_hover = accent.opacity(0.9);
    colors.button_primary_active = accent.opacity(0.8);
    colors.button_primary_foreground = gpui_kit::white();
    // Buttons read tokens made from the colours.
    theme.tokens = gpui_kit::component::theme::ThemeTokens::from(&theme.colors);
}

