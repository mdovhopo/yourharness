//! Markdown into blocks of styled text, parsed once per version of an entry. Inline styles are
//! highlight ranges over one string per paragraph, so a paragraph is one text element.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Bold,
    Italic,
    Code,
    Link,
    Strike,
    // Code highlighting.
    Keyword,
    Str,
    Comment,
    Number,
}

#[derive(Clone, Debug, Default)]
pub struct Styled {
    pub text: String,
    pub spans: Vec<(Range<usize>, Span)>,
}

#[derive(Clone, Debug)]
pub enum MdBlock {
    Paragraph(Styled),
    Heading(u8, Styled),
    Code(Styled),
    /// Items with their nesting depth and number (none: a bullet).
    List(Vec<(usize, Option<u64>, Styled)>),
    Quote(Styled),
    Table { header: Vec<Styled>, rows: Vec<Vec<Styled>> },
    Rule,
}

pub struct Markdown {
    pub blocks: Vec<MdBlock>,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<MdBlock>,
    current: Styled,
    open: Vec<(Span, usize)>,
    list: Vec<(usize, Option<u64>, Styled)>,
    list_stack: Vec<Option<u64>>,
    in_item: bool,
    heading: Option<u8>,
    quote: usize,
    code: Option<String>,
    table: Option<(Vec<Styled>, Vec<Vec<Styled>>, bool)>,
    row: Vec<Styled>,
}

impl Builder {
    fn take(&mut self) -> Styled {
        std::mem::take(&mut self.current)
    }

    fn flush_paragraph(&mut self) {
        if self.current.text.trim().is_empty() {
            self.current = Styled::default();
            return;
        }
        let styled = self.take();
        if self.in_item {
            let depth = self.list_stack.len().saturating_sub(1);
            let number = self.list_stack.last().copied().flatten();
            self.list.push((depth, number, styled));
            if let Some(Some(n)) = self.list_stack.last_mut() {
                *n += 1;
            }
            self.in_item = false;
        } else if self.quote > 0 {
            self.blocks.push(MdBlock::Quote(styled));
        } else if let Some(level) = self.heading {
            self.blocks.push(MdBlock::Heading(level, styled));
        } else {
            self.blocks.push(MdBlock::Paragraph(styled));
        }
    }

    fn text(&mut self, text: &str) {
        self.current.text.push_str(text);
    }
}

pub fn parse(source: &str) -> Markdown {
    let mut b = Builder::default();
    let parser = Parser::new_ext(source, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH);
    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => b.heading = Some(level as u8),
                Tag::BlockQuote(_) => b.quote += 1,
                Tag::CodeBlock(kind) => {
                    b.flush_paragraph();
                    b.code = Some(match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    });
                }
                Tag::List(start) => {
                    if b.in_item {
                        b.flush_paragraph();
                    }
                    b.list_stack.push(start);
                }
                Tag::Item => {
                    if b.in_item {
                        b.flush_paragraph();
                    }
                    b.in_item = true;
                }
                Tag::Table(_) => b.table = Some((vec![], vec![], false)),
                Tag::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.2 = true;
                    }
                }
                Tag::TableRow => b.row.clear(),
                Tag::TableCell => b.current = Styled::default(),
                Tag::Emphasis => b.open.push((Span::Italic, b.current.text.len())),
                Tag::Strong => b.open.push((Span::Bold, b.current.text.len())),
                Tag::Strikethrough => b.open.push((Span::Strike, b.current.text.len())),
                Tag::Link { .. } => b.open.push((Span::Link, b.current.text.len())),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => b.flush_paragraph(),
                TagEnd::Heading(_) => {
                    b.flush_paragraph();
                    b.heading = None;
                }
                TagEnd::BlockQuote(_) => {
                    b.flush_paragraph();
                    b.quote = b.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    let lang = b.code.take().unwrap_or_default();
                    let mut text = b.take().text;
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    b.blocks.push(MdBlock::Code(highlight(&text, &lang)));
                }
                TagEnd::Item => b.flush_paragraph(),
                TagEnd::List(_) => {
                    b.flush_paragraph();
                    b.list_stack.pop();
                    if b.list_stack.is_empty() && !b.list.is_empty() {
                        let list = std::mem::take(&mut b.list);
                        b.blocks.push(MdBlock::List(list));
                    }
                }
                TagEnd::TableCell => {
                    let cell = b.take();
                    b.row.push(cell);
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.0 = std::mem::take(&mut b.row);
                        t.2 = false;
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut b.table {
                        t.1.push(std::mem::take(&mut b.row));
                    }
                }
                TagEnd::Table => {
                    if let Some((header, rows, _)) = b.table.take() {
                        b.blocks.push(MdBlock::Table { header, rows });
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    if let Some((span, start)) = b.open.pop() {
                        let end = b.current.text.len();
                        if end > start {
                            b.current.spans.push((start..end, span));
                        }
                    }
                }
                _ => {}
            },
            Event::Text(text) => b.text(&text),
            Event::Code(code) => {
                let start = b.current.text.len();
                b.text(&code);
                let end = b.current.text.len();
                b.current.spans.push((start..end, Span::Code));
            }
            Event::SoftBreak => b.text(" "),
            Event::HardBreak => b.text("\n"),
            Event::Rule => {
                b.flush_paragraph();
                b.blocks.push(MdBlock::Rule);
            }
            Event::Html(html) | Event::InlineHtml(html) => b.text(&html),
            _ => {}
        }
    }
    b.flush_paragraph();
    if !b.list.is_empty() {
        let list = std::mem::take(&mut b.list);
        b.blocks.push(MdBlock::List(list));
    }
    // Spans must be sorted and not overlap for a text element: nested styles keep the inner.
    for block in &mut b.blocks {
        for styled in styled_mut(block) {
            normalize(&mut styled.spans);
        }
    }
    Markdown { blocks: b.blocks }
}

