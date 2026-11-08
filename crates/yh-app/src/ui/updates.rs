//! Updates, as the Swift app shows them (UpdateViews.swift): the sidebar's banner while there's
//! something to do (a version to get, its download with Cancel, Restart, a failure, "X is the
//! latest" after a check you asked for, "Updated to X" after an update), the version under the
//! limits with a popover (version, build, release notes, where updates stand), release notes
//! from GitHub, and Settings → Releases' list with Installed and New tags.

use crate::state::updater::{Phase, Updater};
use crate::state::workspace::Workspace;
use crate::theme::{radius, space, text, Theme};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::{div, prelude::*, px, AnyElement, App, Context, Entity, FontWeight, IntoElement, SharedString, Window};
use yh_core::updates::{Notes, Version, RELEASES_PAGE};

/// How many threads are working now, for Restart's question.
pub fn working(ws: &Entity<Workspace>, cx: &App) -> usize {
    ws.read(cx).threads.iter().filter(|t| t.read(cx).running).count()
}

/// Release notes, from `after` (exclusive) through `through`; without `after`, only `through`'s.
pub struct ReleaseNotesView {
    updater: Entity<Updater>,
    pub after: Option<Version>,
    pub through: Option<Version>,
    _watch: gpui_kit::Subscription,
}

impl ReleaseNotesView {
    pub fn new(updater: Entity<Updater>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&updater, |_, _, cx| cx.notify());
        ReleaseNotesView { updater, after: None, through: None, _watch: watch }
    }

    /// Shows these releases' notes, reading them if they aren't yet.
    pub fn show(&mut self, after: Option<Version>, through: Version, cx: &mut Context<Self>) {
        self.after = after;
        self.through = Some(through);
        self.updater.update(cx, |u, cx| u.load_notes(cx));
        cx.notify();
    }
}

impl Render for ReleaseNotesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let u = self.updater.read(cx);
        let Some(through) = self.through.clone() else { return div() };
        let mut notes = u.notes.as_ref().map(|(all, _)| yh_core::updates::notes_between(all, self.after.as_ref(), &through)).unwrap_or_default();
        // The release an update offers is already known, before the list arrives.
        if notes.is_empty() {
            if let Phase::Available(r) = &u.phase {
                if r.version == through && !r.notes.is_empty() {
                    notes.push(Notes { version: r.version.clone(), text: yh_core::updates::whats_new(&r.notes), page: r.page.clone(), published_at: None });
                }
            }
        }
        let title = match &self.after {
            Some(a) if *a < through && notes.len() > 1 => format!("What's New Since {a}"),
            _ => format!("What's New in {through}"),
        };
        let mut list = div().id("release-notes-list").flex().flex_col().gap(px(space::L + 6.)).p(px(space::L)).h(px(380.)).overflow_y_scroll();
        if notes.is_empty() {
            list = list.child(div().text_color(theme.secondary).child(if u.notes_loading {
                "Loading the notes from GitHub…".to_string()
            } else if let Some(e) = &u.notes_error {
                format!("Couldn't load the notes: {e}")
            } else {
                format!("There are no notes for {through}.")
            }));
        }
        let several = notes.len() > 1;
        for n in &notes {
            list = list.child(section(n, several, None, &theme));
        }
        let offers = matches!(&u.phase, Phase::Available(r) if r.version == through);
        let updater = self.updater.clone();
        div()
            .w(px(460.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::L))
                    .pt(px(space::L))
                    .pb(px(space::S + 2.))
                    .child(div().flex_1().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(u.notes_loading && !notes.is_empty(), |d| d.child(Icon::new(IconName::LoaderCircle).size(px(12.)).text_color(theme.tertiary))),
            )
            .child(div().h(px(1.)).bg(theme.separator))
            .child(list)
            .child(div().h(px(1.)).bg(theme.separator))
            .child(
                div()
                    .flex()
                    .items_center()
                    .p(px(space::M))
                    .child(div().id("all-releases").text_color(theme.accent).text_size(px(text::FOOTNOTE)).child("All Releases on GitHub").on_click(|_, _, cx| cx.open_url(RELEASES_PAGE)))
                    .child(div().flex_1())
                    .when(offers, |d| d.child(Button::new("notes-update").small().primary().label("Update").on_click(move |_, _, cx| updater.update(cx, |u, cx| u.download(cx))))),
            )
    }
}

