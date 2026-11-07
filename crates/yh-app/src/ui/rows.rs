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
    let ctx = RowContext {
        thread: thread.clone(),
        pos: item.pos,
        thread_id: state.id().to_string().into(),
        project_id: state.meta.project_id.clone().into(),
        linker: state.linker.clone(),
        live: item.entries.last().is_some_and(|e| e.borrow().live),
    };
    let content = match item.ty {
        ItemType::User => user_row(item, &theme),
        ItemType::Text => text_row(&item.entries[0], &ctx, &theme, window),
        ItemType::Notice => notice_row(&item.entries[0], &theme),
        ItemType::Work => work_row(thread, item, &theme),
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

fn user_row(item: &Item, theme: &Theme) -> AnyElement {
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
    let media: Vec<String> = images
        .iter()
        .filter(|n| !n.ends_with('/') && media_kind(n).is_some())
        .map(|n| attachments_dir().join(n).to_string_lossy().into_owned())
        .collect();
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
                let index = media.iter().position(|m| *m == path);
                let all = media.clone();
                let tile = div()
                    .id(("user-image", ((pos as u64) << 8 | n as u64) as usize))
                    .cursor_pointer()
                    .when(queued, |d| d.opacity(0.6))
                    .on_click(move |_, _, _| match index {
                        Some(at) => crate::platform::quick_look(all.clone(), at),
                        None => {
                            let _ = std::process::Command::new("open").arg(&path).spawn();
                        }
                    });
                if media_kind(name) == Some(MediaKind::Image) && !name.contains('/') {
                    tile.child(gpui_kit::img(attachments_dir().join(name)).max_w(px(240.)).max_h(px(180.)).rounded(px(8.)))
                } else {
                    tile.px(px(10.)).py(px(8.)).rounded(px(8.)).bg(theme.card).text_size(px(12.)).child(yh_core::library::last_component(name.trim_end_matches('/')))
                }
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
                    .child(text.clone()),
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
            .child(div().text_color(theme.secondary).child("▣"))
            .child(div().when(caption.is_none(), |d| d.text_color(theme.secondary)).child(caption.unwrap_or(summary)))
            .when(!done, |d| d.child(div().text_color(theme.secondary).child("…"))),
    );
    if failed {
        column = column.child(div().text_size(px(12.5)).text_color(theme.error).child(result));
    } else {
        column = column.child(div().flex().flex_wrap().gap(px(10.)).children(items.into_iter().enumerate().map(|(n, (stored, original, kind))| {
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
        })));
    }
    column.into_any_element()
}

fn notice_row(e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    if b.data.kind == Kind::Command {
        drop(b);
        let md = markdown_of(e, None);
        return div().bg(theme.card).rounded(px(10.)).px(px(12.)).py(px(8.)).child(markdown_view(&md, theme)).into_any_element();
    }
    div()
        .w_full()
        .flex()
        .justify_center()
        .text_size(px(12.))
        .text_color(if b.data.is_error { theme.error } else { theme.secondary })
        .child(b.data.text.clone())
        .into_any_element()
}

/// A sub-agent's call: what it was asked, how it's going, then its report.
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
    let prompt = input["prompt"].as_str().unwrap_or("").to_string();
    let report = e.sub.as_ref().and_then(|s| s.report.clone()).or_else(|| e.data.tool_result.clone()).unwrap_or_default();
    drop(e);
    let (pos, open) = (item.pos, item.open || expand_all());
    let handle = thread.clone();
    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .border_1()
        .border_color(if running { theme.accent.opacity(0.5) } else { theme.separator })
        .rounded(px(10.))
        .bg(theme.card)
        .px(px(12.))
        .py(px(8.))
        .child(
            div()
                .id(("agent", pos as u64 as usize))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(if running { theme.accent } else { theme.secondary }))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_weight(FontWeight::SEMIBOLD).child(title))
                .when(!kind.is_empty(), |d| d.child(div().text_size(px(11.)).px(px(6.)).rounded(px(6.)).bg(theme.selected).child(kind)))
                .child(div().text_size(px(11.)).text_color(theme.secondary).child(if steps > 0 {
                    format!("{status} · {steps} steps")
                } else {
                    status.clone()
                }))
                .on_click(move |_, _, cx| {
                    handle.update(cx, |t, cx| {
                        t.toggle_item(pos);
                        cx.notify();
                    })
                }),
        );
    if open {
        if !prompt.is_empty() {
            card = card
                .child(div().text_size(px(11.)).text_color(theme.secondary).child("Asked"))
                .child(div().text_size(px(12.)).child(prompt));
        }
        if !report.is_empty() {
            let md = markdown::parse(&report);
            card = card
                .child(div().text_size(px(11.)).text_color(theme.secondary).child("Report"))
                .child(div().text_size(px(12.)).child(markdown_view(&md, theme)));
        }
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

fn styled_text(styled: &Text, theme: &Theme, ctx: Option<&RowContext>) -> AnyElement {
    let text: SharedString = styled.text.clone().into();
    let shaped = if styled.spans.is_empty() {
        StyledText::new(text)
    } else {
        StyledText::new(text).with_highlights(styled.spans.iter().map(|(r, s)| (r.clone(), highlight_style(*s, theme))))
    };
    if styled.links.is_empty() {
        return shaped.into_any_element();
    }
    // Web links open in the browser, file links in the Files tab; the paragraph's text names it.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&styled.text, &mut hasher);
    let id = std::hash::Hasher::finish(&hasher) as usize;
    let ranges: Vec<std::ops::Range<usize>> = styled.links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = styled.links.iter().map(|(_, u)| u.clone()).collect();
    let target = ctx.map(|c| (c.thread_id.clone(), c.project_id.clone()));
    gpui_kit::InteractiveText::new(("link-text", id), shaped)
        .on_click(ranges, move |ix, window, cx| {
            if let Some(url) = urls.get(ix) {
                follow(url, target.as_ref(), window, cx);
            }
        })
        .into_any_element()
}

pub fn markdown_view(md: &markdown::Markdown, theme: &Theme) -> impl IntoElement {
    markdown_view_in(md, theme, None, None)
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

/// A tool call's one-line gist: the command, the file, the pattern.
pub fn tool_gist(e: &crate::state::thread::Entry) -> String {
    if let Some(sub) = &e.sub {
        if !sub.description.is_empty() {
            return sub.description.clone();
        }
    }
    let input = e.data.tool_input.as_deref().unwrap_or("");
    if input.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(input) {
            for key in ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"] {
                if let Some(s) = value[key].as_str() {
                    return s.lines().next().unwrap_or("").chars().take(160).collect();
                }
            }
        }
    }
    input.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect()
}

