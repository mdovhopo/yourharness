//! One kind of row per item type, as web/ui/src/Rows.tsx. The list asks for a row only while
//! it's on screen (or about to be), and measures it again only when it was spliced.

use yh_core::markdown::{self, MdBlock, Span, Styled as Text};
use crate::state::harness_tools::{media_kind, media_name, media_paths, MediaKind};
use crate::state::thread::{EntryRef, Fold, Item, ItemType, ThreadState, TurnWork};
use yh_core::library::Kind;
use yh_core::links::Linker;
use crate::theme::{radius, space, text as ts, Theme, MONO};
use gpui_kit::{
    div, prelude::*, px, AnyElement, App, ClipboardItem, ElementId, Entity, FontStyle, FontWeight, HighlightStyle, Hsla,
    SharedString, StrikethroughStyle, StyledText, Window,
};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName};
use crate::ui::selectable::SelText;
use crate::ui::steps;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

/// `YH_EXPAND_WORK=1`: folded work and steps drawn open, for looking at them in snapshots.
fn expand_all() -> bool {
    static EXPAND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *EXPAND.get_or_init(|| std::env::var("YH_EXPAND_WORK").as_deref() == Ok("1"))
}

/// What a row's links and buttons need: its thread, its place, what its replies link against.
#[derive(Clone)]
pub struct RowContext {
    pub thread: Entity<ThreadState>,
    pub pos: i64,
    pub thread_id: SharedString,
    pub project_id: SharedString,
    pub linker: Option<Arc<Linker>>,
    /// Still being written: diagrams wait until it's done.
    pub live: bool,
    /// The thread's folder, once its files are read: paths in it open in the Files tab.
    pub root: Option<String>,
}

/// Opens a tab of the right sidebar (Show Browser, Follow in Sidebar): set by the transcript.
pub struct ShowPanel(pub Rc<dyn Fn(crate::state::workspace::Panel, &mut App)>);

impl gpui_kit::Global for ShowPanel {}

fn show_panel(panel: crate::state::workspace::Panel, cx: &mut App) {
    if let Some(show) = cx.try_global::<ShowPanel>().map(|s| s.0.clone()) {
        show(panel, cx);
    }
}

pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

/// A Lucide icon the default set doesn't have, from its SVG paths.
fn lucide(paths: &'static str) -> Icon {
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{paths}</svg>"#
    );
    Icon::default().data(svg.as_bytes())
}

/// Lucide's list-checks: a plan.
fn list_checks_icon() -> Icon {
    lucide(r#"<path d="M13 5h8"/><path d="M13 12h8"/><path d="M13 19h8"/><path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/>"#)
}

/// Lucide's circle-dashed: a task not started yet.
fn circle_dashed_icon() -> Icon {
    lucide(r#"<path d="M10.1 2.182a10 10 0 0 1 3.8 0"/><path d="M13.9 21.818a10 10 0 0 1-3.8 0"/><path d="M17.609 3.721a10 10 0 0 1 2.69 2.7"/><path d="M2.182 13.9a10 10 0 0 1 0-3.8"/><path d="M20.279 17.609a10 10 0 0 1-2.7 2.69"/><path d="M21.818 10.1a10 10 0 0 1 0 3.8"/><path d="M3.721 6.391a10 10 0 0 1 2.7-2.69"/><path d="M6.391 20.279a10 10 0 0 1-2.69-2.7"/>"#)
}

/// Lucide's circle-dot: the task under way.
fn circle_dot_icon() -> Icon {
    lucide(r#"<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="1"/>"#)
}

fn sized(icon: Icon, size: f32, color: Hsla) -> Icon {
    icon.size(px(size)).text_color(color)
}

/// A stable id from what a piece shows, for its buttons: rows have no ids of their own.
fn id_of(name: &'static str, value: impl Hash) -> ElementId {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    ElementId::NamedInteger(name.into(), hasher.finish())
}

pub fn render_item(thread: &Entity<ThreadState>, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = Theme::get(cx);
    let state = thread.read(cx);
    let Some(item) = state.item_at(ix) else { return div().into_any_element() };
    let fold = state.fold_at(ix);
    // Folded into its turn's summary: nothing, and no height.
    if fold == Fold::Hidden {
        return div().into_any_element();
    }
    start_reading_order(item.pos);
    // Find open: this row's texts mark what it looks for (nothing to do when it's closed).
    FIND.with(|f| *f.borrow_mut() = state.find_query.clone().map(|q| (q, state.find_hit == Some(item.pos))));
    let ctx = RowContext {
        thread: thread.clone(),
        pos: item.pos,
        thread_id: state.id().to_string().into(),
        project_id: state.meta.project_id.clone().into(),
        linker: state.linker.clone(),
        live: item.entries.last().is_some_and(|e| e.borrow().live),
        root: state.linker.as_ref().map(|l| l.root.clone()),
    };
    let content = match item.ty {
        ItemType::User => user_row(item, &ctx, &theme, window),
        ItemType::Text => text_row(&item.entries[0], &ctx, &theme, window),
        ItemType::Notice => notice_row(&item.entries[0], &theme),
        ItemType::Work => work_row(state, &ctx, item, &theme),
        ItemType::Agent => agent_row(thread, item, &theme),
        ItemType::Media => media_row(&item.entries[0], &ctx, &theme),
    };
    let content = match fold {
        Fold::Summary { turn, open } => {
            let summary = state.turns.get(turn).map(|work| turn_summary(state, work, open, item.pos, thread, &theme));
            div()
                .flex()
                .flex_col()
                .gap(px(space::XS))
                .children(summary)
                .when(open, |d| d.child(work_box(content, &theme)))
                .into_any_element()
        }
        Fold::Open => work_box(content, &theme),
        _ => content,
    };
    // Texts outside the transcript (the plan card, the right sidebar) never take its marks.
    FIND.with(|f| f.borrow_mut().take());
    let hit = state.find_hit == Some(item.pos);
    // Open work keeps its spacing inside its box, so the rule down its side runs unbroken
    // from row to row.
    let in_box = matches!(fold, Fold::Open);
    // The column: centred, at most 820 wide, as the web transcript's.
    div()
        .when(hit, |d| d.bg(theme.accent.opacity(0.10)))
        .w_full()
        .flex()
        .justify_center()
        .px(px(28.))
        .when(!in_box, |d| d.py(px(space::XS)))
        .child(div().w_full().max_w(px(764.)).child(content))
        .into_any_element()
}

/// A finished turn's work shown open: a hairline down its side, under the summary's chevron,
/// as one group.
fn work_box(content: AnyElement, theme: &Theme) -> AnyElement {
    div().border_l_1().border_color(theme.separator).ml(px(13.)).pl(px(space::M)).py(px(space::XS)).child(content).into_any_element()
}

const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// "9 tool calls · edited 3 files · 1 agent": counted once per state of a turn.
fn turn_details(state: &ThreadState, work: &TurnWork) -> String {
    if let Some(known) = work.details.borrow().as_ref() {
        return known.clone();
    }
    let (mut tools, mut agents) = (0, 0);
    let mut files = BTreeSet::new();
    for item in state.items_at(&work.items) {
        for e in &item.entries {
            let b = e.borrow();
            if b.data.kind != Kind::Tool {
                continue;
            }
            if item.ty == ItemType::Agent || b.sub.is_some() {
                agents += 1;
                continue;
            }
            tools += 1;
            let name = b.data.tool_name.as_deref().unwrap_or("");
            if EDIT_TOOLS.contains(&name) {
                let input: serde_json::Value = serde_json::from_str(b.data.tool_input.as_deref().unwrap_or("{}")).unwrap_or_default();
                if let Some(path) = input["file_path"].as_str().or(input["notebook_path"].as_str()) {
                    // As git names it, symlinks resolved (`/tmp` is `/private/tmp`), so a file
                    // isn't counted twice.
                    let p = std::path::Path::new(path);
                    let real = p.parent().and_then(|d| std::fs::canonicalize(d).ok()).zip(p.file_name()).map(|(d, n)| d.join(n).to_string_lossy().into_owned());
                    files.insert(real.unwrap_or_else(|| path.to_string()));
                }
            }
        }
    }
    // What git saw change in the turn: shell commands' and scripts' edits too.
    files.extend(work.changed.iter().cloned());
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut parts = vec![];
    if tools > 0 {
        parts.push(plural(tools, "tool call", "tool calls"));
    }
    if !files.is_empty() {
        parts.push(format!("edited {}", plural(files.len(), "file", "files")));
    }
    if agents > 0 {
        parts.push(plural(agents, "agent", "agents"));
    }
    let details = parts.join(" · ");
    *work.details.borrow_mut() = Some(details.clone());
    details
}

/// A finished turn's work folded into one line: "Worked for 2m 14s · 9 tool calls · edited 3 files".
fn turn_summary(state: &ThreadState, work: &TurnWork, open: bool, pos: i64, thread: &Entity<ThreadState>, theme: &Theme) -> AnyElement {
    let worked = match (work.started, work.ended) {
        (Some(start), Some(end)) if end >= start => format!("Worked for {}", yh_core::clock::duration(end - start)),
        _ => "Worked".to_string(),
    };
    let details = turn_details(state, work);
    let handle = thread.clone();
    // Quiet, like a disclosure line: secondary text on the canvas, a fill under the pointer.
    div()
        .id(("turn", pos as u64 as usize))
        .flex()
        .items_center()
        .gap(px(6.))
        .w_full()
        .h(px(28.))
        .px(px(6.))
        .rounded(px(radius::MEDIUM))
        .hover(|s| s.bg(theme.hover))
        .cursor_pointer()
        .text_size(px(ts::FOOTNOTE))
        .text_color(theme.secondary)
        .child(div().flex_none().w(px(14.)).flex().justify_center().child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 13., theme.secondary)))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(worked))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme.tertiary)
                .when(!details.is_empty(), |d| d.child(format!("· {details}"))),
        )
        .on_click(move |_, _, cx| {
            handle.update(cx, |t, cx| {
                t.toggle_turn(pos);
                cx.notify();
            })
        })
        .into_any_element()
}

