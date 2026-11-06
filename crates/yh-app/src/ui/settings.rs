//! Settings (⌘,): defaults for new threads, generated titles, notifications, appearance. Saved
//! in the Swift app's settings (`defaults`, `titleGeneration`, `notifications`, `appearance`).

use crate::state::workspace::Workspace;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::{div, prelude::*, px, App, Entity, Window};
use serde_json::{json, Value};

fn row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    div().flex().items_center().justify_between().gap(px(16.)).py(px(4.)).child(label).child(control)
}

fn picker(id: &'static str, current: String, options: Vec<(String, String)>, on_pick: impl Fn(String, &mut Window, &mut App) + 'static) -> impl IntoElement {
    let label = options.iter().find(|(v, _)| *v == current).map(|(_, l)| l.clone()).unwrap_or(current.clone());
    let on_pick = std::rc::Rc::new(on_pick);
    Button::new(id).small().outline().label(label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
        for (value, label) in &options {
            let (on_pick, value) = (on_pick.clone(), value.clone());
            menu = menu.item(PopupMenuItem::new(label.clone()).checked(value == current).on_click(move |_, window, cx| on_pick(value.clone(), window, cx)));
        }
        menu
    })
}

/// What Settings opens besides itself: the MCP sheet and the updater (set by the window).
pub struct SettingsLinks {
    pub mcp: Entity<crate::ui::mcp::McpSheet>,
    pub updater: Entity<crate::state::updater::Updater>,
}

impl gpui_kit::Global for SettingsLinks {}

/// Opens the dialog; each change is saved at once.
pub fn open(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let (ws_export, ws_import) = (workspace.clone(), workspace.clone());
    let (mcp, updater) = cx.try_global::<SettingsLinks>().map(|l| (Some(l.mcp.clone()), Some(l.updater.clone()))).unwrap_or((None, None));
    window.open_dialog(cx, move |dialog, _, cx| {
        let (ws_export, ws_import) = (ws_export.clone(), ws_import.clone());
        let (mcp, updater) = (mcp.clone(), updater.clone());
        let version_line: &'static str = match updater.as_ref().map(|u| u.read(cx).phase.clone()) {
            Some(crate::state::updater::Phase::UpToDate) => "Up to date",
            Some(crate::state::updater::Phase::Available(_)) => "A new version is out (see the sidebar)",
            Some(crate::state::updater::Phase::Checking) => "Checking…",
            _ => concat!("This is version ", env!("CARGO_PKG_VERSION")),
        };
        let ws = workspace.read(cx);
        let lib = ws.library.clone();
        let defaults = lib.setting("defaults").unwrap_or(json!({}));
        let model = defaults["model"].as_str().unwrap_or("").to_string();
        let effort = defaults["effort"].as_str().unwrap_or("").to_string();
        let titles_on = lib.setting("titleGeneration").map_or(true, |t| t["provider"].as_str() == Some("claude"));
        let notifications = lib.setting("notifications").and_then(|v| v.as_bool()).unwrap_or(true);
        let appearance = lib.setting("appearance").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "system".into());
        let mut models: Vec<(String, String)> = vec![("".into(), "Claude Code's default".into())];
        models.extend(ws.models.iter().map(|m| (m.value.clone(), m.display_name.clone())));
        let efforts: Vec<(String, String)> = std::iter::once(("".into(), "Default".into()))
            .chain(["low", "medium", "high", "xhigh", "max"].iter().map(|e| (e.to_string(), e[..1].to_uppercase() + &e[1..])))
            .collect();
        let set_default = {
            let lib = lib.clone();
            move |key: &'static str| {
                let lib = lib.clone();
                move |value: String, _: &mut Window, _: &mut App| {
                    let mut d = lib.setting("defaults").unwrap_or(json!({}));
                    if !d.is_object() {
                        d = json!({});
                    }
                    if value.is_empty() {
                        d.as_object_mut().unwrap().remove(key);
                    } else {
                        d[key] = Value::from(value);
                    }
                    lib.set_setting("defaults", &d);
                }
            }
        };
        let (l1, l2, l3) = (lib.clone(), lib.clone(), lib.clone());
        let on_off = || vec![("on".to_string(), "On".to_string()), ("off".to_string(), "Off".to_string())];
        dialog
            .title("Settings")
            .w(px(460.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .child(div().text_size(px(11.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("New threads"))
                    .child(row("Model", picker("settings-model", model, models, set_default("model"))))
                    .child(row("Effort", picker("settings-effort", effort, efforts, set_default("effort"))))
                    .child(div().pt(px(8.)).text_size(px(11.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("App"))
                    .child(row(
                        "Generated titles",
                        picker("settings-titles", if titles_on { "on" } else { "off" }.into(), on_off(), move |v, _, _| {
                            let value = if v == "on" { json!({"provider": "claude", "model": "haiku"}) } else { json!({}) };
                            l1.set_setting("titleGeneration", &value);
                        }),
                    ))
                    .child(row(
                        "Notifications",
                        picker("settings-notify", if notifications { "on" } else { "off" }.into(), on_off(), move |v, _, _| {
                            l2.set_setting("notifications", &Value::Bool(v == "on"));
                        }),
                    ))
                    .child(row(
                        "Appearance",
                        picker(
                            "settings-appearance",
                            appearance,
                            vec![("system".into(), "System".into()), ("light".into(), "Light".into()), ("dark".into(), "Dark".into())],
                            move |v, window, cx| {
                                l3.set_setting("appearance", &Value::from(v.clone()));
                                crate::theme::set_chosen_appearance(Some(v));
                                crate::theme::apply_appearance(window.appearance(), window, cx);
                                cx.refresh_windows();
                            },
                        ),
                    ))
                    .child(div().pt(px(8.)).text_size(px(11.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("MCP"))
                    .child(row(
                        "Servers that apply in every project",
                        Button::new("settings-mcp").small().outline().label("MCP Servers…").on_click({
                            let mcp = mcp.clone();
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                if let Some(m) = mcp.clone() {
                                    m.update(cx, |m, cx| m.show_everywhere(cx));
                                }
                            }
                        }),
                    ))
                    .child(div().pt(px(8.)).text_size(px(11.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Releases"))
                    .child(row(
                        version_line,
                        Button::new("settings-updates").small().outline().label("Check for Updates").on_click({
                            let updater = updater.clone();
                            move |_, _, cx| {
                                if let Some(u) = updater.clone() {
                                    u.update(cx, |u, cx| u.check(cx));
                                }
                            }
                        }),
                    ))
                    .child(div().pt(px(8.)).text_size(px(11.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("State"))
                    .child(row(
                        "Everything the app keeps, as one file",
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(Button::new("state-export").small().outline().label("Export…").on_click({
                                let ws = ws_export.clone();
                                move |_, _, cx| crate::ui::state_transfer::export(ws.clone(), cx)
                            }))
                            .child(Button::new("state-import").small().outline().label("Import…").on_click({
                                let ws = ws_import.clone();
                                move |_, window, cx| crate::ui::state_transfer::import(ws.clone(), window, cx)
                            })),
                    )),
            )
    });
}
