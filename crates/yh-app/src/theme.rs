//! The app's palette, light and dark.

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
    pub const LARGE_TITLE: f32 = 19.;
    /// Line heights that go with them.
    pub const BODY_LINE: f32 = 19.;
    pub const CODE_LINE: f32 = 17.;
    /// Compact monospaced output (tool results, diffs), and headings.
    pub const MONO_LINE: f32 = 16.;
    pub const TITLE_LINE: f32 = 21.;
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
    /// Secondary brand accent (the teal strand of the YourHarness logo): complements the
    /// primary violet accent, e.g. to distinguish secondary agent providers at a glance.
    pub fn teal(&self) -> Hsla {
        if self.canvas.l < 0.5 { rgb(0x5eead4).into() } else { rgb(0x0d9488).into() }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme {
                canvas: rgb(0x16191e).into(),
                sidebar: rgb(0x1c2026).into(),
                text: rgb(0xededf2).into(),
                secondary: rgb(0x9a9ca8).into(),
                separator: rgba(0xffffff1a).into(),
                card: rgb(0x22272f).into(),
                bubble: rgb(0x2a303b).into(),
                code: rgb(0x1b1f25).into(),
                accent: rgb(0x9575fa).into(),
                selected: rgba(0xffffff19).into(),
                error: rgb(0xff6961).into(),
                warning: rgb(0xff9f0a).into(),
                keyword: rgb(0xfc5fa3).into(),
                string: rgb(0xfc6a5d).into(),
                number: rgb(0xd0bf69).into(),
                comment: rgb(0x6c7986).into(),
                tertiary: rgb(0x6c6e7a).into(),
                hover: rgba(0xffffff0e).into(),
                accent_soft: rgba(0x9575fa29).into(),
                success: rgb(0x32d74b).into(),
            },
            _ => Theme {
                canvas: rgb(0xffffff).into(),
                sidebar: rgb(0xf4f4f8).into(),
                text: rgb(0x1b1c22).into(),
                secondary: rgb(0x646572).into(),
                separator: rgba(0x00000016).into(),
                card: rgb(0xf8f8fb).into(),
                bubble: rgb(0xedeef5).into(),
                code: rgb(0xf5f6fa).into(),
                accent: rgb(0x6d4aff).into(),
                selected: rgba(0x00000012).into(),
                error: rgb(0xd70015).into(),
                warning: rgb(0xe58600).into(),
                keyword: rgb(0xad3da4).into(),
                string: rgb(0xd12f1b).into(),
                number: rgb(0x272ad8).into(),
                comment: rgb(0x707f8c).into(),
                tertiary: rgb(0x9495a3).into(),
                hover: rgba(0x0000000a).into(),
                accent_soft: rgba(0x6d4aff1a).into(),
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
    // Its primary buttons in the app's violet accent, deepened slightly in dark mode so white
    // button text stays crisp and accessible.
    let ours = Theme::get(cx);
    let dark = matches!(mode, ThemeMode::Dark);
    let primary: Hsla = if dark { rgb(0x7c5cfa).into() } else { ours.accent };
    let theme = ComponentTheme::global_mut(cx);
    let colors = &mut theme.colors;
    // Dialogs and fields on the app's canvas, not the kit's near-black in dark mode.
    colors.background = ours.canvas;
    colors.primary = primary;
    colors.primary_hover = primary.opacity(0.9);
    colors.primary_active = primary.opacity(0.8);
    colors.primary_foreground = gpui_kit::white();
    colors.button_primary = primary;
    colors.button_primary_hover = primary.opacity(0.9);
    colors.button_primary_active = primary.opacity(0.8);
    colors.button_primary_foreground = gpui_kit::white();
    // A switch's knob is white in both modes, as macOS's (it took the near-black canvas in dark).
    colors.switch_thumb = gpui_kit::white();
    // Menus and popovers on the app's surfaces, not the kit's near-black: a card in dark mode
    // (lighter than the canvas, as macOS's menus are), white in light. A menu's lit row is the
    // soft accent, as the sidebar's selection is soft (the patched menu reads `list_active`).
    colors.popover = if dark { ours.card } else { ours.canvas };
    colors.popover_foreground = ours.text;
    colors.list_active = ours.accent_soft;
    // Buttons read tokens made from the colours.
    theme.tokens = gpui_kit::component::theme::ThemeTokens::from(&theme.colors);
}