/// Where sent images are kept.
fn attachments_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| yh_core::library::data_directory().join("attachments"))
}

/// The Copy button that says "Copied" now: its id's hash, and its row.
type Copied = (u64, gpui_kit::WeakEntity<ThreadState>, i64);

thread_local! {
    /// The Copy button clicked last, while it says "Copied" (`COPIED_FOR`). Transient and per
    /// row: only that row is spliced and drawn again, when it starts and when it ends.
    static COPIED: std::cell::RefCell<Option<Copied>> = const { std::cell::RefCell::new(None) };
}

/// How long a Copy button says "Copied".
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1500);

pub fn copy_hash(name: &'static str, key: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (name, key).hash(&mut hasher);
    hasher.finish()
}

/// Whether this Copy button says "Copied" now (it stays shown then, hovered or not).
pub fn just_copied(name: &'static str, key: impl Hash) -> bool {
    let h = copy_hash(name, key);
    COPIED.with(|c| c.borrow().as_ref().is_some_and(|(x, _, _)| *x == h))
}

/// A small Copy button in a row: what it copies goes on the clipboard as plain text, and it
/// says "Copied" with a check for a moment. No animation; the row's height never changes.
fn copy_button(name: &'static str, key: impl Hash, text: String, ctx: &RowContext, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    let h = copy_hash(name, key);
    let copied = COPIED.with(|c| c.borrow().as_ref().is_some_and(|(x, _, _)| *x == h));
    let (thread, pos) = (ctx.thread.clone(), ctx.pos);
    div()
        .id(ElementId::NamedInteger(name.into(), h))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(space::XS))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(radius::SMALL))
        .font_family(crate::theme::UI)
        .text_size(px(ts::CAPTION))
        .text_color(if copied { theme.text } else { theme.secondary })
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover).text_color(theme.text))
        .child(icon(if copied { IconName::Check } else { IconName::Copy }, 12., theme.secondary))
        .child(if copied { "Copied" } else { "Copy" })
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            mark_copied(h, &thread, pos, cx);
        })
}

/// Splices one row so it's drawn again (its height can't change: only a label does).
fn redraw_row(thread: &gpui_kit::WeakEntity<ThreadState>, pos: i64, cx: &mut App) {
    let _ = thread.update(cx, |t, cx| {
        t.touch_pos(pos);
        cx.notify();
    });
}

/// Shows "Copied" on one button (`h`, a `copy_hash`) and takes it back `COPIED_FOR` later,
/// unless another was clicked meanwhile. Only the rows involved are spliced, never the list.
pub fn mark_copied(h: u64, thread: &Entity<ThreadState>, pos: i64, cx: &mut App) {
    let weak = thread.downgrade();
    // The one shown before goes back now.
    if let Some((_, old, old_pos)) = COPIED.with(|c| c.replace(Some((h, weak.clone(), pos)))) {
        redraw_row(&old, old_pos, cx);
    }
    redraw_row(&weak, pos, cx);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(COPIED_FOR).await;
        let still = COPIED.with(|c| {
            let mut c = c.borrow_mut();
            let still = c.as_ref().is_some_and(|(x, _, _)| *x == h);
            if still {
                *c = None;
            }
            still
        });
        if still {
            let _ = cx.update(|cx| redraw_row(&weak, pos, cx));
        }
    })
    .detach();
}

/// What right-clicking a picture, a video or a file offers: Open Large (Quick Look) for media,
/// its own app, Finder, its path.
fn media_menu(menu: PopupMenu, path: &str, all: &[String], kind: Option<MediaKind>) -> PopupMenu {
    let (p1, p2, p3) = (path.to_string(), path.to_string(), path.to_string());
    let mut menu = menu;
    if let Some(kind) = kind {
        let at = all.iter().position(|m| m == path).unwrap_or(0);
        let all = if all.is_empty() { vec![path.to_string()] } else { all.to_vec() };
        menu = menu
            .item(PopupMenuItem::new("Open Large").on_click(move |_, _, _| crate::platform::quick_look(all.clone(), at)))
            .item(PopupMenuItem::new(if kind == MediaKind::Image { "Open in Preview" } else { "Open in QuickTime Player" }).on_click(move |_, _, _| {
                let _ = std::process::Command::new("open").arg(&p1).spawn();
            }));
    } else {
        menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, _, _| {
            let _ = std::process::Command::new("open").arg(&p1).spawn();
        }));
    }
    menu.item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&p2))))
        .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(p3.clone()))))
}

/// A sent file or folder: its icon, its name.
fn file_chip(name: &str, theme: &Theme) -> gpui_kit::Div {
    let folder = name.ends_with('/');
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(40.))
        .px(px(10.))
        .rounded(px(radius::MEDIUM))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .text_size(px(ts::FOOTNOTE))
        .child(icon(if folder { IconName::Folder } else { IconName::File }, 16., theme.secondary))
        .child(div().max_w(px(220.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(yh_core::library::last_component(name.trim_end_matches('/')).to_string()))
}

/// `/review fix it`: the length of the command's name, when the message starts with one.
fn command_name(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('/')?;
    let len = rest.find(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | ':'))).unwrap_or(rest.len());
    (len > 0 && rest[len..].chars().next().is_none_or(char::is_whitespace)).then_some(len + 1)
}

