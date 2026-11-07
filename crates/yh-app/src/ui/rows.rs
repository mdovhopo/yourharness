//! One kind of row per item type, as web/ui/src/Rows.tsx. The list asks for a row only while
//! it's on screen (or about to be), and measures it again only when it was spliced.

use yh_core::markdown::{self, MdBlock, Span, Styled as Text};
use crate::state::harness_tools::{media_kind, media_name, media_paths, MediaKind};
use crate::state::thread::{EntryRef, Fold, Item, ItemType, ThreadState, TurnWork};
use yh_core::library::Kind;
use yh_core::links::Linker;
use crate::theme::{Theme, MONO};
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

fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

/// Green, for done.
fn green() -> Hsla {
    gpui_kit::rgb(0x34c759).into()
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
                .gap(px(6.))
                .children(summary)
                .when(open, |d| d.child(work_box(content, &theme)))
                .into_any_element()
        }
        Fold::Open => work_box(content, &theme),
        _ => content,
    };
    let hit = state.find_hit == Some(item.pos);
    // The column: centred, at most 820 wide, as the web transcript's.
    div()
        .when(hit, |d| d.bg(theme.accent.opacity(0.10)))
        .w_full()
        .flex()
        .justify_center()
        .px(px(28.))
        .py(px(4.))
        .child(div().w_full().max_w(px(764.)).child(content))
        .into_any_element()
}

/// A finished turn's work shown open: a rule down its side, as one group under its summary.
fn work_box(content: AnyElement, theme: &Theme) -> AnyElement {
    div().border_l_2().border_color(theme.separator).ml(px(8.)).pl(px(12.)).child(content).into_any_element()
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
                    files.insert(path.to_string());
                }
            }
        }
    }
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
    div()
        .id(("turn", pos as u64 as usize))
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .px(px(12.))
        .py(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .hover(|s| s.bg(theme.selected))
        .cursor_pointer()
        .text_size(px(12.5))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(worked))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(theme.secondary)
                .when(!details.is_empty(), |d| d.child(format!("· {details}"))),
        )
        .child(div().flex_none().text_size(px(11.)).text_color(theme.secondary).child(if open { "Hide work" } else { "Show work" }))
        .child(div().flex_none().w(px(10.)).text_color(theme.secondary).child(if open { "⌄" } else { "›" }))
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