fn styled_mut(block: &mut MdBlock) -> Vec<&mut Styled> {
    match block {
        MdBlock::Paragraph(s) | MdBlock::Heading(_, s) | MdBlock::Code(s) | MdBlock::Quote(s) => vec![s],
        MdBlock::List(items) => items.iter_mut().map(|(_, _, s)| s).collect(),
        MdBlock::Table { header, rows } => header.iter_mut().chain(rows.iter_mut().flatten()).collect(),
        MdBlock::Rule => vec![],
    }
}

fn normalize(spans: &mut Vec<(Range<usize>, Span)>) {
    spans.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let mut out: Vec<(Range<usize>, Span)> = vec![];
    for (range, span) in spans.drain(..) {
        if let Some((last, _)) = out.last() {
            if range.start < last.end {
                continue;
            }
        }
        out.push((range, span));
    }
    *spans = out;
}

const KEYWORDS: &[&str] = &[
    "let", "var", "func", "fn", "if", "else", "for", "while", "return", "struct", "class", "enum", "impl", "pub", "use",
    "import", "const", "static", "match", "case", "switch", "guard", "in", "of", "self", "Self", "true", "false", "nil",
    "null", "None", "Some", "async", "await", "try", "def", "function", "export", "type", "interface", "mut", "where",
    "protocol", "extension", "private", "public", "final", "override", "new", "break", "continue", "do", "from", "as",
];

/// A light pass that colours keywords, strings, comments and numbers: enough to read code by.
pub fn highlight(text: &str, _lang: &str) -> Styled {
    let mut spans = vec![];
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') || c == b'#' && (i == 0 || bytes[i - 1] == b'\n' || bytes[i - 1] == b' ') {
            let end = text[i..].find('\n').map_or(text.len(), |n| i + n);
            spans.push((i..end, Span::Comment));
            i = end;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c && bytes[j] != b'\n' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(bytes.len());
            spans.push((i..end, Span::Str));
            i = end;
        } else if c.is_ascii_digit() && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_') {
                j += 1;
            }
            spans.push((i..j, Span::Number));
            i = j;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if KEYWORDS.contains(&&text[i..j]) {
                spans.push((i..j, Span::Keyword));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // Ranges must fall on character boundaries.
    spans.retain(|(r, _): &(Range<usize>, Span)| text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    Styled { text: text.to_string(), spans }
}
