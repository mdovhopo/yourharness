//! One kind of row per item type, as web/ui/src/Rows.tsx. The list asks for a row only while
//! it's on screen (or about to be), and measures it again only when it was spliced.

use yh_core::markdown::{self, MdBlock, Span, Styled as Text};
use crate::state::thread::{EntryRef, Item, ItemType, ThreadState};
use yh_core::library::Kind;
use crate::theme::{Theme, MONO};
use gpui_kit::{
    div, prelude::*, px, AnyElement, App, Entity, FontStyle, FontWeight, HighlightStyle, Hsla, SharedString,
    StrikethroughStyle, StyledText, Window,
};
use std::collections::HashMap;
use std::rc::Rc;

pub fn render_item(thread: &Entity<ThreadState>, ix: usize, _window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = Theme::get(cx);
    let state = thread.read(cx);
    let Some(item) = state.item_at(ix) else { return div().into_any_element() };
    let content = match item.ty {
        ItemType::User => user_row(item, &theme),
        ItemType::Text => text_row(&item.entries[0], &theme),
        ItemType::Notice => notice_row(&item.entries[0], &theme),
        ItemType::Work => work_row(thread, item, &theme),
    };
    // The column: centred, at most 820 wide, as the web transcript's.
    div()
        .w_full()
        .flex()
        .justify_center()
        .px(px(28.))
        .py(px(4.))
        .child(div().w_full().max_w(px(764.)).child(content))
        .into_any_element()
}

fn user_row(item: &Item, theme: &Theme) -> AnyElement {
    let e = item.entries[0].borrow();
    let text = e.data.text.clone();
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
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.))
        .py(px(4.))
        .when_some(note, |d, n| d.child(div().text_size(px(11.)).text_color(theme.secondary).child(n)))
        .child(
            div()
                .max_w(gpui_kit::relative(0.8))
                .bg(theme.bubble)
                .px(px(12.))
                .py(px(8.))
                .rounded(px(14.))
                .when(queued, |d| d.opacity(0.6))
                .child(text),
        )
        .into_any_element()
}

fn markdown_of(e: &EntryRef) -> Rc<markdown::Markdown> {
    let mut b = e.borrow_mut();
    if let Some((version, md)) = &b.markdown {
        if *version == b.version {
            return md.clone();
        }
    }
    let md = Rc::new(markdown::parse(&b.data.text));
    b.markdown = Some((b.version, md.clone()));
    md
}

fn text_row(e: &EntryRef, theme: &Theme) -> AnyElement {
    let md = markdown_of(e);
    markdown_view(&md, theme).into_any_element()
}

fn notice_row(e: &EntryRef, theme: &Theme) -> AnyElement {
    let b = e.borrow();
    if b.data.kind == Kind::Command {
        drop(b);
        let md = markdown_of(e);
        return div().bg(theme.card).rounded(px(10.)).px(px(12.)).py(px(8.)).child(markdown_view(&md, theme)).into_any_element();
    }
    div()
        .w_full()
        .flex()
        .justify_center()
        .text_size(px(12.))
        .text_color(theme.secondary)
        .child(b.data.text.clone())
        .into_any_element()
}

fn highlight_style(span: Span, theme: &Theme) -> HighlightStyle {
    match span {
        Span::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
        Span::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
        Span::Code => HighlightStyle { background_color: Some(theme.code), color: Some(theme.string), ..Default::default() },
        Span::Link => HighlightStyle { color: Some(theme.accent), ..Default::default() },
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

fn styled_text(styled: &Text, theme: &Theme) -> StyledText {
    let text: SharedString = styled.text.clone().into();
    if styled.spans.is_empty() {
        return StyledText::new(text);
    }
    StyledText::new(text).with_highlights(styled.spans.iter().map(|(r, s)| (r.clone(), highlight_style(*s, theme))))
}

pub fn markdown_view(md: &markdown::Markdown, theme: &Theme) -> impl IntoElement {
    let mut column = div().flex().flex_col().gap(px(8.)).w_full();
    for block in &md.blocks {
        let element: AnyElement = match block {
            MdBlock::Paragraph(s) => div().child(styled_text(s, theme)).into_any_element(),
            MdBlock::Heading(level, s) => div()
                .pt(px(6.))
                .text_size(px(match level {
                    1 => 18.,
                    2 => 16.,
                    3 => 14.,
                    _ => 13.,
                }))
                .font_weight(FontWeight::SEMIBOLD)
                .child(styled_text(s, theme))
                .into_any_element(),
            MdBlock::Code(s) => div()
                .bg(theme.code)
                .border_1()
                .border_color(theme.separator)
                .rounded(px(8.))
                .px(px(12.))
                .py(px(10.))
                .font_family(MONO)
                .text_size(px(12.))
                .line_height(px(17.))
                .child(styled_text(s, theme))
                .into_any_element(),
            MdBlock::List(items) => {
                let mut list = div().flex().flex_col().gap(px(2.));
                for (depth, number, s) in items {
                    let marker = number.map_or("•".to_string(), |n| format!("{n}."));
                    list = list.child(
                        div()
                            .flex()
                            .pl(px(4. + 18. * *depth as f32))
                            .child(div().w(px(18.)).flex_none().text_color(theme.secondary).child(marker))
                            .child(div().flex_1().min_w_0().child(styled_text(s, theme))),
                    );
                }
                list.into_any_element()
            }
            MdBlock::Quote(s) => div()
                .border_l(px(3.))
                .border_color(theme.separator)
                .pl(px(10.))
                .text_color(theme.secondary)
                .child(styled_text(s, theme))
                .into_any_element(),
            MdBlock::Table { header, rows } => {
                let columns = header.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
                let cell = |s: Option<&Text>, bold: bool| {
                    let mut c = div().flex_1().min_w_0().px(px(8.)).py(px(4.)).border_1().border_color(theme.separator);
                    if bold {
                        c = c.font_weight(FontWeight::SEMIBOLD);
                    }
                    if let Some(s) = s {
                        c = c.child(styled_text(s, theme));
                    }
                    c
                };
                let mut table = div().flex().flex_col();
                table = table.child(div().flex().children((0..columns).map(|i| cell(header.get(i), true))));
                for row in rows {
                    table = table.child(div().flex().children((0..columns).map(|i| cell(row.get(i), false))));
                }
                table.into_any_element()
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
    let open = item.open;
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
    let open = b.open;
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
            column = column.child(mono_box(b.data.tool_input.clone().unwrap_or_default(), theme));
            if let Some(result) = &b.data.tool_result {
                let more = b.data.result_len.saturating_sub(result.len());
                let text = if more > 0 { format!("{result}\n… {more} more characters") } else { result.clone() };
                column = column.child(mono_box(text, theme));
            }
        } else {
            drop(b);
            let md = markdown_of(e);
            column = column.child(div().text_size(px(12.)).py(px(4.)).child(markdown_view(&md, theme)));
        }
    }
    column.into_any_element()
}
