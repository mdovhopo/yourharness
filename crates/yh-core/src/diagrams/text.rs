//! Text for diagrams: measuring and wrapping labels, cleaning Mermaid's label syntax, splitting
//! a source into statements, and the colors and styles `classDef` and `style` give.

use super::Size;
use std::fmt;

/// How the renderer measures text. Layout lives here, fonts live in the app, so the app hands
/// in a width function for its font and the line height that goes with it.
pub struct TextMetrics {
    pub line_height: f64,
    pub width: Box<dyn Fn(&str) -> f64 + Send + Sync>,
}

impl TextMetrics {
    pub fn new(line_height: f64, width: impl Fn(&str) -> f64 + Send + Sync + 'static) -> Self {
        TextMetrics { line_height, width: Box::new(width) }
    }

    /// Fixed-width stand-in for tests: every character is `char_width` wide.
    pub fn monospaced(char_width: f64, line_height: f64) -> Self {
        TextMetrics::new(line_height, move |text| text.chars().count() as f64 * char_width)
    }

    pub fn width(&self, text: &str) -> f64 {
        (self.width)(text)
    }
}

impl fmt::Debug for TextMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextMetrics").field("line_height", &self.line_height).finish()
    }
}

/// Text broken into lines that fit a width, plus the size it takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WrappedText {
    pub lines: Vec<String>,
    pub size: Size,
}

impl WrappedText {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Greedy word wrap. Explicit line breaks are kept; a single word longer than the width
    /// gets its own line rather than being split.
    pub fn wrap(text: &str, max_width: f64, metrics: &TextMetrics) -> WrappedText {
        let text = text.trim();
        if text.is_empty() {
            return WrappedText::default();
        }
        let mut lines: Vec<String> = Vec::new();
        for paragraph in text.split('\n') {
            let mut current = String::new();
            for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
                let candidate =
                    if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
                if current.is_empty() || metrics.width(&candidate) <= max_width {
                    current = candidate;
                } else {
                    lines.push(std::mem::replace(&mut current, word.to_string()));
                }
            }
            lines.push(current);
        }
        let width = lines.iter().map(|line| metrics.width(line)).fold(0.0, f64::max);
        let height = lines.len() as f64 * metrics.line_height;
        WrappedText { lines, size: Size::new(width, height) }
    }
}

/// Why a diagram couldn't be read, in words fit to show the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidError {
    pub message: String,
}

impl MermaidError {
    pub fn new(message: impl Into<String>) -> Self {
        MermaidError { message: message.into() }
    }
}

impl fmt::Display for MermaidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MermaidError {}

/// Spaces and tabs, not line breaks.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() && c != '\n' && c != '\r'
}

pub(crate) fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// Lines, split at any line break.
pub(crate) fn lines(source: &str) -> impl Iterator<Item = &str> {
    source.split(['\n', '\r', '\u{2028}', '\u{2029}', '\u{85}', '\u{0B}', '\u{0C}'])
}

/// Whether `text` starts with the ASCII `prefix`, ignoring case. Slicing at `prefix.len()`
/// is then safe.
pub(crate) fn has_prefix_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len() && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Mermaid label text to plain text: quotes, `<br>`, a few entities and simple tags.
pub(crate) fn clean(raw: &str) -> String {
    let mut text = trim(raw);
    if text.chars().count() >= 2 && text.starts_with('"') && text.ends_with('"') {
        text = &text[1..text.len() - 1];
    }
    if text.chars().count() >= 2 && text.starts_with('`') && text.ends_with('`') {
        text = &text[1..text.len() - 1];
    }
    let mut text = strip_simple_tags(&replace_breaks(text));
    for (entity, value) in [
        ("#quot;", "\""),
        ("&quot;", "\""),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("#35;", "#"),
        ("#59;", ";"),
        ("&nbsp;", " "),
    ] {
        if text.contains(entity) {
            text = text.replace(entity, value);
        }
    }
    // Mermaid markdown strings: drop emphasis markers rather than show them.
    let text = text.replace("**", "");
    trim(&text).to_string()
}

