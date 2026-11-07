//! A Mermaid `sequenceDiagram`: participants, messages, notes, activations and blocks (`loop`,
//! `alt`/`else`, `opt`, `par`/`and`, `critical`/`option`, `break`, `rect`). `box` groups are
//! read but not drawn; their contents are kept.

use super::text::{self, MermaidError, clean, trim};

#[derive(Clone, Debug, PartialEq)]
pub struct Participant {
    pub id: String,
    pub label: String,
    /// Declared with `actor`: drawn with a stick figure.
    pub is_actor: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineStyle {
    Solid,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrowHead {
    /// `->` / `-->`
    None,
    /// `->>` / `-->>`
    Filled,
    /// `-x` / `--x`
    Cross,
    /// `-)` / `--)`: async
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub from: String,
    pub to: String,
    pub text: String,
    pub line: LineStyle,
    pub head: ArrowHead,
    /// `<<->>`: a head at both ends.
    pub bidirectional: bool,
    /// `->>+B`
    pub activates_target: bool,
    /// `->>-B`
    pub deactivates_source: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotePlacement {
    LeftOf,
    RightOf,
    Over,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub placement: NotePlacement,
    pub participants: Vec<String>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Loop,
    Alt,
    Opt,
    Par,
    Critical,
    Break,
    Rect,
}

impl BlockKind {
    fn from_word(word: &str) -> Option<BlockKind> {
        Some(match word {
            "loop" => BlockKind::Loop,
            "alt" => BlockKind::Alt,
            "opt" => BlockKind::Opt,
            "par" => BlockKind::Par,
            "critical" => BlockKind::Critical,
            "break" => BlockKind::Break,
            "rect" => BlockKind::Rect,
            _ => return None,
        })
    }

    /// The keyword, as drawn in the block's tab.
    pub fn name(self) -> &'static str {
        match self {
            BlockKind::Loop => "loop",
            BlockKind::Alt => "alt",
            BlockKind::Opt => "opt",
            BlockKind::Par => "par",
            BlockKind::Critical => "critical",
            BlockKind::Break => "break",
            BlockKind::Rect => "rect",
        }
    }
}

/// One part of a block: the first, then one per `else`, `and` or `option`.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub label: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Message(Message),
    Note(Note),
    Activate(String),
    Deactivate(String),
    Block(Block),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SequenceDiagram {
    pub participants: Vec<Participant>,
    pub items: Vec<Item>,
    pub autonumber: bool,
    pub title: Option<String>,
}

fn unreadable(line: &str) -> MermaidError {
    MermaidError::new(format!("Couldn't read “{line}”."))
}

struct Participants(Vec<Participant>);

impl Participants {
    fn ensure(&mut self, id: &str, label: Option<String>, actor: bool) {
        match self.0.iter_mut().find(|participant| participant.id == id) {
            None => self.0.push(Participant {
                id: id.to_string(),
                label: label.unwrap_or_else(|| id.to_string()),
                is_actor: actor,
            }),
            Some(existing) => {
                if let Some(label) = label {
                    existing.label = label;
                    existing.is_actor = existing.is_actor || actor;
                }
            }
        }
    }
}

/// An open block (or `box`, with no kind), collecting its sections; the root collects
/// top-level items.
struct Open {
    kind: Option<BlockKind>,
    sections: Vec<Section>,
}

impl Open {
    fn new(kind: Option<BlockKind>, label: String) -> Self {
        Open { kind, sections: vec![Section { label, items: Vec::new() }] }
    }
}

fn append(stack: &mut [Open], item: Item) {
    if let Some(open) = stack.last_mut()
        && let Some(section) = open.sections.last_mut()
    {
        section.items.push(item);
    }
}

pub(crate) fn parse(source: &str) -> Result<SequenceDiagram, MermaidError> {
    let lines = text::statements(source);
    if !lines.first().is_some_and(|first| first.to_lowercase().starts_with("sequencediagram")) {
        return Err(MermaidError::new("Expected `sequenceDiagram`."));
    }

    let mut participants = Participants(Vec::new());
    let mut autonumber = false;
    let mut title = None;
    let mut stack = vec![Open::new(None, String::new())];

    for line in &lines[1..] {
        let first = line.split(' ').next().unwrap_or(line);
        let word = first.to_lowercase();
        let rest = &line[first.len()..];

        match word.as_str() {
            "participant" | "actor" => {
                let rest = trim(rest);
                let actor = word == "actor";
                match rest.find(" as ") {
                    Some(at) => participants.ensure(
                        trim(&rest[..at]),
                        Some(clean(&rest[at + 4..])),
                        actor,
                    ),
                    None => participants.ensure(rest, None, actor),
                }
            }
            "autonumber" => autonumber = true,
            "title" => title = Some(clean(rest)),
            "activate" | "deactivate" => {
                let id = trim(rest);
                participants.ensure(id, None, false);
                let item = if word == "activate" {
                    Item::Activate(id.to_string())
                } else {
                    Item::Deactivate(id.to_string())
                };
                append(&mut stack, item);
            }
            "note" => {
                let note = parse_note(line).ok_or_else(|| unreadable(line))?;
                for id in &note.participants {
                    participants.ensure(id, None, false);
                }
                append(&mut stack, Item::Note(note));
            }
            "loop" | "alt" | "opt" | "par" | "critical" | "break" | "rect" => {
                let kind = BlockKind::from_word(&word);
                let label = if kind == Some(BlockKind::Rect) { String::new() } else { clean(rest) };
                stack.push(Open::new(kind, label));
            }
            "else" | "and" | "option" => {
                if stack.len() <= 1 {
                    return Err(MermaidError::new(format!("`{word}` outside a block.")));
                }
                if let Some(open) = stack.last_mut() {
                    open.sections.push(Section { label: clean(rest), items: Vec::new() });
                }
            }
            "end" => {
                if stack.len() <= 1 {
                    return Err(MermaidError::new("`end` without a block."));
                }
                let Some(closed) = stack.pop() else { continue };
                match closed.kind {
                    Some(kind) => append(
                        &mut stack,
                        Item::Block(Block { kind, sections: closed.sections }),
                    ),
                    // A `box`: we don't draw it, but keep whatever was inside.
                    None => {
                        for item in closed.sections.into_iter().flat_map(|section| section.items) {
                            append(&mut stack, item);
                        }
                    }
                }
            }
            "box" => stack.push(Open::new(None, String::new())),
            // Decoration we don't draw.
            "links" | "link" | "properties" | "details" | "create" | "destroy" => {}
            _ => {
                let message = parse_message(line).ok_or_else(|| unreadable(line))?;
                participants.ensure(&message.from, None, false);
                participants.ensure(&message.to, None, false);
                append(&mut stack, Item::Message(message));
            }
        }
    }
    if stack.len() != 1 {
        return Err(MermaidError::new("A block is missing its `end`."));
    }
    if participants.0.is_empty() {
        return Err(MermaidError::new("The diagram has no participants."));
    }
    let items = stack
        .pop()
        .and_then(|root| root.sections.into_iter().next())
        .map(|section| section.items)
        .unwrap_or_default();
    Ok(SequenceDiagram { participants: participants.0, items, autonumber, title })
}

/// Longest arrows first, so `-->>` isn't read as `-->` followed by `>`.
const ARROWS: [(&str, LineStyle, ArrowHead, bool); 10] = [
    ("<<-->>", LineStyle::Dotted, ArrowHead::Filled, true),
    ("<<->>", LineStyle::Solid, ArrowHead::Filled, true),
    ("-->>", LineStyle::Dotted, ArrowHead::Filled, false),
    ("->>", LineStyle::Solid, ArrowHead::Filled, false),
    ("--x", LineStyle::Dotted, ArrowHead::Cross, false),
    ("-x", LineStyle::Solid, ArrowHead::Cross, false),
    ("--)", LineStyle::Dotted, ArrowHead::Open, false),
    ("-)", LineStyle::Solid, ArrowHead::Open, false),
    ("-->", LineStyle::Dotted, ArrowHead::None, false),
    ("->", LineStyle::Solid, ArrowHead::None, false),
];

fn parse_message(line: &str) -> Option<Message> {
    let colon = line.find(':')?;
    let head = &line[..colon];
    let text = clean(&line[colon + 1..]);
    for (token, style, arrow, both) in ARROWS {
        let Some(at) = head.find(token) else { continue };
        let from = trim(&head[..at]);
        let mut to = trim(&head[at + token.len()..]);
        let mut activates = false;
        let mut deactivates = false;
        if let Some(rest) = to.strip_prefix('+') {
            activates = true;
            to = rest;
        } else if let Some(rest) = to.strip_prefix('-') {
            deactivates = true;
            to = rest;
        }
        let to = trim(to);
        if from.is_empty() || to.is_empty() {
            return None;
        }
        return Some(Message {
            from: from.to_string(),
            to: to.to_string(),
            text,
            line: style,
            head: arrow,
            bidirectional: both,
            activates_target: activates,
            deactivates_source: deactivates,
        });
    }
    None
}

fn parse_note(line: &str) -> Option<Note> {
    let colon = line.find(':')?;
    let head = trim(&line[..colon]);
    let text = clean(&line[colon + 1..]);
    // Up to three words, then the rest as one.
    let mut words: Vec<&str> = Vec::new();
    let mut rest = head;
    while words.len() < 3 {
        rest = rest.trim_start_matches(' ');
        if rest.is_empty() {
            break;
        }
        let end = rest.find(' ').unwrap_or(rest.len());
        words.push(&rest[..end]);
        rest = &rest[end..];
    }
    let rest = rest.trim_start_matches(' ');
    if !rest.is_empty() {
        words.push(rest);
    }
    if words.len() < 3 {
        return None;
    }
    let (placement, target) = match words[1].to_lowercase().as_str() {
        "over" => (NotePlacement::Over, words[2..].join(" ")),
        side @ ("left" | "right") => {
            if words.len() < 4 || !words[2].eq_ignore_ascii_case("of") {
                return None;
            }
            let placement = if side == "left" { NotePlacement::LeftOf } else { NotePlacement::RightOf };
            (placement, words[3].to_string())
        }
        _ => return None,
    };
    let ids: Vec<String> = target
        .split(',')
        .map(|id| trim(id).to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(Note { placement, participants: ids, text })
}
