//! The web POC's palette (web/ui/src/styles.css), light and dark.

use gpui::{rgb, rgba, App, Global, Hsla, WindowAppearance};

#[derive(Clone, Copy)]
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
}

impl Global for Theme {}

pub const MONO: &str = "Menlo";
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
            },
        }
    }

    pub fn get(cx: &App) -> Theme {
        *cx.global::<Theme>()
    }
}