/// `<br>`, `<br/>`, `<BR />` to a line break.
fn replace_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        if has_prefix_ci(tail, "br") {
            let after = tail[2..].trim_start_matches(char::is_whitespace);
            let after = after.strip_prefix('/').unwrap_or(after);
            if let Some(after) = after.strip_prefix('>') {
                out.push('\n');
                rest = after;
                continue;
            }
        }
        out.push('<');
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Drops `<b>`, `</i>`, `<span …>` and the like, keeping what's between them.
fn strip_simple_tags(text: &str) -> String {
    const NAMES: [&str; 8] = ["b", "i", "em", "strong", "u", "code", "small", "span"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let matches = |name_start: &str| {
            NAMES.iter().any(|name| has_prefix_ci(name_start, name)) && name_start.contains('>')
        };
        let name_start = if matches(tail) {
            Some(tail)
        } else {
            tail.strip_prefix('/').filter(|after| matches(after))
        };
        if let Some(start) = name_start {
            // Up to and including the first `>`.
            rest = start.find('>').map_or("", |close| &start[close + 1..]);
            continue;
        }
        out.push('<');
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Statements, without comments and blank lines. Semicolons separate statements too, except
/// inside quotes and brackets.
pub(crate) fn statements(source: &str) -> Vec<String> {
    let mut result = Vec::new();
    for raw in lines(source) {
        let line = match raw.find("%%") {
            Some(comment) => &raw[..comment],
            None => raw,
        };
        let mut current = String::new();
        let mut depth = 0usize;
        let mut in_quote = false;
        for c in line.chars() {
            match c {
                '"' => in_quote = !in_quote,
                '[' | '(' | '{' if !in_quote => depth += 1,
                ']' | ')' | '}' if !in_quote => depth = depth.saturating_sub(1),
                _ => {}
            }
            if c == ';' && depth == 0 && !in_quote {
                result.push(std::mem::take(&mut current));
            } else {
                current.push(c);
            }
        }
        result.push(current);
    }
    result
        .into_iter()
        .map(|statement| trim(&statement).to_string())
        .filter(|statement| !statement.is_empty())
        .collect()
}

/// An sRGB color from Mermaid styling (`#f96`, `#ff9966cc`, `rgb(…)`, `red`), each part 0…1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagramColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl DiagramColor {
    pub const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        DiagramColor { red, green, blue, alpha: 1.0 }
    }

    /// Named colors are softened to read well on light and dark backgrounds.
    fn named(name: &str) -> Option<DiagramColor> {
        let (r, g, b) = match name {
            "red" => (0.90, 0.28, 0.30),
            "orange" => (0.96, 0.58, 0.20),
            "yellow" => (0.95, 0.80, 0.20),
            "green" => (0.20, 0.70, 0.40),
            "blue" => (0.25, 0.50, 0.95),
            "purple" => (0.60, 0.40, 0.90),
            "pink" => (0.95, 0.45, 0.65),
            "teal" => (0.20, 0.65, 0.70),
            "gray" | "grey" => (0.55, 0.55, 0.58),
            "black" => (0.0, 0.0, 0.0),
            "white" => (1.0, 1.0, 1.0),
            _ => return None,
        };
        Some(DiagramColor::rgb(r, g, b))
    }

    pub fn parse(raw: &str) -> Option<DiagramColor> {
        let text = trim(raw).to_lowercase();
        if let Some(color) = Self::named(&text) {
            return Some(color);
        }
        if let Some(hex) = text.strip_prefix('#') {
            if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let hex: String = if hex.len() == 3 || hex.len() == 4 {
                hex.chars().flat_map(|c| [c, c]).collect()
            } else {
                hex.to_string()
            };
            if hex.len() != 6 && hex.len() != 8 {
                return None;
            }
            let value = u64::from_str_radix(&hex, 16).ok()?;
            let has_alpha = hex.len() == 8;
            let shift = if has_alpha { 8 } else { 0 };
            let part = |offset: u32| ((value >> offset) & 0xFF) as f64 / 255.0;
            return Some(DiagramColor {
                red: part(16 + shift),
                green: part(8 + shift),
                blue: part(shift),
                alpha: if has_alpha { part(0) } else { 1.0 },
            });
        }
        if text.starts_with("rgb")
            && let (Some(open), Some(close)) = (text.find('('), text.rfind(')'))
            && open < close
        {
            let parts: Vec<f64> = text[open + 1..close]
                .split(',')
                .filter_map(|part| trim(part).parse::<f64>().ok())
                .collect();
            if parts.len() < 3 {
                return None;
            }
            return Some(DiagramColor {
                red: parts[0] / 255.0,
                green: parts[1] / 255.0,
                blue: parts[2] / 255.0,
                alpha: parts.get(3).copied().unwrap_or(1.0),
            });
        }
        None
    }
}

/// The subset of Mermaid node styling we honor: fill, outline color, dashes and width. Text
/// color is ignored on purpose so labels stay readable in light and dark mode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShapeStyle {
    pub fill: Option<DiagramColor>,
    pub stroke: Option<DiagramColor>,
    pub dashed: bool,
    pub stroke_width: Option<f64>,
}

impl ShapeStyle {
    pub fn is_empty(&self) -> bool {
        *self == ShapeStyle::default()
    }

    /// `fill:#f96,stroke:#333,stroke-width:2px,stroke-dasharray: 5 5`
    pub fn parse(declarations: &str) -> ShapeStyle {
        let mut style = ShapeStyle::default();
        // Commas separate declarations, except inside `rgb(…)`.
        let mut parts = vec![String::new()];
        let mut depth = 0usize;
        for c in declarations.chars() {
            if c == '(' {
                depth += 1;
            }
            if c == ')' {
                depth = depth.saturating_sub(1);
            }
            if c == ',' && depth == 0 {
                parts.push(String::new());
            } else if let Some(last) = parts.last_mut() {
                last.push(c);
            }
        }
        for declaration in &parts {
            let Some((key, value)) = declaration.split_once(':') else { continue };
            if key.is_empty() || value.is_empty() {
                continue;
            }
            let value = trim(value);
            match trim(key).to_lowercase().as_str() {
                "fill" => style.fill = DiagramColor::parse(value),
                "stroke" => style.stroke = DiagramColor::parse(value),
                "stroke-width" => {
                    style.stroke_width = trim(&value.to_lowercase().replace("px", "")).parse().ok()
                }
                "stroke-dasharray" => {
                    style.dashed = !matches!(value.to_lowercase().as_str(), "0" | "none" | "")
                }
                _ => {}
            }
        }
        style
    }

    /// `other` wins where it says something.
    pub fn merged(&self, other: &ShapeStyle) -> ShapeStyle {
        let mut result = *self;
        if other.fill.is_some() {
            result.fill = other.fill;
        }
        if other.stroke.is_some() {
            result.stroke = other.stroke;
        }
        if other.dashed {
            result.dashed = true;
        }
        if other.stroke_width.is_some() {
            result.stroke_width = other.stroke_width;
        }
        result
    }
}