/// Your message's text: a command's name in bold monospace, `@mentions` and the thread's file
/// paths as links into the Files tab.
fn user_text(text: &str, ctx: &RowContext, theme: &Theme, window: &Window) -> AnyElement {
    let mut styled = Text { text: text.to_string(), spans: vec![], links: vec![] };
    if let Some(linker) = ctx.linker.as_deref() {
        linker.link(&mut styled);
    }
    let command = command_name(text);
    if command.is_none() && styled.links.is_empty() {
        return selectable(text.to_string(), theme);
    }
    // Runs: the window's text style, the command in Menlo semibold, links in the accent.
    let base = window.text_style();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    if let Some(end) = command {
        cuts.push(end);
    }
    for (r, _) in &styled.links {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    let marks = find_marks(text);
    for (r, _) in &marks {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let runs: Vec<gpui_kit::TextRun> = cuts
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| {
            let mut run = base.to_run(w[1] - w[0]);
            if command.is_some_and(|end| w[0] < end) {
                run.font = gpui_kit::Font { family: MONO.into(), weight: FontWeight::SEMIBOLD, ..run.font };
            }
            if styled.links.iter().any(|(r, _)| r.start <= w[0] && w[1] <= r.end) {
                run.color = theme.accent;
            }
            if let Some((_, mark)) = marks.iter().find(|(r, _)| r.start <= w[0] && w[1] <= r.end) {
                run.background_color = mark.background_color;
            }
            run
        })
        .collect();
    let shaped = StyledText::new(SharedString::from(text.to_string())).with_runs(runs);
    if styled.links.is_empty() {
        return shaped.into_any_element();
    }
    let ranges: Vec<std::ops::Range<usize>> = styled.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = styled.links.iter().map(|(_, u)| u.clone()).collect();
    let target = (ctx.thread_id.clone(), ctx.project_id.clone());
    gpui_kit::InteractiveText::new(id_of("user-text", (ctx.pos, text)), shaped)
        .on_click(ranges, move |ix, window, cx| {
            if let Some(url) = urls.get(ix) {
                follow(url, Some(&target), window, cx);
            }
        })
        .into_any_element()
}

fn user_row(item: &Item, ctx: &RowContext, theme: &Theme, window: &Window) -> AnyElement {
    let e = item.entries[0].borrow();
    let text = e.data.text.clone();
    let images = e.data.attachments.clone();
    let date = e.date;
    // Sent while Claude worked: how it got in, under the bubble (queued ones wait above the
    // composer, not here).
    let note = match e.delivery.as_deref() {
        Some("steered") => Some((crate::ui::icons::corner_down_right(), "Steered · Claude read this after its step")),
        Some("interrupted") => Some((crate::ui::icons::zap(), "Interrupted Claude to send this")),
        _ => None,
    };

    drop(e);
    // Pictures and videos open in Quick Look, the arrow keys going through the message's others.
    // An image in a folder of its own couldn't go inline: it's a file.
    let kind_of = |n: &str| media_kind(n).filter(|k| !n.ends_with('/') && !(*k == MediaKind::Image && n.contains('/')));
    let media: Vec<String> = images.iter().filter(|n| kind_of(n).is_some()).map(|n| attachments_dir().join(n).to_string_lossy().into_owned()).collect();
    let pos = item.pos;
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.))
        .pt(px(space::S))
        .pb(px(space::XS))
        .group("user-message")
        .when(!images.is_empty(), |d| {
            d.child(div().flex().flex_wrap().justify_end().gap(px(6.)).mb(px(2.)).children(images.iter().enumerate().map(|(n, name)| {
                let path = attachments_dir().join(name).to_string_lossy().into_owned();
                let kind = kind_of(name);
                let index = media.iter().position(|m| *m == path);
                let all = media.clone();
                let menu_all = media.clone();
                let menu_path = path.clone();
                let target = path.clone();
                let tile = div()
                    .id(("user-image", ((pos as u64) << 8 | n as u64) as usize))
                    .cursor_pointer()
                    .on_click(move |_, _, _| match index {
                        Some(at) => crate::platform::quick_look(all.clone(), at),
                        None => {
                            let _ = std::process::Command::new("open").arg(&target).spawn();
                        }
                    });
                let tile = match kind {
                    Some(MediaKind::Image) => tile.child(gpui_kit::img(std::path::PathBuf::from(&path)).max_w(px(240.)).max_h(px(180.)).rounded(px(radius::LARGE))),
                    Some(MediaKind::Video) => tile
                        .w(px(220.))
                        .h(px(124.))
                        .rounded(px(radius::LARGE))
                        .bg(gpui_kit::black().opacity(0.82))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Play, 28., gpui_kit::white().opacity(0.9))),
                    None => tile.child(file_chip(name, theme)),
                };
                tile.context_menu(move |menu, _, _| media_menu(menu, &menu_path, &menu_all, kind))
            })))
        })
        .when(!text.is_empty(), |d| {
            d.child(
                div()
                    .max_w(gpui_kit::relative(0.8))
                    .bg(theme.bubble)
                    .px(px(14.))
                    .py(px(space::S))
                    .rounded(px(radius::LARGE))
                    .child(user_text(&text, ctx, theme, window)),
            )
        })
        .when_some(note, |d, (mark, n)| {
            d.child(div().flex().items_center().gap(px(space::XS)).text_size(px(ts::CAPTION)).text_color(theme.secondary).child(mark.size(px(11.)).text_color(theme.secondary)).child(n))
        })
        // Copy and when it was sent, small under it. Copy's room is always kept: hovering
        // mustn't change the row's height.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::XS))
                .h(px(20.))
                .when(!text.is_empty(), |d| {
                    let shown = just_copied("copy-user", (pos, &text));
                    d.child(copy_button("copy-user", (pos, &text), text.clone(), ctx, theme).when(!shown, |b| b.opacity(0.).group_hover("user-message", |s| s.opacity(1.))))
                })
                .when_some(date, |d, date| {
                    let full = yh_core::clock::full_time(date);
                    d.child(
                        div()
                            .id(("user-time", pos as u64 as usize))
                            .text_size(px(ts::CAPTION))
                            .text_color(theme.tertiary)
                            .child(yh_core::clock::message_time(date, yh_core::library::now_ms() / 1000.0))
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)),
                    )
                }),
        )
        .into_any_element()
}

fn markdown_of(e: &EntryRef, linker: Option<&Linker>) -> Rc<markdown::Markdown> {
    let revision = linker.map_or(0, |l| l.revision);
    let mut b = e.borrow_mut();
    if let Some((version, links, md)) = &b.markdown {
        if *version == b.version && *links == revision {
            return md.clone();
        }
    }
    let mut md = markdown::parse(&b.data.text);
    if let Some(linker) = linker {
        linker.link_markdown(&mut md);
    }
    let md = Rc::new(md);
    b.markdown = Some((b.version, revision, md.clone()));
    md
}

/// Claude's reply, with Copy under it while the pointer is over it (its room always kept).
fn text_row(e: &EntryRef, ctx: &RowContext, theme: &Theme, window: &Window) -> AnyElement {
    let md = markdown_of(e, ctx.linker.as_deref());
    let b = e.borrow();
    let (live, text) = (b.live, b.data.text.clone());
    drop(b);
    div()
        .flex()
        .flex_col()
        .group("reply")
        .child(markdown_view_in(&md, theme, Some(ctx), Some(window)))
        .child(div().h(px(20.)).mt(px(space::XS)).ml(px(-6.)).flex().when(!live, |d| {
            let shown = just_copied("copy-reply", (ctx.pos, &text));
            d.child(copy_button("copy-reply", (ctx.pos, &text), text.clone(), ctx, theme).when(!shown, |b| b.opacity(0.).group_hover("reply", |s| s.opacity(1.))))
        }))
        .into_any_element()
}

/// Images and videos Claude showed you (`show_media`), from the thread's own copies.
fn media_row(e: &EntryRef, ctx: &RowContext, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    let input: serde_json::Value = serde_json::from_str(b.data.tool_input.as_deref().unwrap_or("{}")).unwrap_or_default();
    let paths = media_paths(&input);
    let caption = input["caption"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
    let (done, failed, result) = (b.data.tool_result.is_some(), b.data.is_error, b.data.tool_result.clone().unwrap_or_default());
    let tool_use = b.data.id.clone();
    drop(b);
    let kinds: Vec<MediaKind> = paths.iter().filter_map(|p| media_kind(p)).collect();
    let videos = kinds.iter().filter(|k| **k == MediaKind::Video).count();
    let images = kinds.len() - videos;
    let mut said = vec![];
    if images > 0 {
        said.push(if images == 1 { "an image".to_string() } else { format!("{images} images") });
    }
    if videos > 0 {
        said.push(if videos == 1 { "a video".to_string() } else { format!("{videos} videos") });
    }
    let summary = format!("Claude showed {}", if said.is_empty() { "media".to_string() } else { said.join(" and ") });
    let items: Vec<(String, String, MediaKind)> = paths
        .iter()
        .enumerate()
        .filter_map(|(n, p)| {
            let stored = attachments_dir().join(media_name(&ctx.thread_id, &tool_use, n, p));
            Some((stored.to_string_lossy().into_owned(), p.clone(), media_kind(p)?))
        })
        .filter(|(stored, _, _)| done && std::path::Path::new(stored).exists())
        .collect();
    let all: Vec<String> = items.iter().map(|(s, _, _)| s.clone()).collect();
    let single = items.len() == 1;
    let mut column = div().flex().flex_col().gap(px(space::S)).py(px(space::XS)).child(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(ts::FOOTNOTE))
            .child(icon(IconName::GalleryVerticalEnd, 14., theme.secondary))
            .child(div().when(caption.is_none(), |d| d.text_color(theme.secondary)).child(caption.unwrap_or(summary)))
            .when(!done, |d| d.child(div().text_color(theme.secondary).child("…"))),
    );
    if failed {
        column = column.child(div().text_size(px(ts::FOOTNOTE)).text_color(theme.error).child(result));
    } else {
        column = column.child(div().flex().flex_wrap().gap(px(10.)).children(items.into_iter().enumerate().map(|(n, (stored, original, kind))| {
            let (menu_all, menu_path) = (all.clone(), stored.clone());
            let all = all.clone();
            let (max_w, max_h) = if single { (640., 480.) } else { (320., 240.) };
            let tile = div()
                .id(id_of("media", (&stored, n)))
                .cursor_pointer()
                .rounded(px(radius::LARGE))
                .overflow_hidden()
                .border_1()
                .border_color(theme.separator)
                .on_click(move |_, _, _| crate::platform::quick_look(all.clone(), n));
            match kind {
                MediaKind::Image => tile.child(gpui_kit::img(std::path::PathBuf::from(&stored)).max_w(px(max_w)).max_h(px(max_h))),
                // A poster of its own: a play mark and its name; Quick Look plays it.
                MediaKind::Video => tile
                    .w(px(max_w.min(360.)))
                    .h(px(max_h.min(200.)))
                    .bg(gpui_kit::black().opacity(0.82))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .child(icon(IconName::Play, 30., gpui_kit::white().opacity(0.9)))
                    .child(div().text_size(px(ts::CAPTION)).text_color(gpui_kit::white().opacity(0.7)).child(yh_core::library::last_component(&original))),
            }
            .context_menu(move |menu, _, _| media_menu(menu, &menu_path, &menu_all, Some(kind)))
        })));
    }
    column.into_any_element()
}

fn notice_row(e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    if b.data.kind == Kind::Command {
        drop(b);
        let md = markdown_of(e, None);
        return div()
            .bg(theme.card)
            .border_1()
            .border_color(theme.separator)
            .rounded(px(radius::LARGE))
            .px(px(space::M))
            .py(px(10.))
            .text_size(px(ts::FOOTNOTE))
            .child(markdown_view_in(&md, theme, None, None))
            .into_any_element();
    }
    let error = b.data.is_error;
    // A line between turns, centred: what happened (compacted, model changed, an error).
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(space::M))
        .py(px(2.))
        .text_size(px(ts::FOOTNOTE))
        .text_color(if error { theme.error } else { theme.secondary })
        .child(div().flex_1().h(px(1.)).bg(theme.separator))
        .child(
            div()
                .flex_none()
                .max_w(gpui_kit::relative(0.8))
                .flex()
                .items_start()
                .gap(px(6.))
                .when(error, |d| d.child(div().pt(px(3.)).child(icon(IconName::CircleAlert, 13., theme.error))))
                .child(div().min_w_0().child(selectable(b.data.text.clone(), theme))),
        )
        .child(div().flex_1().h(px(1.)).bg(theme.separator))
        .into_any_element()
}

