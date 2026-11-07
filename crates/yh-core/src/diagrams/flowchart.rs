//! A Mermaid `flowchart` / `graph`: nodes, edges and subgraphs. Node and subgraph styling
//! (`classDef`, `class`, `:::name`, `style`) is honored for fill, outline and dashes;
//! `linkStyle` and `click` are accepted and ignored.

use super::text::{self, MermaidError, ShapeStyle, clean, has_prefix_ci, trim};
use std::collections::HashMap;

/// Which way the flow runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl Direction {
    /// Whether the flow runs up and down.
    pub fn is_vertical(self) -> bool {
        matches!(self, Direction::TopBottom | Direction::BottomTop)
    }
}

/// A node's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// `[text]`
    Rectangle,
    /// `(text)`
    Rounded,
    /// `([text])`
    Stadium,
    /// `[[text]]`
    Subroutine,
    /// `[(text)]`
    Cylinder,
    /// `((text))`
    Circle,
    /// `(((text)))`
    DoubleCircle,
    /// `{text}`
    Diamond,
    /// `{{text}}`
    Hexagon,
    /// `[/text/]`
    Parallelogram,
    /// `[\text\]`
    ParallelogramAlt,
    /// `[/text\]`
    Trapezoid,
    /// `[\text/]`
    TrapezoidAlt,
    /// `>text]`
    Flag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    /// `classDef default`, then its classes in order, then `style`, merged.
    pub style: ShapeStyle,
}

/// How an edge's line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Line {
    /// `--`
    Solid,
    /// `-.-`
    Dotted,
    /// `==`
    Thick,
    /// `~~~`: takes part in the layout but isn't drawn.
    Invisible,
}