/// A small Copy button: what it copies goes on the clipboard as plain text.
fn copy_button(id: ElementId, text: String, label: &'static str, theme: &Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(6.))
        .rounded(px(5.))
        .text_size(px(11.))
        .line_height(px(17.))
        .text_color(theme.secondary)
        .cursor_pointer()
        .hover(|s| s.bg(theme.selected))
        .child(label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
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
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .text_size(px(12.))
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
    // Sent while Claude worked: waiting for the current step, or how it got in.
    let note = if e.queued {
        Some("Queued")
    } else {
        match e.delivery.as_deref() {
            Some("steered") => Some("Steered"),
            Some("interrupted") => Some("Interrupted and sent"),
            _ => None,
        }
    };
    let queued = e.queued;
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
        .py(px(4.))
        .group("user-message")
        .when_some(note, |d, n| d.child(div().text_size(px(11.)).text_color(theme.secondary).child(n)))
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
                    .when(queued, |d| d.opacity(0.6))
                    .on_click(move |_, _, _| match index {
                        Some(at) => crate::platform::quick_look(all.clone(), at),
                        None => {
                            let _ = std::process::Command::new("open").arg(&target).spawn();
                        }
                    });
                let tile = match kind {
                    Some(MediaKind::Image) => tile.child(gpui_kit::img(std::path::PathBuf::from(&path)).max_w(px(240.)).max_h(px(180.)).rounded(px(8.))),
                    Some(MediaKind::Video) => tile
                        .w(px(220.))
                        .h(px(124.))
                        .rounded(px(8.))
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
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(14.))
                    .when(queued, |d| d.opacity(0.6))
                    .child(user_text(&text, ctx, theme, window)),
            )
        })
        // Copy and when it was sent, small under it. Copy's room is always kept: hovering
        // mustn't change the row's height.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(17.))
                .when(!text.is_empty(), |d| {
                    d.child(copy_button(id_of("copy-user", (pos, &text)), text.clone(), "Copy", theme).opacity(0.).group_hover("user-message", |s| s.opacity(1.)))
                })
                .when_some(date, |d, date| {
                    let full = yh_core::clock::full_time(date);
                    d.child(
                        div()
                            .id(("user-time", pos as u64 as usize))
                            .text_size(px(10.5))
                            .text_color(theme.secondary.opacity(0.75))
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
        .child(div().h(px(17.)).mt(px(2.)).flex().when(!live, |d| {
            d.child(copy_button(id_of("copy-reply", (ctx.pos, &text)), text, "Copy", theme).opacity(0.).group_hover("reply", |s| s.opacity(1.)))
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
    let mut column = div().flex().flex_col().gap(px(8.)).py(px(4.)).child(
        div()
            .flex()
            .gap(px(6.))
            .text_size(px(12.5))
            .child(icon(IconName::GalleryVerticalEnd, 14., theme.secondary))
            .child(div().when(caption.is_none(), |d| d.text_color(theme.secondary)).child(caption.unwrap_or(summary)))
            .when(!done, |d| d.child(div().text_color(theme.secondary).child("…"))),
    );
    if failed {
        column = column.child(div().text_size(px(12.5)).text_color(theme.error).child(result));
    } else {
        column = column.child(div().flex().flex_wrap().gap(px(10.)).children(items.into_iter().enumerate().map(|(n, (stored, original, kind))| {
            let (menu_all, menu_path) = (all.clone(), stored.clone());
            let all = all.clone();
            let (max_w, max_h) = if single { (640., 480.) } else { (320., 240.) };
            let tile = div()
                .id(id_of("media", (&stored, n)))
                .cursor_pointer()
                .rounded(px(8.))
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
                    .child(div().text_size(px(34.)).text_color(gpui_kit::white().opacity(0.9)).child("▶"))
                    .child(div().text_size(px(11.)).text_color(gpui_kit::white().opacity(0.7)).child(yh_core::library::last_component(&original))),
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
        return div().bg(theme.card).rounded(px(10.)).px(px(12.)).py(px(8.)).child(markdown_view_in(&md, theme, None, None)).into_any_element();
    }
    div()
        .w_full()
        .flex()
        .justify_center()
        .text_size(px(12.))
        .text_color(if b.data.is_error { theme.error } else { theme.secondary })
        .child(selectable(b.data.text.clone(), theme))
        .into_any_element()
}

/// A sub-agent's state as an icon: working, done, failed, stopped.
fn status_icon(status: &str, theme: &Theme) -> Icon {
    match status {
        "running" => icon(IconName::LoaderCircle, 15., theme.accent),
        "completed" => icon(IconName::CircleCheck, 15., green()),
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
    let report = e.sub.as_ref().and_then(|s| s.report.clone()).or_else(|| e.data.tool_result.clone()).unwrap_or_default();
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
            theme.accent.opacity(0.5)
        } else if failed {
            theme.error.opacity(0.5)
        } else {
            theme.separator
        })
        .rounded(px(10.))
        .bg(theme.card)
        .child(
            div()
                .id(("agent", pos as u64 as usize))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .cursor_pointer()
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
                                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).child(title))
                                .when(!kind.is_empty(), |d| d.child(div().flex_none().text_size(px(11.)).px(px(6.)).rounded(px(6.)).bg(theme.selected).text_color(theme.secondary).child(kind))),
                        )
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(12.)).text_color(theme.secondary).child(subagent_stats(&status, steps, activity))),
                )
                .when(followable, |d| {
                    d.child(
                        div()
                            .id(("agent-follow", pos as u64 as usize))
                            .flex_none()
                            .p(px(4.))
                            .rounded(px(5.))
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
                .when(!report.is_empty(), |d| d.child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme.secondary.opacity(0.7))))
                .on_click(move |_, _, cx| {
                    handle.update(cx, |t, cx| {
                        t.toggle_item(pos);
                        cx.notify();
                    })
                }),
        );
    if !recent.is_empty() {
        let mut list = div().flex().flex_col().gap(px(5.)).px(px(12.)).py(px(8.)).border_t_1().border_color(theme.separator).text_size(px(12.));
        for (p, live, failed) in recent {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(icon(p.icon, 13., theme.secondary))
                    .child(div().flex_none().child(p.verb))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(11.)).text_color(theme.secondary).child(p.detail))
                    .when(live, |d| d.child(icon(IconName::LoaderCircle, 12., theme.secondary)))
                    .when(failed, |d| d.child(icon(IconName::CircleX, 12., theme.error))),
            );
        }
        card = card.child(list);
    }
    if open && !report.is_empty() {
        let md = markdown::parse(&report);
        card = card.child(div().px(px(12.)).py(px(10.)).border_t_1().border_color(theme.separator).text_size(px(13.)).child(markdown_view_in(&md, theme, None, None)));
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

fn diff_view(lines: Vec<(char, String)>, theme: &Theme) -> impl IntoElement {
    let mut column = div().font_family(MONO).text_size(px(11.)).line_height(px(15.)).bg(theme.code).rounded(px(6.)).py(px(4.)).my(px(2.));
    for (kind, text) in lines {
        let row = div().px(px(8.)).whitespace_normal();
        column = column.child(match kind {
            '-' => row.bg(theme.error.opacity(0.12)).text_color(theme.text).child(format!("- {text}")),
            '+' => row.bg(gpui_kit::Hsla::from(gpui_kit::rgb(0x34c759)).opacity(0.14)).text_color(theme.text).child(format!("+ {text}")),
            'f' => row.text_color(theme.secondary).child(text),
            _ => row.text_color(theme.secondary).child(text),
        });
    }
    column
}

fn highlight_style(span: Span, theme: &Theme) -> HighlightStyle {
    match span {
        Span::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
        Span::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
        Span::Code => HighlightStyle { background_color: Some(theme.code), color: Some(theme.string), ..Default::default() },
        Span::Link => HighlightStyle { color: Some(theme.accent), ..Default::default() },
        Span::CodeLink => HighlightStyle { background_color: Some(theme.code), color: Some(theme.accent), ..Default::default() },
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

/// Plain text you can select (a tool's output, a notice).
pub fn selectable(text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    let text: SharedString = text.into();
    let (order, id) = next_reading_order(&text);
    SelText::new(id, text.clone(), StyledText::new(text), order, theme.accent.opacity(0.28)).into_any_element()
}

fn styled_text(styled: &Text, theme: &Theme, ctx: Option<&RowContext>) -> AnyElement {
    let text: SharedString = styled.text.clone().into();
    let shaped = if styled.spans.is_empty() {
        StyledText::new(text.clone())
    } else {
        StyledText::new(text.clone()).with_highlights(styled.spans.iter().map(|(r, s)| (r.clone(), highlight_style(*s, theme))))
    };
    let (order, id) = next_reading_order(&styled.text);
    let element = SelText::new(id, text, shaped, order, theme.accent.opacity(0.28));
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

/// A code block, with Copy over its corner while the pointer is over it.
fn code_block(code: &Text, theme: &Theme, ctx: Option<&RowContext>, n: usize) -> AnyElement {
    div()
        .relative()
        .group("code")
        .bg(theme.code)
        .border_1()
        .border_color(theme.separator)
        .rounded(px(8.))
        .px(px(12.))
        .py(px(10.))
        .font_family(MONO)
        .text_size(px(12.))
        .line_height(px(17.))
        .child(styled_text(code, theme, None))
        .when_some(ctx, |d, ctx| {
            d.child(
                div().absolute().top(px(4.)).right(px(4.)).font_family(crate::theme::UI).child(
                    copy_button(id_of("copy-code", (ctx.pos, n, &code.text)), code.text.clone(), "Copy", theme)
                        .bg(theme.code)
                        .opacity(0.)
                        .group_hover("code", |s| s.opacity(1.)),
                ),
            )
        })
        .into_any_element()
}

/// Markdown blocks; in a row (`ctx`), with links that know the thread, Copy on code and
/// tables, and Mermaid diagrams drawn.
pub fn markdown_view_in(md: &markdown::Markdown, theme: &Theme, ctx: Option<&RowContext>, window: Option<&Window>) -> impl IntoElement {
    let mut column = div().flex().flex_col().gap(px(8.)).w_full();
    for (n, block) in md.blocks.iter().enumerate() {
        let element: AnyElement = match block {
            MdBlock::Paragraph(s) => div().child(styled_text(s, theme, ctx)).into_any_element(),
            MdBlock::Heading(level, s) => div()
                .pt(px(6.))
                .text_size(px(match level {
                    1 => 18.,
                    2 => 16.,
                    3 => 14.,
                    _ => 13.,
                }))
                .font_weight(FontWeight::SEMIBOLD)
                .child(styled_text(s, theme, ctx))
                .into_any_element(),
            MdBlock::Code { lang, code } if lang.eq_ignore_ascii_case("mermaid") => match (ctx, window) {
                (Some(ctx), Some(window)) => crate::ui::diagram::diagram_block(&code.text, ctx, n, theme, window)
                    .unwrap_or_else(|| code_block(code, theme, Some(ctx), n)),
                _ => code_block(code, theme, ctx, n),
            },
            MdBlock::Code { code, .. } => code_block(code, theme, ctx, n),
            MdBlock::List(items) => {
                let mut list = div().flex().flex_col().gap(px(2.));
                for (depth, number, s) in items {
                    let marker = number.map_or("•".to_string(), |n| format!("{n}."));
                    list = list.child(
                        div()
                            .flex()
                            .pl(px(4. + 18. * *depth as f32))
                            .child(div().w(px(18.)).flex_none().text_color(theme.secondary).child(marker))
                            .child(div().flex_1().min_w_0().child(styled_text(s, theme, ctx))),
                    );
                }
                list.into_any_element()
            }
            MdBlock::Quote(s) => div()
                .border_l(px(3.))
                .border_color(theme.separator)
                .pl(px(10.))
                .text_color(theme.secondary)
                .child(styled_text(s, theme, ctx))
                .into_any_element(),
            MdBlock::Table { header, rows } => {
                let columns = header.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
                let cell = |s: Option<&Text>, bold: bool| {
                    let mut c = div().flex_1().min_w_0().px(px(8.)).py(px(4.)).border_1().border_color(theme.separator);
                    if bold {
                        c = c.font_weight(FontWeight::SEMIBOLD);
                    }
                    if let Some(s) = s {
                        c = c.child(styled_text(s, theme, ctx));
                    }
                    c
                };
                let mut table = div().flex().flex_col();
                table = table.child(div().flex().children((0..columns).map(|i| cell(header.get(i), true))));
                for row in rows {
                    table = table.child(div().flex().children((0..columns).map(|i| cell(row.get(i), false))));
                }
                // Copy as tab-separated values, over its top corner (its room kept).
                match ctx {
                    Some(ctx) => {
                        let tsv = markdown::table_tsv(header, rows);
                        div()
                            .flex()
                            .flex_col()
                            .group("table")
                            .child(div().h(px(17.)).flex().justify_end().child(
                                copy_button(id_of("copy-table", (ctx.pos, n, &tsv)), tsv, "Copy table", theme).opacity(0.).group_hover("table", |s| s.opacity(1.)),
                            ))
                            .child(table)
                            .into_any_element()
                    }
                    None => table.into_any_element(),
                }
            }
            MdBlock::Rule => div().h(px(1.)).bg(theme.separator).into_any_element(),
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
        .py(px(3.))
        .text_size(px(12.))
        .cursor_pointer()
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 12., theme.secondary))
        .child(div().flex_none().child(summary(&item.entries)))
        .when(live, |d| {
            d.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(11.)).opacity(0.8).child(latest))
        })
        .on_click(move |_, _, cx| {
            handle.update(cx, |t, cx| {
                t.toggle_item(pos);
                cx.notify();
            })
        });
    let mut row = div().flex().flex_col().text_color(theme.secondary).child(head);
    if let (Some(line), false) = (job_line, open) {
        row = row.child(div().pl(px(18.)).pb(px(3.)).overflow_hidden().whitespace_nowrap().text_ellipsis().font_family(MONO).text_size(px(11.)).child(line));
    }
    if open {
        let mut body = div().flex().flex_col().gap(px(4.)).border_l_2().border_color(theme.separator).ml(px(5.)).pl(px(10.)).mb(px(6.));
        for (n, e) in item.entries.iter().enumerate() {
            body = body.child(step(state, ctx, pos, n, e, theme));
        }
        row = row.child(body);
    }
    row.into_any_element()
}

/// Monospaced text in a box: a command, a tool's input or output. `tint` colours it (a failure).
fn mono_box(text: String, tint: Option<Hsla>, theme: &Theme) -> impl IntoElement {
    div()
        .font_family(MONO)
        .text_size(px(11.))
        .line_height(px(15.))
        .bg(theme.code)
        .rounded(px(6.))
        .px(px(8.))
        .py(px(6.))
        .text_color(tint.unwrap_or(theme.text))
        .child(selectable(text, theme))
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
            .gap(px(8.))
            .px(px(12.))
            .py(px(3.))
            .text_size(px(12.5))
            .child(icon(IconName::CircleCheck, 14., theme.secondary))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme.text).child("Plan"))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.secondary).child(parts.join(" · ")))
            .into_any_element();
    }
    let heading = match plan.current() {
        Some(current) => current.active_form.clone().unwrap_or_else(|| current.subject.clone()),
        None if plan.is_finished() => "Plan done".into(),
        None => "Plan".into(),
    };
    let mut list = div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(10.)).border_t_1().border_color(theme.separator).text_size(px(12.5));
    for item in &plan.items {
        let (name, color) = match item.status {
            Status::Pending => (IconName::Square, theme.secondary.opacity(0.6)),
            Status::InProgress => (IconName::LoaderCircle, theme.accent),
            Status::Completed => (IconName::CircleCheck, green()),
        };
        let text = if item.status == Status::InProgress { item.active_form.clone().unwrap_or_else(|| item.subject.clone()) } else { item.subject.clone() };
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(div().w(px(16.)).pt(px(2.)).flex().justify_center().child(icon(name, 13., color)))
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
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.card)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .text_size(px(12.5))
                .child(icon(if plan.is_finished() { IconName::CircleCheck } else { IconName::LoaderCircle }, 15., if plan.is_finished() { green() } else { theme.accent }))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(heading))
                .child(div().flex_none().text_color(theme.secondary).child(format!("{done} of {total}"))),
        )
        .child(list)
        .into_any_element()
}