/// A sub-agent's state as an icon: working, done, failed, stopped.
fn status_icon(status: &str, theme: &Theme) -> Icon {
    match status {
        "running" => icon(IconName::LoaderCircle, 15., theme.accent),
        "completed" => icon(IconName::CircleCheck, 15., theme.success),
        "failed" => icon(IconName::CircleX, 15., theme.error),
        _ => icon(IconName::Square, 13., theme.secondary),
    }
}

/// "Reading a.txt · 3 steps" while it works; "3 steps" or "Failed · 3 steps" after.
fn subagent_stats(status: &str, steps: usize, activity: Option<String>) -> String {
    let mut parts = vec![];
    match status {
        "running" => parts.push(activity.unwrap_or_else(|| "Working".into())),
        "failed" => parts.push("Failed".into()),
        "stopped" | "killed" => parts.push("Stopped".into()),
        _ => {}
    }
    if steps > 0 {
        parts.push(if steps == 1 { "1 step".into() } else { format!("{steps} steps") });
    }
    if parts.is_empty() {
        "Done".into()
    } else {
        parts.join(" · ")
    }
}

/// A sub-agent's call: what it's doing (its last few steps while it works), then its report.
fn agent_row(thread: &Entity<ThreadState>, item: &Item, theme: &Theme) -> AnyElement {
    let e = item.entries[0].borrow();
    let input: serde_json::Value = serde_json::from_str(e.data.tool_input.as_deref().unwrap_or("{}")).unwrap_or_default();
    let title = e
        .sub
        .as_ref()
        .map(|s| s.description.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| input["description"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Sub-agent".into());
    let kind = input["subagent_type"].as_str().unwrap_or("").to_string();
    let status = e.sub.as_ref().map(|s| s.status.clone()).unwrap_or_else(|| match &e.data.tool_result {
        Some(_) if e.data.is_error => "failed".into(),
        Some(_) => "completed".into(),
        None => "running".into(),
    });
    let steps = e.sub.as_ref().map_or(0, |s| s.steps);
    let running = status == "running";
    let failed = status == "failed" || e.data.is_error;
    let report = e.sub.as_ref().and_then(|s| s.report.clone()).or_else(|| e.data.tool_result.as_deref().map(crate::ui::steps::subagent_report)).unwrap_or_default();
    // Its last three tool calls while it works, so it visibly moves.
    let recent: Vec<(steps::Presentation, bool, bool)> = if running {
        e.sub
            .as_ref()
            .map(|s| {
                let tools: Vec<_> = s.log.iter().filter(|x| x.tool.is_some()).collect();
                tools[tools.len().saturating_sub(3)..]
                    .iter()
                    .map(|x| {
                        let input: serde_json::Value = serde_json::from_str(&x.text).unwrap_or_default();
                        (steps::present(x.tool.as_deref().unwrap_or(""), &input, x.result.as_deref()), x.result.is_none(), x.is_error)
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };
    let activity = recent.last().map(|(p, _, _)| if p.detail.is_empty() { p.verb.clone() } else { format!("{} {}", p.verb, p.detail) });
    let followable = e.sub.is_some();
    let tool_id = e.data.id.clone();
    drop(e);
    let (pos, open) = (item.pos, item.open || expand_all());
    let handle = thread.clone();
    let follow_thread = thread.clone();
    let mut card = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(if running {
            theme.accent.opacity(0.45)
        } else if failed {
            theme.error.opacity(0.45)
        } else {
            theme.separator
        })
        .rounded(px(radius::LARGE))
        .bg(theme.card)
        .overflow_hidden()
        .child(
            div()
                .id(("agent", pos as u64 as usize))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(space::M))
                .py(px(space::S))
                .cursor_pointer()
                .hover(|s| s.bg(theme.hover))
                .child(div().w(px(18.)).flex().justify_center().child(status_icon(&status, theme)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::SEMIBOLD).child(title))
                                .when(!kind.is_empty(), |d| {
                                    d.child(div().flex_none().text_size(px(ts::CAPTION)).line_height(px(16.)).px(px(6.)).rounded(px(radius::SMALL)).bg(theme.selected).text_color(theme.secondary).child(kind))
                                }),
                        )
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(ts::FOOTNOTE)).text_color(theme.secondary).child(subagent_stats(&status, steps, activity))),
                )
                .when(followable, |d| {
                    d.child(
                        div()
                            .id(("agent-follow", pos as u64 as usize))
                            .flex_none()
                            .p(px(space::XS))
                            .rounded(px(radius::SMALL))
                            .hover(|s| s.bg(theme.selected))
                            .child(icon(IconName::PanelRight, 14., theme.secondary))
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Follow in Sidebar").build(window, cx))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let id = tool_id.clone();
                                follow_thread.update(cx, |t, cx| {
                                    t.inspected_subagent = Some(id);
                                    t.subagents_version += 1;
                                    cx.notify();
                                });
                                show_panel(crate::state::workspace::Panel::Subagents, cx);
                            }),
                    )
                })
                .when(!report.is_empty(), |d| d.child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 13., theme.tertiary)))
                .on_click(move |_, _, cx| {
                    handle.update(cx, |t, cx| {
                        t.toggle_item(pos);
                        cx.notify();
                    })
                }),
        );
    if !recent.is_empty() {
        let mut list = div().flex().flex_col().gap(px(6.)).px(px(space::M)).py(px(space::S)).border_t_1().border_color(theme.separator).bg(theme.canvas).text_size(px(ts::FOOTNOTE));
        for (p, live, failed) in recent {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().w(px(18.)).flex().justify_center().child(icon(p.icon, 13., theme.secondary)))
                    .child(div().flex_none().text_color(theme.text).child(p.verb))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(ts::CAPTION)).text_color(theme.secondary).child(p.detail))
                    .when(live, |d| d.child(icon(IconName::LoaderCircle, 12., theme.accent)))
                    .when(failed, |d| d.child(icon(IconName::CircleX, 12., theme.error))),
            );
        }
        card = card.child(list);
    }
    if open && !report.is_empty() {
        let md = markdown::parse(&report);
        card = card.child(div().px(px(space::M)).py(px(space::M)).border_t_1().border_color(theme.separator).bg(theme.canvas).child(markdown_view_in(&md, theme, None, None)));
    }
    card.into_any_element()
}

/// An edit's change as diff lines: what went, then what came (Edit, MultiEdit, Write).
fn diff_of(e: &crate::state::thread::Entry) -> Option<Vec<(char, String)>> {
    let name = e.data.tool_name.as_deref()?;
    if !matches!(name, "Edit" | "MultiEdit" | "Write") {
        return None;
    }
    let input: serde_json::Value = serde_json::from_str(e.data.tool_input.as_deref()?).ok()?;
    let edits: Vec<(Option<String>, Option<String>)> = match name {
        "MultiEdit" => input["edits"]
            .as_array()?
            .iter()
            .map(|x| (x["old_string"].as_str().map(str::to_string), x["new_string"].as_str().map(str::to_string)))
            .collect(),
        "Write" => vec![(None, input["content"].as_str().map(str::to_string))],
        _ => vec![(input["old_string"].as_str().map(str::to_string), input["new_string"].as_str().map(str::to_string))],
    };
    let mut lines = vec![];
    if let Some(path) = input["file_path"].as_str() {
        lines.push(('f', path.to_string()));
    }
    for (i, (old, new)) in edits.into_iter().enumerate() {
        if i > 0 {
            lines.push((' ', "⋯".into()));
        }
        for l in old.unwrap_or_default().lines() {
            lines.push(('-', l.to_string()));
        }
        for l in new.unwrap_or_default().lines() {
            lines.push(('+', l.to_string()));
        }
    }
    if lines.len() > 400 {
        let more = lines.len() - 400;
        lines.truncate(400);
        lines.push((' ', format!("⋯ {more} more lines")));
    }
    Some(lines)
}