/// One release's notes: its version, a tag, its date, a link to its page, then the notes.
pub fn section(n: &Notes, shows_version: bool, tag: Option<(&'static str, gpui_kit::Hsla)>, theme: &Theme) -> AnyElement {
    let page = n.page.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(space::S + 2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S))
                .when(shows_version, |d| d.child(div().font_weight(FontWeight::SEMIBOLD).child(n.version.to_string())))
                .when_some(tag, |d, (label, tint)| {
                    d.child(div().px(px(6.)).rounded(px(radius::SMALL)).bg(tint.opacity(0.15)).text_color(tint).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).child(label))
                })
                .when_some(n.published_at, |d, t| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(day(t))))
                .child(div().flex_1())
                .when_some(page, |d, page| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("release-page-{}", n.version)))
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .text_size(px(text::CAPTION))
                            .text_color(theme.accent)
                            .child("View on GitHub")
                            .child(Icon::new(IconName::ExternalLink).size(px(11.)).text_color(theme.accent))
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                }),
        )
        .child(markdown(if n.text.is_empty() { "No notes for this release." } else { &n.text }, theme))
        .into_any_element()
}

/// "Nov 7, 2026".
fn day(t: f64) -> String {
    let full = yh_core::clock::short_date_time(t);
    full.split(" at ").next().unwrap_or(&full).to_string()
}

/// Release notes' markdown, the parts they use: headings, bullets, paragraphs; emphasis and
/// code marks dropped.
fn markdown(md: &str, theme: &Theme) -> AnyElement {
    let clean = |s: &str| s.replace("**", "").replace('`', "");
    let mut out = div().flex().flex_col().gap(px(space::XS + 1.)).text_size(px(text::FOOTNOTE));
    for line in md.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        let trimmed = t.trim_start();
        if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            out = out.child(div().pt(px(space::XS)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(clean(h)));
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let indent = (t.len() - trimmed.len()) as f32 * 4.;
            out = out.child(div().flex().gap(px(space::S)).pl(px(indent)).text_color(theme.secondary).child(div().flex_none().child("•")).child(div().flex_1().min_w_0().child(clean(item))));
        } else {
            out = out.child(div().text_color(theme.secondary).child(clean(trimmed)));
        }
    }
    out.into_any_element()
}

/// The sidebar's banner while there's something to do about an update; None the rest of the time.
pub fn banner(updater: &Entity<Updater>, workspace: &Entity<Workspace>, notes: &Entity<ReleaseNotesView>, theme: &Theme, cx: &App) -> Option<AnyElement> {
    let u = updater.read(cx);
    let icon = |name: Icon, color: gpui_kit::Hsla| name.size(px(16.)).text_color(color);
    let small = |t: String| div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(t);
    let strong = |t: String| div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).child(t);
    let dismiss = |id: &'static str, tip: &'static str, f: Box<dyn Fn(&mut App)>| {
        Button::new(id).xsmall().ghost().icon(Icon::new(IconName::CircleX).size(px(13.)).text_color(theme.secondary)).tooltip(tip).on_click(move |_, _, cx| f(cx))
    };
    let whats_new = |id: &'static str, after: Option<Version>, through: Version| {
        let notes = notes.clone();
        let n2 = notes.clone();
        Popover::new(id)
            .anchor(gpui_kit::Anchor::BottomLeft)
            .trigger(Button::new(SharedString::from(format!("{id}-button"))).xsmall().ghost().label("What's New"))
            .content(move |_, _, _| n2.clone())
            .on_open_change(move |open, _, cx| {
                if *open {
                    let (a, t) = (after.clone(), through.clone());
                    notes.update(cx, |n, cx| n.show(a, t, cx));
                }
            })
    };
    let (u1, u2) = (updater.clone(), updater.clone());
    let row = div().flex().items_center().gap(px(space::S));
    let body = match u.phase.clone() {
        Phase::Available(r) => row
            .child(icon(crate::ui::icons::download(), theme.accent))
            .child(div().flex_1().min_w_0().flex().flex_col().child(strong(format!("Version {}", r.version))).child(whats_new("update-notes", u.current.clone(), r.version.clone())))
            .child(Button::new("update-action").xsmall().primary().label("Update").tooltip(format!("Download {}. Nothing changes until you restart.", r.version)).on_click(move |_, _, cx| u1.update(cx, |u, cx| u.download(cx))))
            .into_any_element(),
        Phase::Downloading(r, p) => div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                row.child(div().flex_1().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::MEDIUM).child(format!("Downloading {}", r.version)))
                    .child(small(format!("{:.0}%", p * 100.)))
                    .child(dismiss("update-cancel", "Stop downloading", Box::new(move |cx| u1.update(cx, |u, cx| u.cancel_download(cx))))),
            )
            .child(div().h(px(4.)).rounded_full().bg(theme.selected).child(div().h_full().rounded_full().w(gpui_kit::relative(p.clamp(0., 1.))).bg(theme.accent)))
            .into_any_element(),
        Phase::Preparing(r) => row.child(icon(Icon::new(IconName::LoaderCircle), theme.secondary)).child(div().text_size(px(text::FOOTNOTE)).child(format!("Preparing {}…", r.version))).into_any_element(),
        Phase::Ready(v) => {
            let ws = workspace.clone();
            row.child(icon(Icon::new(IconName::CircleCheck), theme.success))
                .child(div().flex_1().min_w_0().flex().flex_col().child(strong(format!("{v} is ready"))).child(small("Restart to use it".into())))
                .child(Button::new("update-action").xsmall().primary().label("Restart").tooltip(format!("Quit and open {v}")).on_click(move |_, window, cx| {
                    let n = working(&ws, cx);
                    u1.update(cx, |u, cx| u.confirm_restart(n, window, cx))
                }))
                .into_any_element()
        }
        Phase::Failed(e, release) => row
            .items_start()
            .child(icon(Icon::new(IconName::TriangleAlert), theme.warning))
            .child(div().flex_1().min_w_0().flex().flex_col().child(strong(if release.is_none() { "Couldn't check for updates".into() } else { "Couldn't update".into() })).child(small(e).line_clamp(3)))
            .child(if release.is_some() {
                Button::new("update-action").xsmall().outline().label("Retry").on_click(move |_, _, cx| u1.update(cx, |u, cx| u.download(cx))).into_any_element()
            } else {
                dismiss("update-dismiss", "Dismiss", Box::new(move |cx| u1.update(cx, |u, cx| u.dismiss_failure(cx)))).into_any_element()
            })
            .into_any_element(),
        Phase::UpToDate if u.announces => row.child(icon(Icon::new(IconName::CircleCheck), theme.success)).child(div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::MEDIUM).child(format!("{} is the latest", u.version_label()))).into_any_element(),
        Phase::Checking if u.announces => row.child(icon(Icon::new(IconName::LoaderCircle), theme.secondary)).child(div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::MEDIUM).child("Checking for updates…")).into_any_element(),
        _ => {
            let (Some(from), Some(current)) = (u.updated_from.clone(), u.current.clone()) else { return None };
            row.child(icon(crate::ui::icons::sparkles(), theme.accent))
                .child(div().flex_1().min_w_0().flex().flex_col().child(strong(format!("Updated to {current}"))).child(whats_new("updated-notes", Some(from), current)))
                .child(dismiss("updated-dismiss", "Hide this", Box::new(move |cx| u2.update(cx, |u, cx| u.dismiss_updated(cx)))))
                .into_any_element()
        }
    };
    Some(body)
}