/// Opens a Read, Write or Edit's file in the Files tab: where the edit went (looked up off the
/// main thread), or where the read started.
fn open_target(path: String, line: Option<usize>, written: Option<String>, cx: &mut App) {
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
    let handle = ctx.thread.clone();
    let entry = e.clone();
    let detail: AnyElement = match target {
        Some((path, line, written)) if !p.detail.is_empty() => div()
            .id(id_of("step-path", (&id, n)))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(MONO)
            .text_size(px(11.5))
            .text_color(theme.secondary)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.accent))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Show in Files").build(window, cx))
            .child(p.detail.clone())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                open_target(path.clone(), line, written.clone(), cx);
            })
            .into_any_element(),
        _ => div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .font_family(MONO)
            .text_size(px(11.5))
            .text_color(theme.secondary)
            .child(p.detail.clone())
            .into_any_element(),
    };
    let head = div()
        .id(("step", ((pos as u64) << 20 | n as u64) as usize))
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .text_size(px(12.5))
        .cursor_pointer()
        .child(div().w(px(16.)).flex_none().flex().justify_center().child(icon(p.icon, 14., theme.secondary)))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(theme.text).child(p.verb.clone()))
        .child(detail)
        .when(running, |d| d.child(icon(IconName::LoaderCircle, 13., theme.accent)))
        .when(failed, |d| d.child(icon(IconName::CircleX, 13., theme.error)))
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 11., theme.secondary.opacity(0.7)))
        .on_click(move |_, _, cx| {
            let entry = entry.clone();
            handle.update(cx, |t, cx| {
                t.toggle_step(pos, &entry);
                cx.notify();
            })
        });
    let mut column = div()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .border_1()
        .border_color(if failed { theme.error.opacity(0.5) } else { theme.separator })
        .bg(theme.card)
        .child(head);
    if let Some(line) = last_line {
        column = column.child(
            div()
                .pl(px(34.))
                .pr(px(10.))
                .pb(px(6.))
                .mt(px(-2.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_family(MONO)
                .text_size(px(11.))
                .text_color(theme.secondary)
                .child(line),
        );
    }
    if let (Some(shot), false) = (&shot, open) {
        column = column.child(div().px(px(10.)).pb(px(8.)).child(gpui_kit::img(shot.clone()).max_w(px(260.)).max_h(px(160.)).rounded(px(6.)).border_1().border_color(theme.separator)));
    }
    if open {
        let mut details = div().flex().flex_col().gap(px(8.)).px(px(10.)).py(px(8.)).border_t_1().border_color(theme.separator);
        details = match diff_of(&b) {
            Some(lines) => details.child(diff_view(lines, theme)),
            None if name == "Bash" && input["command"].is_string() => details.child(mono_box(format!("$ {}", input["command"].as_str().unwrap_or("")), None, theme)),
            None => match b.data.tool_input.as_deref() {
                Some(raw) if !raw.is_empty() && raw != "{}" => details.child(mono_box(steps::pretty_json(raw), None, theme)),
                _ => details,
            },
        };
        if let Some(result) = b.data.tool_result.as_ref().filter(|r| !r.is_empty()) {
            let more = b.data.result_len.saturating_sub(result.len());
            let text = if more > 0 { format!("{result}\n… {more} more characters") } else { result.clone() };
            details = details.child(mono_box(text, failed.then_some(theme.error), theme));
        }
        if let Some(shot) = shot {
            details = details.child(gpui_kit::img(shot).max_w_full().max_h(px(480.)).rounded(px(6.)).border_1().border_color(theme.separator));
        }
        if is_browser {
            details = details.child(
                div().flex().child(
                    div()
                        .id(id_of("show-browser", (&id, n)))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.separator)
                        .bg(theme.canvas)
                        .text_size(px(12.))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.selected))
                        .child(icon(IconName::Globe, 13., theme.secondary))
                        .child("Show Browser")
                        .on_click(|_, _, cx| show_panel(crate::state::workspace::Panel::Browser, cx)),
                ),
            );
        }
        column = column.child(details);
    }
    column.into_any_element()
}

/// Claude thinking: "Thought", its first line, its text when opened.
fn thinking_step(thread: &Entity<ThreadState>, pos: i64, n: usize, e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    let gist: String = b.data.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect();
    let live = b.live;
    let open = b.open || expand_all();
    drop(b);
    let handle = thread.clone();
    let entry = e.clone();
    let head = div()
        .id(("step", ((pos as u64) << 20 | n as u64) as usize))
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .px(px(10.))
        .py(px(3.))
        .text_size(px(12.5))
        .cursor_pointer()
        .child(div().w(px(16.)).flex_none().flex().justify_center().child(icon(IconName::Asterisk, 13., theme.secondary)))
        .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(if live { "Thinking…" } else { "Thought" }))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(theme.secondary).when(!open, |d| d.child(gist)))
        .child(icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 11., theme.secondary.opacity(0.7)))
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
        column = column.child(div().text_size(px(12.)).pl(px(34.)).pr(px(10.)).py(px(4.)).text_color(theme.secondary).child(markdown_view_in(&md, theme, None, None)));
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