/// A change as a unified diff: its file as a header, then each line with its sign in a gutter,
/// removed lines on red, added on green.
fn diff_view(lines: Vec<(char, String)>, theme: &Theme) -> impl IntoElement {
    // Long changes scroll inside the box past 320 pt, as tool output does.
    let key = copy_hash("diff-box", (lines.len(), lines.iter().take(3).map(|(_, t)| t.clone()).collect::<Vec<_>>()));
    let mut column = div()
        .font_family(MONO)
        .text_size(px(ts::CAPTION))
        .line_height(px(16.))
        .bg(theme.code)
        .border_1()
        .border_color(theme.separator)
        .rounded(px(radius::MEDIUM))
        .overflow_hidden();
    let mut body = div().py(px(space::XS));
    for (kind, text) in lines {
        let line = |sign: &'static str, sign_color: Hsla, bg: Option<Hsla>| {
            div()
                .flex()
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().flex_none().w(px(22.)).pl(px(space::S)).text_color(sign_color).child(sign))
                .child(div().flex_1().min_w_0().pr(px(space::S)).whitespace_normal().text_color(theme.text).child(text.clone()))
        };
        match kind {
            'f' => {
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(26.))
                        .px(px(space::S))
                        .border_b_1()
                        .border_color(theme.separator)
                        .font_family(crate::theme::UI)
                        .text_color(theme.secondary)
                        .child(icon(IconName::File, 12., theme.secondary))
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text)),
                )
            }
            '-' => body = body.child(line("-", theme.error, Some(theme.error.opacity(0.10)))),
            '+' => body = body.child(line("+", theme.success, Some(theme.success.opacity(0.12)))),
            _ => body = body.child(div().pl(px(22.)).text_color(theme.tertiary).child(text)),
        }
    }
    column.child(capped_box(key, 320., body))
}

fn highlight_style(span: Span, theme: &Theme) -> HighlightStyle {
    match span {
        Span::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
        Span::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
        // Inline code: SF Mono (`SelText::rich`) on a quiet fill, in the text's own colour.
        Span::Code => HighlightStyle { background_color: Some(theme.selected), ..Default::default() },
        Span::Link => HighlightStyle { color: Some(theme.accent), ..Default::default() },
        Span::CodeLink => HighlightStyle { background_color: Some(theme.accent_soft), color: Some(theme.accent), ..Default::default() },
        Span::Strike => HighlightStyle {
            strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: None }),
            ..Default::default()
        },
        Span::Keyword => HighlightStyle { color: Some(theme.keyword), ..Default::default() },
        Span::Str => HighlightStyle { color: Some(theme.string), ..Default::default() },
        Span::Number => HighlightStyle { color: Some(theme.number), ..Default::default() },
        Span::Comment => HighlightStyle { color: Some(theme.comment), font_style: Some(FontStyle::Italic), ..Default::default() },
    }
}

/// `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]`, `*.localhost`: the app Claude is running.
fn is_local(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) else { return false };
    let host = if rest.starts_with('[') { rest.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { rest.split([':', '/', '?', '#']).next().unwrap_or("").to_string() };
    let host = host.to_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]") || host.ends_with(".localhost")
}

/// Follows a link from a reply: a local web address in the thread's own browser, any other
/// in the default browser; a file in the Files tab, or with ⌘ in its default app.
fn follow(url: &str, ctx: Option<&(SharedString, SharedString)>, window: &Window, cx: &mut App) {
    let is_web = url.contains("://") || url.starts_with("mailto:");
    if is_web {
        if let (Some((thread, project)), true) = (ctx, is_local(url)) {
            crate::browser::Browsers::open_page(thread, Some(project), url, cx);
            return;
        }
    } else if window.modifiers().platform {
        let (path, _) = crate::ui::inspector::split_lines(url);
        let _ = std::process::Command::new("open").arg(path).spawn();
        return;
    }
    crate::ui::inspector::open_link(url, cx);
}

thread_local! {
    /// The next selectable text's place in reading order, counted from its row's (`render_item`).
    static READING_ORDER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Where a row's selectable texts start in reading order: rows in turn, then within them.
fn start_reading_order(pos: i64) {
    READING_ORDER.with(|o| o.set(((pos + (1 << 40)) as u64) << 16));
}

/// The next place in reading order, and an id for the text there: stable while the row's
/// blocks are, so a selection holds as the row is drawn again.
fn next_reading_order(text: &str) -> (u64, ElementId) {
    let order = READING_ORDER.with(|o| {
        let n = o.get();
        o.set(n + 1);
        n
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.len().hash(&mut hasher);
    order.hash(&mut hasher);
    (order, ElementId::NamedInteger("sel".into(), hasher.finish()))
}

thread_local! {
    /// What Find looks for while it's open, and whether the row being drawn is the one it's on
    /// (set per row by `render_item`). None: no marks, nothing looked for.
    static FIND: std::cell::RefCell<Option<(String, bool)>> = const { std::cell::RefCell::new(None) };
}

/// Find's marks in `text`: every match, orange in the row Find is on, yellow in the others.
fn find_marks(text: &str) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    FIND.with(|f| {
        let f = f.borrow();
        let Some((query, current)) = f.as_ref() else { return vec![] };
        let color = if *current { gpui_kit::hsla(30. / 360., 1., 0.5, 0.6) } else { gpui_kit::hsla(52. / 360., 1., 0.5, 0.35) };
        yh_core::find::ranges(text, query).into_iter().map(|r| (r, HighlightStyle { background_color: Some(color), ..Default::default() })).collect()
    })
}

/// Plain text you can select (a tool's output, a notice).
pub fn selectable(text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    let text: SharedString = text.into();
    let (order, id) = next_reading_order(&text);
    let marks = find_marks(&text);
    let element = SelText::new(id, text.clone(), StyledText::new(text), order, theme.accent.opacity(0.28));
    if marks.is_empty() { element.into_any_element() } else { element.rich(marks, vec![]).into_any_element() }
}

fn styled_text(styled: &Text, theme: &Theme, ctx: Option<&RowContext>) -> AnyElement {
    let text: SharedString = styled.text.clone().into();
    let (order, id) = next_reading_order(&styled.text);
    let mut element = SelText::new(id, text.clone(), StyledText::new(text), order, theme.accent.opacity(0.28));
    let marks = find_marks(&styled.text);
    if !styled.spans.is_empty() || !marks.is_empty() {
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = styled.spans.iter().map(|(r, s)| (r.clone(), highlight_style(*s, theme))).collect();
        highlights.extend(marks);
        highlights.sort_by_key(|(r, _)| r.start);
        let mono = styled.spans.iter().filter(|(_, s)| matches!(s, Span::Code | Span::CodeLink)).map(|(r, _)| r.clone()).collect();
        element = element.rich(highlights, mono);
    }
    if styled.links.is_empty() {
        return element.into_any_element();
    }
    // Web links open in the browser, file links in the Files tab.
    let ranges: Vec<std::ops::Range<usize>> = styled.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = styled.links.iter().map(|(_, u)| u.clone()).collect();
    let target = ctx.map(|c| (c.thread_id.clone(), c.project_id.clone()));
    element
        .links(ranges, move |ix, window, cx| {
            if let Some(url) = urls.get(ix) {
                follow(url, target.as_ref(), window, cx);
            }
        })
        .into_any_element()
}

/// Markdown outside the transcript (the plan card): its texts numbered from a place of their
/// own, so their ids hold from one drawing to the next.
pub fn markdown_view(md: &markdown::Markdown, theme: &Theme) -> impl IntoElement {
    let kept = READING_ORDER.with(|o| o.replace(1 << 62));
    let view = markdown_view_in(md, theme, None, None);
    READING_ORDER.with(|o| o.set(kept));
    view
}

/// A fence's language as a reader would name it: `py` → Python. Empty: none shown.
fn language_name(lang: &str) -> String {
    let lang = lang.split_whitespace().next().unwrap_or("");
    let known = match lang.to_ascii_lowercase().as_str() {
        "" => "",
        "rs" | "rust" => "Rust",
        "py" | "python" | "python3" => "Python",
        "ts" | "typescript" => "TypeScript",
        "tsx" => "TSX",
        "js" | "javascript" => "JavaScript",
        "jsx" => "JSX",
        "sh" | "bash" | "zsh" | "shell" | "console" => "Shell",
        "swift" => "Swift",
        "go" | "golang" => "Go",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "md" | "markdown" => "Markdown",
        "diff" | "patch" => "Diff",
        "c" => "C",
        "cpp" | "cxx" => "C++",
        "java" => "Java",
        "kotlin" | "kt" => "Kotlin",
        "ruby" | "rb" => "Ruby",
        "mermaid" => "Mermaid",
        "text" | "txt" | "plain" => "Text",
        _ => return lang.to_string(),
    };
    known.to_string()
}

/// A code block: a header with its language and Copy, then the code in SF Mono.
fn code_block(code: &Text, lang: &str, theme: &Theme, ctx: Option<&RowContext>, n: usize) -> AnyElement {
    let name = language_name(lang);
    let has_header = ctx.is_some() || !name.is_empty();
    div()
        .flex()
        .flex_col()
        .bg(theme.code)
        .border_1()
        .border_color(theme.separator)
        .rounded(px(radius::MEDIUM))
        .overflow_hidden()
        .when(has_header, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(30.))
                    .pl(px(space::M))
                    .pr(px(space::XS))
                    .border_b_1()
                    .border_color(theme.separator)
                    .text_size(px(ts::CAPTION))
                    .text_color(theme.secondary)
                    .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).child(name))
                    .when_some(ctx, |d, ctx| d.child(copy_button("copy-code", (ctx.pos, n, &code.text), code.text.clone(), ctx, theme))),
            )
        })
        // Long lines scroll sideways instead of wrapping, as in the Swift app; a vertical wheel
        // over it still scrolls the transcript (the box takes only the horizontal axis).
        .child(
            div()
                .id(id_of("code-scroll", (n, code.text.len(), ctx.map(|c| c.pos))))
                .w_full()
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .child(
                    div()
                        .flex_none()
                        .px(px(space::M))
                        .py(px(10.))
                        .whitespace_nowrap()
                        .font_family(MONO)
                        .text_size(px(ts::CODE))
                        .line_height(px(ts::CODE_LINE + 2.))
                        .child(styled_text(code, theme, None)),
                ),
        )
        .into_any_element()
}