/// Where updates stand and what you can do next, in a line: for Settings and the version popover.
pub fn status(updater: &Entity<Updater>, workspace: &Entity<Workspace>, notes: &Entity<ReleaseNotesView>, theme: &Theme, cx: &App) -> AnyElement {
    let u = updater.read(cx);
    let row = div().flex().items_center().gap(px(space::S)).min_h(px(28.));
    if let Some(why) = u.why_not() {
        return row.child(div().text_color(theme.secondary).child(why)).into_any_element();
    }
    let u1 = updater.clone();
    let check = move || {
        let u = u1.clone();
        Button::new("status-check").small().outline().label("Check Now").on_click(move |_, _, cx| u.update(cx, |u, cx| u.check_now(cx)))
    };
    let u2 = updater.clone();
    match u.phase.clone() {
        Phase::Idle | Phase::UpToDate | Phase::Failed(_, None) => {
            let label = match &u.phase {
                Phase::UpToDate => "Up to date".to_string(),
                Phase::Failed(e, _) => format!("Couldn't check: {e}"),
                _ => "Not checked yet".to_string(),
            };
            let checked = u.last_checked.map(|t| format!("Checked {}", yh_core::clock::relative(t, yh_core::library::now_ms() / 1000.)));
            row.child(div().flex_1().min_w_0().flex().flex_col().child(div().child(label)).when_some(checked, |d, c| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(c)))).child(check()).into_any_element()
        }
        Phase::Checking => row.child(Icon::new(IconName::LoaderCircle).size(px(14.)).text_color(theme.secondary)).child("Checking…").into_any_element(),
        Phase::Available(r) => {
            let (notes, current, v) = (notes.clone(), u.current.clone(), r.version.clone());
            let n2 = notes.clone();
            row.child(div().flex_1().child(format!("{} is available", r.version)))
                .child(
                    Popover::new("status-notes")
                        .trigger(Button::new("status-notes-button").small().ghost().label("What's New"))
                        .content(move |_, _, _| n2.clone())
                        .on_open_change(move |open, _, cx| {
                            if *open {
                                let (a, t) = (current.clone(), v.clone());
                                notes.update(cx, |n, cx| n.show(a, t, cx));
                            }
                        }),
                )
                .child(Button::new("status-update").small().primary().label("Update").on_click(move |_, _, cx| u2.update(cx, |u, cx| u.download(cx))))
                .into_any_element()
        }
        Phase::Downloading(r, p) => row
            .child(div().child(format!("Downloading {}", r.version)))
            .child(div().w(px(120.)).h(px(4.)).rounded_full().bg(theme.selected).child(div().h_full().rounded_full().w(gpui_kit::relative(p.clamp(0., 1.))).bg(theme.accent)))
            .child(div().flex_1())
            .child(Button::new("status-cancel").small().outline().label("Cancel").on_click(move |_, _, cx| u2.update(cx, |u, cx| u.cancel_download(cx))))
            .into_any_element(),
        Phase::Preparing(r) => row.child(Icon::new(IconName::LoaderCircle).size(px(14.)).text_color(theme.secondary)).child(format!("Preparing {}…", r.version)).into_any_element(),
        Phase::Ready(v) => {
            let ws = workspace.clone();
            row.child(div().flex_1().child(format!("{v} is ready")))
                .child(Button::new("status-restart").small().primary().label("Restart").on_click(move |_, window, cx| {
                    let n = working(&ws, cx);
                    u2.update(cx, |u, cx| u.confirm_restart(n, window, cx))
                }))
                .into_any_element()
        }
        Phase::Failed(e, Some(_)) => row
            .child(div().flex_1().min_w_0().text_color(theme.warning).child(e))
            .child(Button::new("status-retry").small().outline().label("Retry").on_click(move |_, _, cx| u2.update(cx, |u, cx| u.download(cx))))
            .into_any_element(),
    }
}