/// What an edge's end looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arrow {
    None,
    Arrow,
    Circle,
    Cross,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub line: Line,
    /// At `from`.
    pub start: Arrow,
    /// At `to`.
    pub end: Arrow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subgraph {
    pub id: String,
    pub title: String,
    /// Nodes declared directly inside it, not in subgraphs within.
    pub node_ids: Vec<String>,
    pub parent: Option<String>,
    /// Only classes and `style` aimed at the subgraph itself; `default` is for nodes.
    pub style: ShapeStyle,
    /// Its own `direction`, used when no edge crosses its border.
    pub direction: Option<Direction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flowchart {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub subgraphs: Vec<Subgraph>,
}

pub(crate) fn parse(source: &str) -> Result<Flowchart, MermaidError> {
    let statements = text::statements(source);
    let Some(header) = statements.first() else {
        return Err(MermaidError::new("The diagram is empty."));
    };
    let mut words = header.split(' ').filter(|word| !word.is_empty());
    let keyword = words.next();
    if keyword != Some("flowchart") && keyword != Some("graph") {
        return Err(MermaidError::new("Expected `flowchart` or `graph`."));
    }
    let mut builder = Builder::new(direction(words.next()));
    for statement in &statements[1..] {
        builder.add(statement)?;
    }
    if !builder.subgraph_stack.is_empty() {
        return Err(MermaidError::new("A `subgraph` is missing its `end`."));
    }
    if builder.nodes.is_empty() {
        return Err(MermaidError::new("The flowchart has no nodes."));
    }
    Ok(builder.flowchart())
}

fn direction(token: Option<&str>) -> Direction {
    match token.map(|token| token.to_uppercase()).as_deref() {
        Some("LR") => Direction::LeftRight,
        Some("RL") => Direction::RightLeft,
        Some("BT") => Direction::BottomTop,
        _ => Direction::TopBottom,
    }
}

fn unreadable(statement: &str) -> MermaidError {
    MermaidError::new(format!("Couldn't read “{statement}”."))
}

// Building

struct Builder {
    direction: Direction,
    nodes: Vec<Node>,
    node_index: HashMap<String, usize>,
    edges: Vec<Edge>,
    subgraphs: Vec<Subgraph>,
    subgraph_stack: Vec<usize>,
    class_definitions: HashMap<String, ShapeStyle>,
    classes: HashMap<String, Vec<String>>,
    inline_styles: HashMap<String, ShapeStyle>,
}

impl Builder {
    fn new(direction: Direction) -> Self {
        Builder {
            direction,
            nodes: Vec::new(),
            node_index: HashMap::new(),
            edges: Vec::new(),
            subgraphs: Vec::new(),
            subgraph_stack: Vec::new(),
            class_definitions: HashMap::new(),
            classes: HashMap::new(),
            inline_styles: HashMap::new(),
        }
    }

    fn classes_style(&self, id: &str, base: ShapeStyle) -> ShapeStyle {
        let mut style = base;
        for name in self.classes.get(id).into_iter().flatten() {
            if let Some(definition) = self.class_definitions.get(name) {
                style = style.merged(definition);
            }
        }
        style.merged(&self.inline_styles.get(id).copied().unwrap_or_default())
    }

    fn flowchart(self) -> Flowchart {
        // An edge to a subgraph lands on its first node: close to Mermaid's look, and keeps
        // the layout a plain graph.
        let mut subgraph_targets: HashMap<&str, &str> = HashMap::new();
        for subgraph in &self.subgraphs {
            if let Some(first) = subgraph.node_ids.first() {
                subgraph_targets.entry(subgraph.id.as_str()).or_insert(first.as_str());
            }
        }
        let resolve = |id: &String| -> String {
            if self.node_index.contains_key(id) {
                id.clone()
            } else {
                subgraph_targets.get(id.as_str()).map_or_else(|| id.clone(), |t| t.to_string())
            }
        };
        let edges = self
            .edges
            .iter()
            .filter_map(|edge| {
                let mut edge = edge.clone();
                edge.from = resolve(&edge.from);
                edge.to = resolve(&edge.to);
                (self.node_index.contains_key(&edge.from) && self.node_index.contains_key(&edge.to))
                    .then_some(edge)
            })
            .collect();
        let default = self.class_definitions.get("default").copied().unwrap_or_default();
        let nodes = self
            .nodes
            .iter()
            .map(|node| Node { style: self.classes_style(&node.id, default), ..node.clone() })
            .collect();
        let subgraphs = self
            .subgraphs
            .iter()
            .map(|subgraph| Subgraph {
                style: self.classes_style(&subgraph.id, ShapeStyle::default()),
                ..subgraph.clone()
            })
            .collect();
        Flowchart { direction: self.direction, nodes, edges, subgraphs }
    }

    fn add(&mut self, statement: &str) -> Result<(), MermaidError> {
        if has_prefix_ci(statement, "classdef ") {
            let rest = trim(&statement["classdef ".len()..]);
            let Some(space) = rest.find(' ') else { return Ok(()) };
            let style = ShapeStyle::parse(&rest[space..]);
            for name in rest[..space].split(',').filter(|name| !name.is_empty()) {
                self.class_definitions.insert(name.to_string(), style);
            }
            return Ok(());
        }
        if has_prefix_ci(statement, "class ") {
            let parts: Vec<&str> =
                statement["class ".len()..].split(' ').filter(|part| !part.is_empty()).collect();
            if parts.len() < 2 {
                return Ok(());
            }
            for id in parts[0].split(',').filter(|id| !id.is_empty()) {
                self.classes.entry(id.to_string()).or_default().push(parts[1].to_string());
            }
            return Ok(());
        }
        if has_prefix_ci(statement, "style ") {
            let rest = trim(&statement["style ".len()..]);
            let Some(space) = rest.find(' ') else { return Ok(()) };
            let style = ShapeStyle::parse(&rest[space..]);
            let entry = self.inline_styles.entry(rest[..space].to_string()).or_default();
            *entry = entry.merged(&style);
            return Ok(());
        }
        if ["linkstyle ", "click ", "acctitle", "accdescr"]
            .iter()
            .any(|ignored| has_prefix_ci(statement, ignored))
        {
            return Ok(());
        }
        if has_prefix_ci(statement, "direction ") {
            let value = direction(Some(trim(&statement["direction ".len()..])));
            match self.subgraph_stack.last() {
                Some(&current) => self.subgraphs[current].direction = Some(value),
                None => self.direction = value,
            }
            return Ok(());
        }
        if statement.eq_ignore_ascii_case("end") {
            if self.subgraph_stack.pop().is_none() {
                return Err(MermaidError::new("`end` without a `subgraph`."));
            }
            return Ok(());
        }
        if has_prefix_ci(statement, "subgraph") {
            self.open_subgraph(trim(&statement["subgraph".len()..]));
            return Ok(());
        }
        self.parse_chain(statement)
    }

    fn open_subgraph(&mut self, rest: &str) {
        let mut id;
        let title;
        // `subgraph id [Title]` or `subgraph id["Title"]`
        match rest.find('[') {
            Some(bracket) if rest.ends_with(']') && bracket < rest.len() - 1 => {
                id = trim(&rest[..bracket]).to_string();
                title = clean(&rest[bracket + 1..rest.len() - 1]);
            }
            _ => {
                title = clean(rest);
                id = if rest.contains(' ') || rest.contains('"') {
                    format!("subgraph-{}", self.subgraphs.len())
                } else {
                    rest.to_string()
                };
            }
        }
        if id.is_empty() {
            id = format!("subgraph-{}", self.subgraphs.len());
        }
        let parent = self.subgraph_stack.last().map(|&index| self.subgraphs[index].id.clone());
        self.subgraphs.push(Subgraph {
            id,
            title,
            node_ids: Vec::new(),
            parent,
            style: ShapeStyle::default(),
            direction: None,
        });
        self.subgraph_stack.push(self.subgraphs.len() - 1);
    }

    /// `A[x] --> B & C -->|label| D`
    fn parse_chain(&mut self, statement: &str) -> Result<(), MermaidError> {
        let mut scanner = Scanner::new(statement);
        let mut previous = self.parse_group(&mut scanner);
        if previous.is_empty() {
            return Err(unreadable(statement));
        }
        loop {
            scanner.skip_spaces();
            if scanner.is_at_end() {
                break;
            }
            let Some(link) = scanner.parse_link() else {
                return Err(unreadable(statement));
            };
            let next = self.parse_group(&mut scanner);
            if next.is_empty() {
                return Err(MermaidError::new(format!(
                    "An arrow in “{statement}” points nowhere."
                )));
            }
            for from in &previous {
                for to in &next {
                    self.edges.push(Edge {
                        from: from.clone(),
                        to: to.clone(),
                        label: link.label.clone(),
                        line: link.line,
                        start: link.start,
                        end: link.end,
                    });
                }
            }
            previous = next;
        }
        Ok(())
    }

    /// One or more nodes joined by `&`.
    fn parse_group(&mut self, scanner: &mut Scanner) -> Vec<String> {
        let mut ids = Vec::new();
        loop {
            scanner.skip_spaces();
            let Some(parsed) = scanner.parse_node() else { break };
            if let Some(class_name) = parsed.class_name {
                self.classes.entry(parsed.id.clone()).or_default().push(class_name);
            }
            // A bare subgraph id is an edge to the subgraph, not a new node.
            if parsed.shape.is_some()
                || parsed.label.is_some()
                || self.node_index.contains_key(&parsed.id)
                || !self.subgraphs.iter().any(|subgraph| subgraph.id == parsed.id)
            {
                self.declare(&parsed.id, parsed.shape, parsed.label);
            }
            ids.push(parsed.id);
            scanner.skip_spaces();
            if !scanner.consume("&") {
                break;
            }
        }
        ids
    }

    fn declare(&mut self, id: &str, shape: Option<Shape>, label: Option<String>) {
        if let Some(&index) = self.node_index.get(id) {
            if let Some(shape) = shape {
                self.nodes[index].shape = shape;
            }
            if let Some(label) = label {
                self.nodes[index].label = label;
            }
            return;
        }
        self.node_index.insert(id.to_string(), self.nodes.len());
        self.nodes.push(Node {
            id: id.to_string(),
            label: label.unwrap_or_else(|| id.to_string()),
            shape: shape.unwrap_or(Shape::Rectangle),
            style: ShapeStyle::default(),
        });
        if let Some(&current) = self.subgraph_stack.last()
            && !self.subgraphs.iter().any(|subgraph| subgraph.id == id)
        {
            self.subgraphs[current].node_ids.push(id.to_string());
        }
    }
}

// Scanning

struct Link {
    label: Option<String>,
    line: Line,
    start: Arrow,
    end: Arrow,
}

struct ParsedNode {
    id: String,
    shape: Option<Shape>,
    label: Option<String>,
    class_name: Option<String>,
}

/// Openers, longest first so `([` wins over `(`.
const SHAPES: [(&str, &str, Shape); 14] = [
    ("(((", ")))", Shape::DoubleCircle),
    ("([", "])", Shape::Stadium),
    ("[[", "]]", Shape::Subroutine),
    ("[(", ")]", Shape::Cylinder),
    ("((", "))", Shape::Circle),
    ("{{", "}}", Shape::Hexagon),
    ("[/", "/]", Shape::Parallelogram),
    ("[\\", "\\]", Shape::ParallelogramAlt),
    ("[/", "\\]", Shape::Trapezoid),
    ("[\\", "/]", Shape::TrapezoidAlt),
    ("[", "]", Shape::Rectangle),
    ("(", ")", Shape::Rounded),
    ("{", "}", Shape::Diamond),
    (">", "]", Shape::Flag),
];

struct Scanner {
    chars: Vec<char>,
    index: usize,
}

impl Scanner {
    fn new(text: &str) -> Self {
        Scanner { chars: text.chars().collect(), index: 0 }
    }

    fn is_at_end(&self) -> bool {
        self.index >= self.chars.len()
    }

    fn current(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).copied()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.current(), Some(' ' | '\t')) {
            self.index += 1;
        }
    }

    fn matches_at(&self, at: usize, text: &str) -> bool {
        (at..).zip(text.chars()).all(|(position, c)| self.chars.get(position) == Some(&c))
    }

    fn consume(&mut self, text: &str) -> bool {
        if self.matches_at(self.index, text) {
            self.index += text.chars().count();
            true
        } else {
            false
        }
    }

    fn is_id_char(c: char) -> bool {
        c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$')
    }

    /// A node id, optionally followed by a shape with a label, and `:::class`.
    fn parse_node(&mut self) -> Option<ParsedNode> {
        let start = self.index;
        while let Some(c) = self.current() {
            if Self::is_id_char(c) {
                self.index += 1;
            } else if c == '-'
                && let Some(next) = self.peek(1)
                && Self::is_id_char(next)
                && next != '.'
            {
                // Hyphens inside ids (`my-node`), but not the start of an arrow.
                self.index += 1;
            } else {
                break;
            }
        }
        // `id:::className` names a class without a shape.
        let mut id: String = self.chars[start..self.index].iter().collect();
        let mut class_name = None;
        if let Some(at) = id.find(":::") {
            class_name = Some(id[at + 3..].to_string());
            id.truncate(at);
        }
        if id.is_empty() {
            return None;
        }
        if id.ends_with(':') {
            id.pop();
        }
        let non_empty = |name: Option<String>| name.filter(|name| !name.is_empty());
        let Some((shape, label)) = self.parse_shape() else {
            return Some(ParsedNode { id, shape: None, label: None, class_name: non_empty(class_name) });
        };
        if self.consume(":::") {
            let name_start = self.index;
            while let Some(c) = self.current() {
                if Self::is_id_char(c) || c == '-' {
                    self.index += 1;
                } else {
                    break;
                }
            }
            class_name = Some(self.chars[name_start..self.index].iter().collect());
        }
        Some(ParsedNode {
            id,
            shape: Some(shape),
            label: Some(clean(&label)),
            class_name: non_empty(class_name),
        })
    }

    fn parse_shape(&mut self) -> Option<(Shape, String)> {
        for (open, close, shape) in SHAPES {
            let saved = self.index;
            if !self.consume(open) {
                continue;
            }
            if let Some(label) = self.read_until(close) {
                return Some((shape, label));
            }
            self.index = saved;
        }
        None
    }

    /// Reads to `close`, honoring quotes; leaves the scanner after it.
    fn read_until(&mut self, close: &str) -> Option<String> {
        let close_len = close.chars().count();
        let mut text = String::new();
        let mut in_quote = false;
        while !self.is_at_end() {
            if self.chars[self.index] == '"' {
                in_quote = !in_quote;
            }
            if !in_quote && self.matches_at(self.index, close) {
                // `[/x/]` vs `[/x\]`: the closer must end the shape, not sit inside the text.
                let after = self.index + close_len;
                if after >= self.chars.len()
                    || !"])}/\\".contains(self.chars[after])
                    || close_len > 1
                {
                    self.index = after;
                    return Some(text);
                }
            }
            text.push(self.chars[self.index]);
            self.index += 1;
        }
        None
    }

    /// `-->`, `---`, `-.->`, `==>`, `--o`, `--x`, `<-->`, with `|label|` or inline
    /// `-- label -->` text.
    fn parse_link(&mut self) -> Option<Link> {
        let saved = self.index;
        let mut start = Arrow::None;
        if let Some(c) = self.current()
            && "<ox".contains(c)
            && matches!(self.peek(1), Some('-' | '=' | '.'))
        {
            start = match c {
                '<' => Arrow::Arrow,
                'o' => Arrow::Circle,
                _ => Arrow::Cross,
            };
            self.index += 1;
        }
        if !matches!(self.current(), Some('-' | '=' | '~')) {
            self.index = saved;
            return None;
        }

        let mut body = String::new();
        while let Some(c) = self.current().filter(|c| "-=.~".contains(*c)) {
            body.push(c);
            self.index += 1;
        }
        // Inline label: `-- text -->`, `-. text .->`, `== text ==>`
        if self.current() == Some(' ') && matches!(body.as_str(), "--" | "-." | "==") {
            let rest_start = self.index;
            if let Some((label, closing_body, closing_end)) = self.read_inline_label(&body) {
                let mut link = link(&closing_body, closing_end);
                link.start = start;
                link.label = Some(label);
                return Some(link);
            }
            self.index = rest_start;
        }

        let mut end = Arrow::None;
        match self.current() {
            Some('>') => {
                end = Arrow::Arrow;
                self.index += 1;
            }
            Some(c @ ('o' | 'x')) => {
                // `--o B` is a circle end; `--oops` would be a node.
                if matches!(self.peek(1), Some(' ' | '|') | None) {
                    end = if c == 'o' { Arrow::Circle } else { Arrow::Cross };
                    self.index += 1;
                }
            }
            _ => {}
        }
        // Bare `--` with nothing after is not a link.
        if end == Arrow::None && body.chars().count() < 3 && !body.contains('.') {
            self.index = saved;
            return None;
        }
        let mut link = link(&body, end);
        link.start = start;
        self.skip_spaces();
        if self.consume("|")
            && let Some(label) = self.read_until("|")
        {
            link.label = Some(clean(&label));
        }
        Some(link)
    }

    fn read_inline_label(&mut self, opening: &str) -> Option<(String, String, Arrow)> {
        let mut text = String::new();
        while !self.is_at_end() {
            let c = self.chars[self.index];
            if "-=.".contains(c) && (text.is_empty() || text.ends_with(' ')) {
                let mut body = String::new();
                let saved = self.index;
                while let Some(symbol) = self.current().filter(|c| "-=.".contains(*c)) {
                    body.push(symbol);
                    self.index += 1;
                }
                let mut end = Arrow::None;
                if self.current() == Some('>') {
                    end = Arrow::Arrow;
                    self.index += 1;
                } else if let Some(symbol @ ('o' | 'x')) = self.current()
                    && self.peek(1) == Some(' ')
                {
                    end = if symbol == 'o' { Arrow::Circle } else { Arrow::Cross };
                    self.index += 1;
                }
                if body.chars().count() >= 2 || end != Arrow::None {
                    let label = clean(&text);
                    if label.is_empty() {
                        return None;
                    }
                    return Some((label, format!("{opening}{body}"), end));
                }
                self.index = saved;
            }
            text.push(c);
            self.index += 1;
        }
        None
    }
}

fn link(body: &str, end: Arrow) -> Link {
    let line = if body.contains('~') {
        Line::Invisible
    } else if body.contains('.') {
        Line::Dotted
    } else if body.contains('=') {
        Line::Thick
    } else {
        Line::Solid
    };
    Link { label: None, line, start: Arrow::None, end }
}