/// Markdown blocks; in a row (`ctx`), with links that know the thread, Copy on code and
/// tables, and Mermaid diagrams drawn.
pub fn markdown_view_in(md: &markdown::Markdown, theme: &Theme, ctx: Option<&RowContext>, window: Option<&Window>) -> impl IntoElement {
    let mut column = div().flex().flex_col().gap(px(10.)).w_full();
    for (n, block) in md.blocks.iter().enumerate() {
        let element: AnyElement = match block {
            MdBlock::Paragraph(s) => div().child(styled_text(s, theme, ctx)).into_any_element(),
            // Sizes from the type scale; the space above sets a heading apart from what's before.
            MdBlock::Heading(level, s) => div()
                .when(n > 0, |d| d.pt(px(if *level <= 2 { space::S } else { space::XS })))
                .text_size(px(match level {
                    1 | 2 => ts::TITLE,
                    _ => ts::HEADLINE,
                }))
                .line_height(px(if *level <= 2 { 22. } else { ts::BODY_LINE }))
                .font_weight(if *level == 1 { FontWeight::BOLD } else { FontWeight::SEMIBOLD })
                .when(*level >= 4, |d| d.text_color(theme.secondary))
                .child(styled_text(s, theme, ctx))
                .into_any_element(),
            MdBlock::Code { lang, code } if lang.eq_ignore_ascii_case("mermaid") => match (ctx, window) {
                (Some(ctx), Some(window)) => crate::ui::diagram::diagram_block(&code.text, ctx, n, theme, window)
                    .unwrap_or_else(|| code_block(code, lang, theme, Some(ctx), n)),
                _ => code_block(code, lang, theme, ctx, n),
            },
            MdBlock::Code { lang, code } => code_block(code, lang, theme, ctx, n),
            MdBlock::List(items) => {
                let mut list = div().flex().flex_col().gap(px(space::XS));
                for (depth, number, s) in items {
                    let marker = match number {
                        Some(n) => format!("{n}."),
                        None => ["•", "◦", "▪"][depth % 3].to_string(),
                    };
                    list = list.child(
                        div()
                            .flex()
                            .pl(px(2. + 20. * *depth as f32))
                            .child(
                                div()
                                    .w(px(20.))
                                    .pr(px(6.))
                                    .flex_none()
                                    .flex()
                                    .justify_end()
                                    .text_color(theme.secondary)
                                    .child(marker),
                            )
                            .child(div().flex_1().min_w_0().child(styled_text(s, theme, ctx))),
                    );
                }
                list.into_any_element()
            }
            MdBlock::Quote(s) => div()
                .border_l(px(3.))
                .border_color(theme.separator)
                .pl(px(space::M))
                .py(px(2.))
                .text_color(theme.secondary)
                .child(styled_text(s, theme, ctx))
                .into_any_element(),
            MdBlock::Table { header, rows } => {
                let columns = header.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
                let cell = |s: Option<&Text>| {
                    let mut c = div().flex_1().min_w_0().px(px(space::M)).py(px(6.));
                    if let Some(s) = s {
                        c = c.child(styled_text(s, theme, ctx));
                    }
                    c
                };
                // Rules between rows only, the header on a quiet fill: a Mac list, not a grid.
                let mut table = div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(theme.separator)
                    .rounded(px(radius::MEDIUM))
                    .overflow_hidden()
                    .child(div().flex().bg(theme.card).font_weight(FontWeight::SEMIBOLD).children((0..columns).map(|i| cell(header.get(i)))));
                for row in rows {
                    table = table.child(div().flex().border_t_1().border_color(theme.separator).children((0..columns).map(|i| cell(row.get(i)))));
                }
                // Copy as tab-separated values, over the header's corner while the pointer is
                // over the table: no room of its own, so nothing moves.
                match ctx {
                    Some(ctx) => {
                        let tsv = markdown::table_tsv(header, rows);
                        div()
                            .relative()
                            .group("table")
                            .child(table)
                            .child(
                                div().absolute().top(px(5.)).right(px(5.)).child(
                                    copy_button("copy-table", (ctx.pos, n, &tsv), tsv.clone(), ctx, theme)
                                        .bg(theme.card)
                                        .when(!just_copied("copy-table", (ctx.pos, n, &tsv)), |b| b.opacity(0.).group_hover("table", |s| s.opacity(1.))),
                                ),
                            )
                            .into_any_element()
                    }
                    None => table.into_any_element(),
                }
            }
            MdBlock::Rule => div().my(px(space::XS)).h(px(1.)).bg(theme.separator).into_any_element(),
        };
        column = column.child(element);
    }
    column
}

fn summary(entries: &[EntryRef]) -> String {
    let (mut tools, mut thinking) = (0, 0);
    let mut names: HashMap<String, usize> = HashMap::new();
    for e in entries {
        let b = e.borrow();
        if b.data.kind == Kind::Tool {
            tools += 1;
            // By what they do ("Run command ×2"), from the name alone: the input isn't parsed here.
            *names.entry(steps::present(b.data.tool_name.as_deref().unwrap_or("Tool"), &serde_json::Value::Null, None).verb).or_default() += 1;
        } else {
            thinking += 1;
        }
    }
    let mut top: Vec<(String, usize)> = names.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top: Vec<String> = top.into_iter().take(4).map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n }).collect();
    let mut text = if tools > 0 {
        format!("{tools} tool call{}", if tools == 1 { "" } else { "s" })
    } else if thinking > 0 {
        "Thought".to_string()
    } else {
        String::new()
    };
    if !top.is_empty() {
        text.push_str(" — ");
        text.push_str(&top.join(", "));
    }
    text
}

fn work_row(state: &ThreadState, ctx: &RowContext, item: &Item, theme: &Theme) -> AnyElement {
    let pos = item.pos;
    // One step on its own needs no group around it: the step itself, with its own disclosure
    // (a lone plan call folded into a later one keeps its group's line, so it still shows).
    if item.entries.len() == 1 {
        let id = item.entries[0].borrow().data.id.clone();
        if !state.folded_plan_calls.contains(&id) {
            return step(state, ctx, pos, 0, &item.entries[0], theme);
        }
    }
    let last = item.entries.last().unwrap().borrow();
    let live = last.live
        || (last.data.kind == Kind::Tool && last.data.tool_result.is_none() && last.sub.as_ref().is_some_and(|s| s.status == "running"))
        || (last.data.kind == Kind::Tool && last.data.tool_result.is_none() && state.running);
    // A foreground command that's taking a while: what it printed last, under the header.
    let job_line = (last.data.kind == Kind::Tool && last.data.tool_result.is_none() && state.running).then(|| state.job_lines.get(&last.data.id).cloned()).flatten();
    let latest = if last.data.kind == Kind::Tool {
        let input: serde_json::Value = serde_json::from_str(last.data.tool_input.as_deref().unwrap_or("")).unwrap_or_default();
        let p = steps::present(last.data.tool_name.as_deref().unwrap_or("Tool"), &input, None);
        if p.detail.is_empty() { p.verb } else { format!("{} {}", p.verb, p.detail) }
    } else {
        "Thinking…".to_string()
    };
    drop(last);
    let open = item.open || expand_all();
    let handle = ctx.thread.clone();
    let head = div()
        .id(("work", pos as u64 as usize))
        .flex()
        .items_center()
        .gap(px(6.))
        .w_full()
        .h(px(26.))
        .px(px(6.))
        .rounded(px(radius::MEDIUM))
        .hover(|s| s.bg(theme.hover))
        .text_size(px(ts::FOOTNOTE))
        .cursor_pointer()
        .child(div().flex_none().w(px(14.)).flex().justify_center().child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 13., theme.secondary)))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(summary(&item.entries)))
        .when(live, |d| {
            d.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(ts::CAPTION)).text_color(theme.tertiary).child(latest))
        })
        .on_click(move |_, _, cx| {
            handle.update(cx, |t, cx| {
                t.toggle_item(pos);
                cx.notify();
            })
        });
    let mut row = div().flex().flex_col().text_color(theme.secondary).child(head);
    if let (Some(line), false) = (job_line, open) {
        row = row.child(div().pl(px(26.)).pb(px(3.)).overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(ts::CAPTION)).text_color(theme.tertiary).child(line));
    }
    if open {
        let mut body = div().flex().flex_col().gap(px(2.)).pl(px(space::M)).pt(px(2.)).pb(px(space::XS));
        for (n, e) in item.entries.iter().enumerate() {
            body = body.child(step(state, ctx, pos, n, e, theme));
        }
        row = row.child(body);
    }
    row.into_any_element()
}

/// A box's scroll and where it was before the latest wheel event, kept between frames by key.
type BoxScroll = (gpui_kit::ScrollHandle, Rc<std::cell::Cell<gpui_kit::Point<gpui_kit::Pixels>>>);

