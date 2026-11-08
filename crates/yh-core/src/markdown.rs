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
    /// A link inside a code span: a path named in code.
    CodeLink,
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
    /// Links: where they are in `text`, and where they go.
    pub links: Vec<(Range<usize>, String)>,
}

#[derive(Clone, Debug)]
pub enum MdBlock {
    Paragraph(Styled),
    Heading(u8, Styled),
    /// A fenced or indented block: its language (`rust`, `mermaid`, or empty) and its text.
    Code { lang: String, code: Styled },
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
    /// Links open: where each starts and where it goes.
    links: Vec<(usize, String)>,
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

impl Markdown {
    /// The text as it reads aloud, without the markup: a block a line, list items and table
    /// rows on lines of their own (for VoiceOver).
    pub fn plain_text(&self) -> String {
        let mut out: Vec<String> = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            match block {
                MdBlock::Paragraph(s) | MdBlock::Heading(_, s) | MdBlock::Quote(s) => out.push(s.text.clone()),
                MdBlock::Code { code, .. } => out.push(code.text.clone()),
                MdBlock::List(items) => out.extend(items.iter().map(|(_, _, s)| s.text.clone())),
                MdBlock::Table { header, rows } => {
                    for row in std::iter::once(header).chain(rows) {
                        out.push(row.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(", "));
                    }
                }
                MdBlock::Rule => {}
            }
        }
        out.join("\n")
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
                Tag::Link { dest_url, .. } => {
                    b.open.push((Span::Link, b.current.text.len()));
                    b.links.push((b.current.text.len(), dest_url.to_string()));
                }
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
                    b.blocks.push(MdBlock::Code { code: highlight(&text, &lang), lang: lang.trim().to_string() });
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
                TagEnd::Link => {
                    if let Some((start, url)) = b.links.pop() {
                        let end = b.current.text.len();
                        if end > start {
                            b.current.links.push((start..end, url));
                        }
                    }
                    if let Some((span, start)) = b.open.pop() {
                        let end = b.current.text.len();
                        if end > start {
                            b.current.spans.push((start..end, span));
                        }
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
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
        MdBlock::Paragraph(s) | MdBlock::Heading(_, s) | MdBlock::Code { code: s, .. } | MdBlock::Quote(s) => vec![s],
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

/// A table as tab-separated values, a row to a line, for pasting into a spreadsheet.
pub fn table_tsv(header: &[Styled], rows: &[Vec<Styled>]) -> String {
    let line = |cells: &[Styled]| cells.iter().map(|c| c.text.replace(['\t', '\n'], " ")).collect::<Vec<_>>().join("\t");
    std::iter::once(line(header)).chain(rows.iter().map(|r| line(r))).collect::<Vec<_>>().join("\n")
}

const KEYWORDS: &[&str] = &[
    "let", "var", "func", "fn", "if", "else", "for", "while", "return", "struct", "class", "enum", "impl", "pub", "use",
    "import", "const", "static", "match", "case", "switch", "guard", "in", "of", "self", "Self", "true", "false", "nil",
    "null", "None", "Some", "async", "await", "try", "def", "function", "export", "type", "interface", "mut", "where",
    "protocol", "extension", "private", "public", "final", "override", "new", "break", "continue", "do", "from", "as",
];

/// What a line of code starts inside, carried over from the line before: a block comment or a
/// string that runs on. A file's viewer colours line by line, so a `/* … */` or `"""…"""`
/// over several lines is coloured whole, as a highlighter that sees the
/// whole text would.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Carry {
    #[default]
    None,
    /// Inside `/* … */` (or `<!-- … -->` in markup).
    Comment,
    /// Inside a string that can span lines: its quote, and whether it's tripled.
    Str(u8, bool),
}

/// What a language's code can carry from one line to the next.
#[derive(Clone, Copy, Default)]
struct Multiline {
    /// `/* … */`.
    block_comments: bool,
    /// `<!-- … -->`.
    markup_comments: bool,
    /// `"""…"""` and `'''…'''`.
    triple_quotes: bool,
    /// `` `…` `` over lines (JavaScript, Go).
    backticks: bool,
    /// `"…"` over lines (Rust, C's continued lines aside).
    double_quotes: bool,
}

fn multiline(lang: &str) -> Multiline {
    let name = lang.split(|c: char| c == ' ' || c == '{' || c == ',').next().unwrap_or("").to_ascii_lowercase();
    let c_like = |m: Multiline| Multiline { block_comments: true, ..m };
    match name.as_str() {
        "rs" | "rust" => c_like(Multiline { double_quotes: true, ..Default::default() }),
        "swift" | "kt" | "kts" | "kotlin" => c_like(Multiline { triple_quotes: true, ..Default::default() }),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "javascript" | "typescript" | "go" | "golang" => c_like(Multiline { backticks: true, ..Default::default() }),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" | "objc" | "java" | "cs" | "csharp" | "css" | "scss" | "less" | "php" | "scala" | "dart" | "sql" => {
            c_like(Multiline::default())
        }
        "py" | "pyi" | "python" | "python3" | "toml" => Multiline { triple_quotes: true, ..Default::default() },
        "html" | "htm" | "xml" | "svg" | "plist" | "vue" | "svelte" | "xhtml" | "md" | "markdown" => Multiline { markup_comments: true, ..Default::default() },
        _ => Multiline::default(),
    }
}

/// A light pass that colours keywords, strings, comments and numbers: enough to read code by.
pub fn highlight(text: &str, lang: &str) -> Styled {
    highlight_line(text, lang, Carry::None).0
}

/// What each line of `lines` starts inside, for colouring them one at a time with
/// `highlight_line`. One pass over the text: run it off the main thread for a big file.
pub fn line_carries<S: AsRef<str>>(lines: &[S], lang: &str) -> Vec<Carry> {
    let mut out = Vec::with_capacity(lines.len());
    let mut carry = Carry::None;
    let m = multiline(lang);
    for line in lines {
        out.push(carry);
        carry = scan(line.as_ref(), m, carry, &mut |_, _| {});
    }
    out
}

/// One line (or a whole text) coloured, starting inside `carry`, and what the next line starts in.
pub fn highlight_line(text: &str, lang: &str, carry: Carry) -> (Styled, Carry) {
    let mut spans = vec![];
    let end = scan(text, multiline(lang), carry, &mut |r, s| spans.push((r, s)));
    // Ranges must fall on character boundaries.
    spans.retain(|(r, _): &(Range<usize>, Span)| text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
    (Styled { text: text.to_string(), spans, links: vec![] }, end)
}

/// The scanner `highlight_line` and `line_carries` share: calls `span` for each coloured run.
fn scan(text: &str, m: Multiline, carry: Carry, span: &mut dyn FnMut(Range<usize>, Span)) -> Carry {
    let bytes = text.as_bytes();
    let find = |from: usize, needle: &[u8]| -> Option<usize> { bytes[from.min(bytes.len())..].windows(needle.len()).position(|w| w == needle).map(|p| from + p) };
    // The end of a string opened with `quote` (tripled or not), from `from`: past its closing
    // quote, or None when it runs past the text.
    let string_end = |from: usize, quote: u8, triple: bool, lines: bool| -> Option<usize> {
        let mut j = from;
        while j < bytes.len() {
            let b = bytes[j];
            if b == b'\\' {
                j += 2;
                continue;
            }
            if b == quote && (!triple || bytes.get(j + 1) == Some(&quote) && bytes.get(j + 2) == Some(&quote)) {
                return Some(j + if triple { 3 } else { 1 });
            }
            if b == b'\n' && !lines {
                return Some(j);
            }
            j += 1;
        }
        None
    };
    let mut i = 0;
    match carry {
        Carry::None => {}
        Carry::Comment => {
            let close: &[u8] = if m.markup_comments { b"-->" } else { b"*/" };
            match find(0, close) {
                Some(p) => {
                    span(0..p + close.len(), Span::Comment);
                    i = p + close.len();
                }
                None => {
                    span(0..bytes.len(), Span::Comment);
                    return Carry::Comment;
                }
            }
        }
        Carry::Str(q, triple) => match string_end(0, q, triple, true) {
            Some(end) => {
                span(0..end, Span::Str);
                i = end;
            }
            None => {
                span(0..bytes.len(), Span::Str);
                return carry;
            }
        },
    }
    while i < bytes.len() {
        let c = bytes[i];
        if m.block_comments && c == b'/' && bytes.get(i + 1) == Some(&b'*') || m.markup_comments && bytes[i..].starts_with(b"<!--") {
            let (open, close): (usize, &[u8]) = if c == b'/' { (2, b"*/") } else { (4, b"-->") };
            match find(i + open, close) {
                Some(p) => {
                    span(i..p + close.len(), Span::Comment);
                    i = p + close.len();
                }
                None => {
                    span(i..bytes.len(), Span::Comment);
                    return Carry::Comment;
                }
            }
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'/') || c == b'#' && (i == 0 || bytes[i - 1] == b'\n' || bytes[i - 1] == b' ') {
            let end = text[i..].find('\n').map_or(text.len(), |n| i + n);
            span(i..end, Span::Comment);
            i = end;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let triple = m.triple_quotes && c != b'`' && bytes.get(i + 1) == Some(&c) && bytes.get(i + 2) == Some(&c);
            let lines = triple || c == b'`' && m.backticks || c == b'"' && m.double_quotes;
            match string_end(i + if triple { 3 } else { 1 }, c, triple, lines) {
                Some(end) => {
                    let end = end.min(bytes.len());
                    span(i..end, Span::Str);
                    i = end;
                }
                None => {
                    span(i..bytes.len(), Span::Str);
                    return if lines { Carry::Str(c, triple) } else { Carry::None };
                }
            }
        } else if c.is_ascii_digit() && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_') {
                j += 1;
            }
            span(i..j, Span::Number);
            i = j;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if KEYWORDS.contains(&&text[i..j]) {
                span(i..j, Span::Keyword);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Carry::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_the_markup() {
        let md = parse("# Done\n\nIt **builds** now, see `main.rs`.\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---");
        assert_eq!(md.plain_text(), "Done\nIt builds now, see main.rs.\none\ntwo\na, b\n1, 2");
    }

    #[test]
    fn code_blocks_keep_their_language() {
        let md = parse("```mermaid\nflowchart TD\n  A --> B\n```\n\n    indented");
        assert!(matches!(&md.blocks[0], MdBlock::Code { lang, code } if lang == "mermaid" && code.text == "flowchart TD\n  A --> B"));
        assert!(matches!(&md.blocks[1], MdBlock::Code { lang, .. } if lang.is_empty()));
    }

    #[test]
    fn comments_and_strings_over_lines_colour_every_line() {
        let lines = ["let a = 1; /* one", "two", "three */ let b = 2;", "s = \"\"\"doc", "more\"\"\" + x"];
        let carries = line_carries(&lines, "rs");
        assert_eq!(carries[..3], [Carry::None, Carry::Comment, Carry::Comment]);
        let (two, after) = highlight_line(lines[1], "rs", carries[1]);
        assert_eq!(two.spans, vec![(0..3, Span::Comment)]);
        assert_eq!(after, Carry::Comment);
        let (three, after) = highlight_line(lines[2], "rs", carries[2]);
        assert_eq!(three.spans[0], (0..8, Span::Comment));
        assert!(three.spans.iter().any(|(r, s)| *s == Span::Keyword && &lines[2][r.clone()] == "let"));
        assert_eq!(after, Carry::None);
        // Python's triple quotes; Rust's don't run on as triples.
        let py = line_carries(&lines[3..], "py");
        assert_eq!(py[1], Carry::Str(b'"', true));
        assert_eq!(highlight_line(lines[4], "py", py[1]).0.spans[0], (0..7, Span::Str));
        // Unknown languages keep to one line, as before.
        assert_eq!(line_carries(&lines, ""), vec![Carry::None; 5]);
        assert_eq!(highlight("a /* b", "").spans, vec![]);
    }

    #[test]
    fn tables_copy_as_tab_separated() {
        let md = parse("| a | b |\n|---|---|\n| 1 | **2** |\n| x | y z |");
        let MdBlock::Table { header, rows } = &md.blocks[0] else { panic!() };
        assert_eq!(table_tsv(header, rows), "a\tb\n1\t2\nx\ty z");
    }
}