/// The version, small, under the limits; click for where it stands.
pub fn version_button(updater: &Entity<Updater>, workspace: &Entity<Workspace>, notes: &Entity<ReleaseNotesView>, theme: &Theme, cx: &App) -> AnyElement {
    let label = updater.read(cx).version_label();
    let (updater, workspace, notes) = (updater.clone(), workspace.clone(), notes.clone());
    let theme = *theme;
    Popover::new("version-popover")
        .anchor(gpui_kit::Anchor::BottomLeft)
        .default_open(std::env::var("YH_OPEN_VERSION").is_ok())
        .trigger(Button::new("version-button").xsmall().ghost().child(div().text_size(px(text::CAPTION)).text_color(theme.tertiary).child(label.clone())).tooltip(format!("YourHarness {label}")))
        .content(move |_, _, cx| {
            let u = updater.read(cx);
            let current = u.current.clone();
            let n = notes.clone();
            div()
                .w(px(300.))
                .p(px(space::M + 2.))
                .flex()
                .flex_col()
                .gap(px(space::S + 2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S + 2.))
                        .child(div().size(px(40.)).rounded(px(radius::LARGE - 3.)).bg(theme.accent_soft).flex().items_center().justify_center().child(crate::ui::icons::bot().size(px(22.)).text_color(theme.accent)))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("YourHarness {}", u.version_label())))
                                .when_some(u.build.clone(), |d, b| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("Build {b}")))),
                        )
                        .when_some(current, |d, v| {
                            let n2 = n.clone();
                            d.child(
                                Popover::new("current-notes")
                                    .trigger(Button::new("current-notes-button").small().ghost().label("Release Notes").tooltip(format!("What changed in {v}")))
                                    .content(move |_, _, _| n2.clone())
                                    .on_open_change(move |open, _, cx| {
                                        if *open {
                                            let v = v.clone();
                                            n.update(cx, |n, cx| n.show(None, v, cx));
                                        }
                                    }),
                            )
                        }),
                )
                .child(status(&updater, &workspace, &notes, &theme, cx))
        })
        .into_any_element()
}

/// Settings → Releases' list: every release's notes, Installed and New tagged.
pub fn all_notes(updater: &Entity<Updater>, theme: &Theme, cx: &App) -> AnyElement {
    let u = updater.read(cx);
    let mut list = div().flex().flex_col().gap(px(space::M));
    match &u.notes {
        Some((all, _)) if !all.is_empty() => {
            for n in all.iter().take(20) {
                let tag = u.current.as_ref().and_then(|c| match n.version.cmp(c) {
                    std::cmp::Ordering::Equal => Some(("Installed", theme.success)),
                    std::cmp::Ordering::Greater => Some(("New", theme.accent)),
                    _ => None,
                });
                list = list.child(div().p(px(space::M + 2.)).rounded(px(radius::LARGE - 2.)).bg(theme.card).border_1().border_color(theme.separator).child(section(n, true, tag, theme)));
            }
        }
        _ => {
            list = list.child(div().text_color(theme.secondary).child(if u.notes_loading {
                "Loading the notes from GitHub…".to_string()
            } else if let Some(e) = &u.notes_error {
                format!("Couldn't load the notes: {e}")
            } else {
                "No releases yet.".to_string()
            }))
        }
    }
    list.into_any_element()
}