thread_local! {
    /// The scrolls of boxes that scroll inside a row (a long tool output), by what they show:
    /// rows are drawn anew each frame, their scroll isn't.
    static BOX_SCROLLS: std::cell::RefCell<HashMap<u64, BoxScroll>> = std::cell::RefCell::new(HashMap::new());
}

fn box_scroll(key: u64) -> BoxScroll {
    BOX_SCROLLS.with(|s| {
        let mut s = s.borrow_mut();
        // Kept for the boxes seen lately; the rest start at the top again.
        if s.len() > 400 {
            s.clear();
        }
        s.entry(key).or_insert_with(|| (gpui_kit::ScrollHandle::new(), Rc::new(std::cell::Cell::new(gpui_kit::Point::default())))).clone()
    })
}

/// A box capped at `max` tall whose content scrolls inside it. A vertical wheel moves the box
/// while it can go that way, and the transcript once it's at its end (the Swift app's
/// `TranscriptWheelRouter`): GPUI hands the event to every scroll under the pointer, so the box
/// stops it from reaching the list only when it took it.
fn capped_box(key: u64, max: f32, content: impl IntoElement) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (handle, before) = box_scroll(key);
    let h = handle.clone();
    div()
        .id(ElementId::NamedInteger("capped-box".into(), key))
        .max_h(px(max))
        .overflow_y_scroll()
        .track_scroll(&handle)
        .on_scroll(move |e: &gpui_kit::ScrollEvent, window, cx| {
            let delta = e.delta.pixel_delta(window.line_height());
            let (was, top) = (before.get(), h.max_offset());
            let inside = delta.y.abs() >= delta.x.abs()
                && ((delta.y > px(0.) && was.y < px(0.)) || (delta.y < px(0.) && was.y > -top.y));
            let now = h.offset();
            before.set(gpui_kit::point(now.x, now.y.clamp(-top.y, px(0.))));
            if inside {
                cx.stop_propagation();
            }
        })
        .child(content)
}

/// Monospaced text in a box: a command, a tool's input or output. `tint` colours it (a failure).
/// Past 320 pt it scrolls inside the box, as the Swift app's `CodeBox`.
fn mono_box(text: String, tint: Option<Hsla>, theme: &Theme) -> impl IntoElement {
    // Keyed by its length and start: hashing a long output whole on every frame costs.
    let key = copy_hash("mono-box", (text.len(), text.chars().take(160).collect::<String>()));
    capped_box(
        key,
        320.,
        div().px(px(10.)).py(px(space::S)).child(selectable(text, theme)),
    )
    .font_family(MONO)
    .text_size(px(ts::CAPTION))
    .line_height(px(16.))
    .bg(theme.code)
    .rounded(px(radius::MEDIUM))
    .text_color(tint.unwrap_or(theme.text))
}

/// A screenshot a browser tool took, where the browser kept it.
fn browser_shot(thread_id: &str, tool_use: &str) -> std::path::PathBuf {
    attachments_dir().join(format!("{thread_id}-browser-{tool_use}.jpg"))
}

/// A run of task tool calls, as the plan it leaves: the newest as the whole checklist, earlier
/// ones as a line saying where things stood.
fn plan_step(plan: &yh_core::plan::Plan, latest: bool, theme: &Theme) -> AnyElement {
    use yh_core::plan::Status;
    let done = plan.completed();
    let total = plan.items.len();
    if !latest {
        let mut parts = vec![format!("{done} of {total} done")];
        if let Some(current) = plan.current() {
            parts.push(current.active_form.clone().unwrap_or_else(|| current.subject.clone()));
        }
        return div()
            .flex()
            .items_center()
            .gap(px(space::S))
            .h(px(28.))
            .px(px(6.))
            .text_size(px(ts::FOOTNOTE))
            .child(div().w(px(16.)).flex_none().flex().justify_center().child(sized(list_checks_icon(), 14., theme.secondary)))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme.text).child("Plan"))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.secondary).child(parts.join(" · ")))
            .into_any_element();
    }
    let heading = match plan.current() {
        Some(current) => current.active_form.clone().unwrap_or_else(|| current.subject.clone()),
        None if plan.is_finished() => "Plan done".into(),
        None => "Plan".into(),
    };
    let mut list = div().flex().flex_col().gap(px(6.)).px(px(space::M)).py(px(10.)).border_t_1().border_color(theme.separator).bg(theme.canvas).text_size(px(ts::BODY));
    for item in &plan.items {
        let mark = match item.status {
            Status::Pending => sized(circle_dashed_icon(), 14., theme.tertiary),
            Status::InProgress => sized(circle_dot_icon(), 14., theme.accent),
            Status::Completed => icon(IconName::CircleCheck, 14., theme.success),
        };
        let text = if item.status == Status::InProgress { item.active_form.clone().unwrap_or_else(|| item.subject.clone()) } else { item.subject.clone() };
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(space::S))
                .child(div().w(px(16.)).pt(px(2.5)).flex_none().flex().justify_center().child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(item.status == Status::InProgress, |d| d.font_weight(FontWeight::MEDIUM).text_color(theme.text))
                        .when(item.status == Status::Pending, |d| d.text_color(theme.text))
                        .when(item.status == Status::Completed, |d| d.text_color(theme.secondary).line_through())
                        .child(text),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .rounded(px(radius::LARGE))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .overflow_hidden()
        .my(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S))
                .px(px(space::M))
                .py(px(space::S))
                .text_size(px(ts::FOOTNOTE))
                .child(div().w(px(16.)).flex_none().flex().justify_center().child(if plan.is_finished() {
                    icon(IconName::CircleCheck, 15., theme.success)
                } else {
                    sized(list_checks_icon(), 15., theme.accent)
                }))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(heading))
                .child(div().flex_none().text_color(theme.secondary).child(format!("{done} of {total}"))),
        )
        .child(list)
        .into_any_element()
}

/// Opens a Read, Write or Edit's file in the Files tab: where the edit went (looked up off the
/// main thread), or where the read started.
pub fn open_target(path: String, line: Option<usize>, written: Option<String>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let line = match (line, written) {
            (Some(line), _) => Some(line),
            (None, Some(text)) => {
                let p = path.clone();
                cx.background_executor().spawn(async move { steps::line_of(&p, &text) }).await
            }
            _ => None,
        };
        let link = match line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        };
        let _ = cx.update(|cx| crate::ui::inspector::open_link(&link, cx));
    })
    .detach();
}

/// The newest `tool` step's file, as its path's link has it: for `YH_OPEN_STEP_FILE`.
pub fn step_file_target(state: &ThreadState, tool: &str) -> Option<(String, Option<usize>, Option<String>)> {
    let root = state.linker.as_ref()?.root.clone();
    state.items.iter().rev().flat_map(|i| i.entries.iter().rev()).find_map(|e| {
        let b = e.borrow();
        if b.data.tool_name.as_deref() != Some(tool) {
            return None;
        }
        let input: serde_json::Value = serde_json::from_str(b.data.tool_input.as_deref().unwrap_or("")).ok()?;
        steps::file_target(tool, &input, &root)
    })
}