fn summary(entries: &[EntryRef]) -> String {
    let (mut tools, mut thinking) = (0, 0);
    let mut names: HashMap<String, usize> = HashMap::new();
    for e in entries {
        let b = e.borrow();
        if b.data.kind == Kind::Tool {
            tools += 1;
            *names.entry(b.data.tool_name.clone().unwrap_or_else(|| "Tool".into())).or_default() += 1;
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

fn work_row(thread: &Entity<ThreadState>, item: &Item, theme: &Theme) -> AnyElement {
    let pos = item.pos;
    let last = item.entries.last().unwrap().borrow();
    let live = last.live
        || (last.data.kind == Kind::Tool && last.data.tool_result.is_none() && last.sub.as_ref().is_some_and(|s| s.status == "running"));
    let latest = if last.data.kind == Kind::Tool {
        format!("{} {}", last.data.tool_name.as_deref().unwrap_or("Tool"), tool_gist(&last))
    } else {
        "Thinking…".to_string()
    };
    drop(last);
    let open = item.open || expand_all();
    let handle = thread.clone();
    let head = div()
        .id(("work", pos as u64 as usize))
        .flex()
        .items_baseline()
        .gap(px(6.))
        .w_full()
        .py(px(3.))
        .text_size(px(12.))
        .child(div().w(px(10.)).child(if open { "⌄" } else { "›" }))
        .child(div().flex_none().child(summary(&item.entries)))
        .when(live, |d| {
            d.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_family(MONO).text_size(px(11.)).opacity(0.8).child(latest))
        })
        .on_click(move |_, _, cx| {
            handle.update(cx, |t, cx| {
                t.toggle_item(pos);
                cx.notify();
            })
        });
    let mut row = div().flex().flex_col().text_color(theme.secondary).child(head);
    if open {
        let mut body = div().flex().flex_col().border_l_2().border_color(theme.separator).ml(px(4.)).pl(px(10.)).mb(px(6.));
        for (n, e) in item.entries.iter().enumerate() {
            body = body.child(step(thread, pos, n, e, theme));
        }
        row = row.child(body);
    }
    row.into_any_element()
}

fn mono_box(text: String, theme: &Theme) -> impl IntoElement {
    div()
        .font_family(MONO)
        .text_size(px(11.))
        .line_height(px(15.))
        .bg(theme.code)
        .rounded(px(6.))
        .px(px(8.))
        .py(px(6.))
        .my(px(2.))
        .text_color(theme.text)
        .child(text)
}

fn step(thread: &Entity<ThreadState>, pos: i64, n: usize, e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    let is_tool = b.data.kind == Kind::Tool;
    let gist = if is_tool { tool_gist(&b) } else { b.data.text.lines().next().unwrap_or("").chars().take(160).collect() };
    let sub = b.sub.as_ref().map(|s| format!("{}{}", s.status, if s.steps > 0 { format!(" · {} steps", s.steps) } else { String::new() }));
    let name = if is_tool { b.data.tool_name.clone().unwrap_or_else(|| "Tool".into()) } else { "Thinking".into() };
    let name_color: Hsla = if b.data.is_error { theme.error } else { theme.text };
    let open = b.open || expand_all();
    let handle = thread.clone();
    let entry = e.clone();
    let head = div()
        .id(("step", ((pos as u64) << 20 | n as u64) as usize))
        .flex()
        .gap(px(8.))
        .w_full()
        .py(px(2.))
        .text_size(px(12.))
        .items_baseline()
        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(name_color).child(name))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_family(MONO).text_size(px(11.)).child(gist))
        .when_some(sub, |d, s| d.child(div().flex_none().text_size(px(11.)).child(s)))
        .on_click(move |_, _, cx| {
            let entry = entry.clone();
            handle.update(cx, |t, cx| {
                t.toggle_step(pos, &entry);
                cx.notify();
            })
        });
    let mut column = div().flex().flex_col().child(head);
    if open {
        if is_tool {
            column = match diff_of(&b) {
                Some(lines) => column.child(diff_view(lines, theme)),
                None => column.child(mono_box(b.data.tool_input.clone().unwrap_or_default(), theme)),
            };
            if let Some(result) = &b.data.tool_result {
                let more = b.data.result_len.saturating_sub(result.len());
                let text = if more > 0 { format!("{result}\n… {more} more characters") } else { result.clone() };
                column = column.child(mono_box(text, theme));
            }
        } else {
            drop(b);
            let md = markdown_of(e, None);
            column = column.child(div().text_size(px(12.)).py(px(4.)).child(markdown_view(&md, theme)));
        }
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