fn step(state: &ThreadState, ctx: &RowContext, pos: i64, n: usize, e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    if b.data.kind != Kind::Tool {
        drop(b);
        return thinking_step(&ctx.thread, pos, n, e, theme);
    }
    let id = b.data.id.clone();
    // A run of task tool calls shows once, as its plan, at its last call.
    if state.folded_plan_calls.contains(&id) {
        return div().into_any_element();
    }
    if let Some(plan) = state.plan_steps.get(&id) {
        // A plan with nothing in it yet (the first TaskCreate before its result): nothing to show.
        if plan.items.is_empty() {
            return div().into_any_element();
        }
        return plan_step(plan, state.latest_plan_step.as_deref() == Some(id.as_str()), theme);
    }
    let name = b.data.tool_name.clone().unwrap_or_else(|| "Tool".into());
    let input: serde_json::Value = serde_json::from_str(b.data.tool_input.as_deref().unwrap_or("")).unwrap_or_default();
    let done = b.data.tool_result.is_some();
    // Still going: the turn's still on, and its result hasn't come.
    let running = !done && (b.live || state.running);
    let failed = done && b.data.is_error;
    let p = steps::present(&name, &input, b.data.tool_result.as_deref());
    let target = ctx.root.as_deref().and_then(|root| steps::file_target(&name, &input, root));
    let open = b.open || expand_all();
    let is_browser = name.starts_with(steps::BROWSER_PREFIX);
    let shot = (name == format!("{}screenshot", steps::BROWSER_PREFIX) && done && !failed).then(|| browser_shot(&ctx.thread_id, &id)).filter(|p| p.exists());
    // A foreground command that's taking a while: what it printed last.
    let last_line = (running && !open).then(|| state.job_lines.get(&id).cloned()).flatten().filter(|l| !l.is_empty());
    // A job it started (a command in the background, one taking a while): how long it's run or
    // how it ended, in the header; open, what it printed.
    let job = state.jobs.for_tool(&id).map(|j| {
        let (label, failed) = j.status_label();
        (label, failed, j.status == crate::state::jobs::JobStatus::Running, j.background, j.output_file.is_some())
    });
    let job_output = job.as_ref().filter(|j| j.2 || j.3).map(|j| match state.job_outputs.get(&id) {
        Some(text) => text.clone(),
        None if j.4 => "…".to_string(),
        None => "Its output shows once it reports".to_string(),
    });
    let handle = ctx.thread.clone();
    let entry = e.clone();
    let detail_base = || {
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(MONO)
            .text_size(px(ts::CAPTION))
            .text_color(theme.secondary)
    };
    let detail: AnyElement = match target {
        Some((path, line, written)) if !p.detail.is_empty() => detail_base()
            .id(id_of("step-path", (&id, n)))
            .cursor_pointer()
            .hover(|s| s.text_color(theme.accent))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Show in Files").build(window, cx))
            .child(p.detail.clone())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                open_target(path.clone(), line, written.clone(), cx);
            })
            .into_any_element(),
        _ => detail_base().child(p.detail.clone()).into_any_element(),
    };
    // Closed: one quiet line (an icon, what it did, on what), a fill under the pointer. Open:
    // the same line heading a card with what went in and what came back.
    let head = div()
        .id(("step", ((pos as u64) << 20 | n as u64) as usize))
        .flex()
        .items_center()
        .gap(px(space::S))
        .w_full()
        .h(px(28.))
        .px(px(6.))
        .rounded(px(radius::MEDIUM))
        .when(!open, |d| d.hover(|s| s.bg(theme.hover)))
        .text_size(px(ts::FOOTNOTE))
        .cursor_pointer()
        .child(div().w(px(16.)).flex_none().flex().justify_center().child(icon(p.icon, 14., if failed { theme.error } else { theme.secondary })))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(if failed { theme.error } else { theme.text }).child(p.verb.clone()))
        .child(detail)
        .when_some(job.clone(), |d, (label, job_failed, job_running, _, _)| {
            d.child(div().flex_none().text_color(if job_failed { theme.error } else { theme.secondary }).child(label))
                .when(job_running, |d| d.child(icon(IconName::LoaderCircle, 13., theme.accent)))
        })
        .when(job.is_none() && running, |d| d.child(icon(IconName::LoaderCircle, 13., theme.accent)))
        .when(job.is_none() && failed, |d| d.child(icon(IconName::CircleX, 13., theme.error)))
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme.tertiary))
        .on_click(move |_, _, cx| {
            let entry = entry.clone();
            handle.update(cx, |t, cx| {
                t.toggle_step(pos, &entry);
                cx.notify();
            })
        });
    let mut column = div().flex().flex_col().child(head);
    if let Some(line) = last_line {
        column = column.child(
            div()
                .pl(px(30.))
                .pr(px(10.))
                .pb(px(space::XS))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(MONO)
                .text_size(px(ts::CAPTION))
                .text_color(theme.tertiary)
                .child(line),
        );
    }
    if let (Some(shot), false) = (&shot, open) {
        column = column.child(div().pl(px(30.)).pb(px(space::S)).child(gpui_kit::img(shot.clone()).max_w(px(260.)).max_h(px(160.)).rounded(px(radius::MEDIUM)).border_1().border_color(theme.separator)));
    }
    if open {
        let mut details = div().flex().flex_col().gap(px(space::S)).px(px(10.)).pt(px(space::XS)).pb(px(10.));
        details = match diff_of(&b) {
            Some(lines) => details.child(diff_view(lines, theme)),
            None if name == "Bash" && input["command"].is_string() => details.child(mono_box(format!("$ {}", input["command"].as_str().unwrap_or("")), None, theme)),
            // A Read of a whole file: its header already says which.
            None if name == "Read" && input.as_object().is_some_and(|o| o.keys().all(|k| k == "file_path")) => details,
            None if name == "AskUserQuestion" && steps::questions_text(&input).is_some() => {
                details.child(div().px(px(space::XS)).text_size(px(ts::BODY)).whitespace_normal().child(steps::questions_text(&input).unwrap_or_default()))
            }
            None => match b.data.tool_input.as_deref() {
                Some(raw) if !raw.is_empty() && raw != "{}" => details.child(mono_box(steps::pretty_json(raw), None, theme)),
                _ => details,
            },
        };
        if let Some(output) = job_output {
            // A background command's result only says it started: what it prints instead.
            details = details.child(mono_box(output, None, theme));
        } else if let Some(result) = b.data.tool_result.as_ref().filter(|r| !r.is_empty()) {
            let more = b.data.result_len.saturating_sub(result.len());
            let text = if more > 0 { format!("{result}\n… {more} more characters") } else { result.clone() };
            details = details.child(mono_box(text, failed.then_some(theme.error), theme));
        }
        if let Some(shot) = shot {
            details = details.child(gpui_kit::img(shot).max_w_full().max_h(px(480.)).rounded(px(radius::MEDIUM)).border_1().border_color(theme.separator));
        }
        if is_browser {
            details = details.child(
                div().flex().child(
                    div()
                        .id(id_of("show-browser", (&id, n)))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(radius::MEDIUM))
                        .border_1()
                        .border_color(theme.separator)
                        .bg(theme.canvas)
                        .text_size(px(ts::FOOTNOTE))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child(icon(IconName::Globe, 13., theme.secondary))
                        .child("Show Browser")
                        .on_click(|_, _, cx| show_panel(crate::state::workspace::Panel::Browser, cx)),
                ),
            );
        }
        column = column
            .rounded(px(radius::LARGE))
            .border_1()
            .border_color(if failed { theme.error.opacity(0.45) } else { theme.separator })
            .bg(theme.canvas)
            .overflow_hidden()
            .my(px(2.))
            .child(details);
    }
    column.into_any_element()
}

/// Claude thinking: "Thought", its first line, its text when opened.
fn thinking_step(thread: &Entity<ThreadState>, pos: i64, n: usize, e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    let gist: String = b.data.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect();
    let live = b.live;
    // Thinking kept from view (redacted): only the line, nothing to open.
    let empty = b.data.text.trim().is_empty();
    let open = (b.open || expand_all()) && !empty;
    drop(b);
    let handle = thread.clone();
    let entry = e.clone();
    let head = div()
        .id(("step", ((pos as u64) << 20 | n as u64) as usize))
        .flex()
        .items_center()
        .gap(px(space::S))
        .w_full()
        .h(px(28.))
        .px(px(6.))
        .rounded(px(radius::MEDIUM))
        .hover(|s| s.bg(theme.hover))
        .text_size(px(ts::FOOTNOTE))
        .text_color(theme.secondary)
        .cursor_pointer()
        .child(div().w(px(16.)).flex_none().flex().justify_center().child(sized(crate::ui::icons::brain(), 14., theme.secondary)))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(if live { "Thinking…" } else { "Thought" }))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.tertiary).when(!open, |d| d.child(gist)))
        .when(!empty, |d| d.child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme.tertiary)))
        .on_click(move |_, _, cx| {
            let entry = entry.clone();
            handle.update(cx, |t, cx| {
                t.toggle_step(pos, &entry);
                cx.notify();
            })
        });
    let mut column = div().flex().flex_col().child(head);
    if open {
        let md = markdown_of(e, None);
        column = column.child(
            div()
                .ml(px(13.))
                .pl(px(17.))
                .pr(px(10.))
                .py(px(space::XS))
                .border_l_1()
                .border_color(theme.separator)
                .text_size(px(ts::FOOTNOTE))
                .line_height(px(18.))
                .text_color(theme.secondary)
                .child(markdown_view_in(&md, theme, None, None)),
        );
    }
    column.into_any_element()
}

/// `YH_PREVIEW_MEDIA=1` (test runs): opens the selected thread's last pictures and videos
/// (shown by Claude, or sent with a message) in Quick Look, as clicking one does, prints PASS
/// when the panel took them, and closes it. It shows on the screen, briefly.
pub fn preview_probe(ws: &Entity<crate::state::workspace::Workspace>, cx: &mut App) {
    let Some(thread) = ws.read(cx).selected_thread() else { return eprintln!("PREVIEW FAIL: no thread") };
    let t = thread.read(cx);
    let mut files: Vec<String> = vec![];
    for item in t.items.iter().rev() {
        let b = item.entries[0].borrow();
        files = match item.ty {
            ItemType::Media => {
                let input: serde_json::Value = serde_json::from_str(b.data.tool_input.as_deref().unwrap_or("{}")).unwrap_or_default();
                media_paths(&input)
                    .iter()
                    .enumerate()
                    .map(|(n, p)| attachments_dir().join(media_name(t.id(), &b.data.id, n, p)).to_string_lossy().into_owned())
                    .filter(|p| std::path::Path::new(p).exists())
                    .collect()
            }
            ItemType::User => b.data.attachments.iter().filter(|n| media_kind(n).is_some()).map(|n| attachments_dir().join(n).to_string_lossy().into_owned()).collect(),
            _ => continue,
        };
        if !files.is_empty() {
            break;
        }
    }
    eprintln!("PREVIEW files: {files:?}");
    crate::platform::quick_look(files, 0);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
        let _ = cx.update(|_| eprintln!("PREVIEW {}", if crate::platform::quick_look_visible(true) { "PASS" } else { "FAIL" }));
    })
    .detach();
}
