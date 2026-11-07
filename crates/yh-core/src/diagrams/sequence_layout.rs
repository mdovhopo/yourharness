//! A sequence diagram placed: participants' headers and lifelines, messages, notes, activation
//! bars and blocks, in one pass down the page in reading order.

use super::sequence::{BlockKind, Item, Message, NotePlacement, Participant, SequenceDiagram};
use super::text::{TextMetrics, WrappedText};
use super::{Rect, Size};
use std::collections::{BTreeMap, HashMap};

const PARTICIPANT_LABEL_WIDTH: f64 = 150.0;
const MESSAGE_LABEL_WIDTH: f64 = 260.0;
const NOTE_WIDTH: f64 = 200.0;
const BAR_WIDTH: f64 = 10.0;
/// How far a message to oneself reaches right of its lifeline.
pub const SELF_LOOP_WIDTH: f64 = 32.0;
/// Room above a header's label for an actor's stick figure.
pub const ACTOR_ICON_HEIGHT: f64 = 18.0;

#[derive(Clone, Debug)]
pub struct PlacedParticipant {
    pub participant: Participant,
    /// Where its lifeline runs.
    pub center_x: f64,
    /// The box at the top (all headers are as tall as the tallest); actors have a stick figure
    /// `ACTOR_ICON_HEIGHT` tall above the label.
    pub header: Rect,
    pub label: WrappedText,
}

#[derive(Clone, Debug)]
pub struct PlacedMessage {
    /// `line`, `head` and `bidirectional` say how to draw the arrow.
    pub message: Message,
    pub from_x: f64,
    /// Equal to `from_x` for a message to oneself, which loops out `SELF_LOOP_WIDTH` to the
    /// right and comes back 20 pt lower.
    pub to_x: f64,
    /// The arrow's height.
    pub y: f64,
    pub is_self: bool,
    pub label: WrappedText,
    /// Above the arrow.
    pub label_frame: Rect,
    /// With `autonumber`, its number, drawn at `from_x`.
    pub number: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct PlacedNote {
    pub frame: Rect,
    /// Centered in `frame`.
    pub text: WrappedText,
}

#[derive(Clone, Debug)]
pub struct PlacedBlockSection {
    pub label: WrappedText,
    /// Top of the section; the first one is the block's top. Later ones draw a dashed
    /// divider here.
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct PlacedBlock {
    /// Its name goes in a tab at the top-left, the first section's label beside it.
    pub kind: BlockKind,
    pub frame: Rect,
    pub sections: Vec<PlacedBlockSection>,
    /// 0 for the outermost.
    pub depth: usize,
}

#[derive(Clone, Debug)]
pub struct SequenceGeometry {
    pub size: Size,
    pub participants: Vec<PlacedParticipant>,
    /// Lifelines run from the headers' bottom to here.
    pub lifeline_top: f64,
    pub lifeline_bottom: f64,
    pub messages: Vec<PlacedMessage>,
    pub notes: Vec<PlacedNote>,
    /// Activation bars.
    pub activations: Vec<Rect>,
    /// Outermost first.
    pub blocks: Vec<PlacedBlock>,
    pub title: Option<String>,
}

struct Placer<'a> {
    diagram: &'a SequenceDiagram,
    metrics: &'a TextMetrics,
    column: &'a HashMap<&'a str, usize>,
    centers: Vec<f64>,
    number_width: f64,
    messages: Vec<PlacedMessage>,
    notes: Vec<PlacedNote>,
    activations: Vec<Rect>,
    blocks: Vec<PlacedBlock>,
    /// Participant → start y of each active bar.
    open: BTreeMap<usize, Vec<f64>>,
    y: f64,
    last_arrow_y: f64,
    number: usize,
}

type Extent = Option<(f64, f64)>;

fn include(extent: &mut Extent, lo: f64, hi: f64) {
    *extent = Some(match *extent {
        Some((a, b)) => (a.min(lo), b.max(hi)),
        None => (lo, hi),
    });
}

impl Placer<'_> {
    fn activation_offset(&self, index: usize) -> f64 {
        if self.open.get(&index).is_none_or(Vec::is_empty) { 0.0 } else { BAR_WIDTH / 2.0 }
    }

    fn activate(&mut self, index: usize, start: f64) {
        self.open.entry(index).or_default().push(start);
    }

    fn deactivate(&mut self, index: usize, end: f64) {
        let Some(bars) = self.open.get_mut(&index) else { return };
        let Some(start) = bars.pop() else { return };
        let depth = bars.len() as f64;
        self.activations.push(Rect::new(
            self.centers[index] - BAR_WIDTH / 2.0 + depth * 4.0,
            start,
            BAR_WIDTH,
            (end - start).max(12.0),
        ));
    }

    /// Returns the horizontal extent of what it placed, for sizing enclosing blocks.
    fn place(&mut self, items: &[Item], depth: usize) -> Extent {
        let mut extent: Extent = None;
        let autonumber = self.diagram.autonumber;
        for item in items {
            match item {
                Item::Message(message) => {
                    let (Some(&a), Some(&b)) =
                        (self.column.get(message.from.as_str()), self.column.get(message.to.as_str()))
                    else {
                        continue;
                    };
                    let label = WrappedText::wrap(&message.text, MESSAGE_LABEL_WIDTH, self.metrics);
                    let label_top = self.y;
                    let arrow_y = self.y + label.size.height + 5.0;
                    if autonumber {
                        self.number += 1;
                    }
                    let number = autonumber.then_some(self.number);
                    if a == b {
                        let from_x = self.centers[a] + self.activation_offset(a);
                        let frame = Rect::new(
                            from_x + 8.0 + self.number_width,
                            label_top,
                            label.size.width,
                            label.size.height,
                        );
                        include(&mut extent, self.centers[a], frame.max_x().max(from_x + SELF_LOOP_WIDTH));
                        self.messages.push(PlacedMessage {
                            message: message.clone(),
                            from_x,
                            to_x: from_x,
                            y: arrow_y,
                            is_self: true,
                            label,
                            label_frame: frame,
                            number,
                        });
                        self.y = arrow_y + 20.0 + 16.0;
                    } else {
                        let direction = if b > a { 1.0 } else { -1.0 };
                        let from_x = self.centers[a] + direction * self.activation_offset(a);
                        let to_x = self.centers[b] - direction * self.activation_offset(b);
                        let mid = (from_x + to_x) / 2.0;
                        let frame = Rect::new(
                            mid - label.size.width / 2.0,
                            label_top,
                            label.size.width,
                            label.size.height,
                        );
                        include(&mut extent, from_x.min(to_x), from_x.max(to_x));
                        self.messages.push(PlacedMessage {
                            message: message.clone(),
                            from_x,
                            to_x,
                            y: arrow_y,
                            is_self: false,
                            label,
                            label_frame: frame,
                            number,
                        });
                        self.y = arrow_y + 16.0;
                    }
                    self.last_arrow_y = arrow_y;
                    if message.activates_target {
                        self.activate(b, arrow_y);
                    }
                    if message.deactivates_source {
                        self.deactivate(a, arrow_y);
                    }
                }
                Item::Note(note) => {
                    let text = WrappedText::wrap(&note.text, NOTE_WIDTH, self.metrics);
                    let ids = note.participants.iter().filter_map(|id| self.column.get(id.as_str()).copied());
                    let (Some(first), Some(last)) = (ids.clone().min(), ids.max()) else { continue };
                    let width = text.size.width + 24.0;
                    let height = text.size.height + 14.0;
                    let y = self.y;
                    let centers = &self.centers;
                    let frame = match note.placement {
                        NotePlacement::RightOf => Rect::new(centers[first] + 12.0, y, width, height),
                        NotePlacement::LeftOf => Rect::new(centers[first] - 12.0 - width, y, width, height),
                        NotePlacement::Over if first == last => {
                            Rect::new(centers[first] - width / 2.0, y, width, height)
                        }
                        NotePlacement::Over => Rect::new(
                            centers[first] - 20.0,
                            y,
                            (centers[last] - centers[first] + 40.0).max(width),
                            height,
                        ),
                    };
                    include(&mut extent, frame.min_x(), frame.max_x());
                    self.notes.push(PlacedNote { frame, text });
                    self.y += height + 14.0;
                }
                Item::Activate(id) => {
                    if let Some(&index) = self.column.get(id.as_str()) {
                        self.activate(index, self.last_arrow_y);
                    }
                }
                Item::Deactivate(id) => {
                    if let Some(&index) = self.column.get(id.as_str()) {
                        self.deactivate(index, self.last_arrow_y);
                    }
                }
                Item::Block(block) => {
                    let top = self.y;
                    let block_index = self.blocks.len();
                    self.blocks.push(PlacedBlock {
                        kind: block.kind,
                        frame: Rect::default(),
                        sections: Vec::new(),
                        depth,
                    });
                    let mut sections = Vec::new();
                    let mut inner: Extent = None;
                    for (index, section) in block.sections.iter().enumerate() {
                        let label = WrappedText::wrap(&section.label, MESSAGE_LABEL_WIDTH, self.metrics);
                        if index > 0 {
                            self.y += 4.0;
                        }
                        let label_height = label.size.height.max(self.metrics.line_height);
                        sections.push(PlacedBlockSection { label, y: self.y });
                        self.y += label_height + 14.0;
                        if let Some((lo, hi)) = self.place(&section.items, depth + 1) {
                            include(&mut inner, lo, hi);
                        }
                    }
                    self.y += 6.0;
                    let (lo, hi) = inner.unwrap_or((
                        self.centers.first().copied().unwrap_or(0.0),
                        self.centers.last().copied().unwrap_or(0.0),
                    ));
                    let pad = 24.0;
                    let mut frame = Rect::new(lo - pad, top, hi - lo + pad * 2.0, self.y - top);
                    // Wide enough for its own labels.
                    let tab = self.metrics.width(block.kind.name()) + 20.0;
                    let widest = sections.iter().map(|s| s.label.size.width).fold(0.0, f64::max);
                    frame.width = frame.width.max(tab + widest + 24.0);
                    self.blocks[block_index].frame = frame;
                    self.blocks[block_index].sections = sections;
                    include(&mut extent, frame.min_x(), frame.max_x());
                    self.y += 12.0;
                }
            }
        }
        extent
    }
}

struct Measure<'a> {
    metrics: &'a TextMetrics,
    column: &'a HashMap<&'a str, usize>,
    count: usize,
    gaps: Vec<f64>,
    left_extra: f64,
    right_extra: f64,
    number_width: f64,
}

impl Measure<'_> {
    fn need(&mut self, from: usize, to: usize, width: f64) {
        let (lo, hi) = (from.min(to), from.max(to));
        if hi <= lo {
            return;
        }
        let have: f64 = self.gaps[lo..hi].iter().sum();
        if have < width {
            self.gaps[hi - 1] += width - have;
        }
    }

    fn need_right(&mut self, index: usize, width: f64) {
        if index + 1 < self.count {
            self.gaps[index] = self.gaps[index].max(width);
        } else {
            self.right_extra = self.right_extra.max(width);
        }
    }

    fn need_left(&mut self, index: usize, width: f64) {
        if index > 0 {
            self.gaps[index - 1] = self.gaps[index - 1].max(width);
        } else {
            self.left_extra = self.left_extra.max(width);
        }
    }

    fn measure(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Message(message) => {
                    let (Some(&a), Some(&b)) =
                        (self.column.get(message.from.as_str()), self.column.get(message.to.as_str()))
                    else {
                        continue;
                    };
                    let label = WrappedText::wrap(&message.text, MESSAGE_LABEL_WIDTH, self.metrics);
                    if a == b {
                        self.need_right(a, label.size.width + SELF_LOOP_WIDTH + 24.0 + self.number_width);
                    } else {
                        self.need(a, b, label.size.width + 36.0 + self.number_width);
                    }
                }
                Item::Note(note) => {
                    let text = WrappedText::wrap(&note.text, NOTE_WIDTH, self.metrics);
                    let width = text.size.width + 24.0;
                    let ids = note.participants.iter().filter_map(|id| self.column.get(id.as_str()).copied());
                    let (Some(first), Some(last)) = (ids.clone().min(), ids.max()) else { continue };
                    match note.placement {
                        NotePlacement::RightOf => self.need_right(first, width + 24.0),
                        NotePlacement::LeftOf => self.need_left(first, width + 24.0),
                        NotePlacement::Over if first == last => {
                            self.need_right(first, width / 2.0 + 16.0);
                            self.need_left(first, width / 2.0 + 16.0);
                        }
                        NotePlacement::Over => self.need(first, last, width - 40.0),
                    }
                }
                Item::Block(block) => {
                    for section in &block.sections {
                        self.measure(&section.items);
                    }
                }
                Item::Activate(_) | Item::Deactivate(_) => {}
            }
        }
    }
}

pub(crate) fn layout(diagram: &SequenceDiagram, metrics: &TextMetrics) -> SequenceGeometry {
    let count = diagram.participants.len();
    let mut column: HashMap<&str, usize> = HashMap::new();
    for (index, participant) in diagram.participants.iter().enumerate() {
        column.entry(participant.id.as_str()).or_insert(index);
    }
    let labels: Vec<WrappedText> = diagram
        .participants
        .iter()
        .map(|p| WrappedText::wrap(&p.label, PARTICIPANT_LABEL_WIDTH, metrics))
        .collect();
    let header_sizes: Vec<Size> = diagram
        .participants
        .iter()
        .zip(&labels)
        .map(|(participant, label)| {
            Size::new(
                (label.size.width + 28.0).max(76.0),
                label.size.height + 18.0 + if participant.is_actor { ACTOR_ICON_HEIGHT } else { 0.0 },
            )
        })
        .collect();
    let header_height = header_sizes.iter().map(|s| s.height).fold(0.0, f64::max);

    // Horizontal: gaps between neighbouring lifelines, widened for what sits between them.
    let number_width = if diagram.autonumber { 22.0 } else { 0.0 };
    let mut measure = Measure {
        metrics,
        column: &column,
        count,
        gaps: (0..count.saturating_sub(1))
            .map(|i| (header_sizes[i].width + header_sizes[i + 1].width) / 2.0 + 28.0)
            .collect(),
        left_extra: 0.0,
        right_extra: 0.0,
        number_width,
    };
    measure.measure(&diagram.items);

    let margin = 12.0;
    let mut centers = Vec::with_capacity(count);
    let mut x = margin + header_sizes.first().map_or(0.0, |s| s.width / 2.0).max(measure.left_extra);
    for index in 0..count {
        if index > 0 {
            x += measure.gaps[index - 1];
        }
        centers.push(x);
    }

    // Vertical: one pass in reading order.
    let y = header_height + 22.0;
    let mut placer = Placer {
        diagram,
        metrics,
        column: &column,
        centers,
        number_width,
        messages: Vec::new(),
        notes: Vec::new(),
        activations: Vec::new(),
        blocks: Vec::new(),
        open: BTreeMap::new(),
        y,
        last_arrow_y: y,
        number: 0,
    };
    placer.place(&diagram.items, 0);
    let still_open: Vec<(usize, usize)> = placer.open.iter().map(|(&i, bars)| (i, bars.len())).collect();
    for (index, bars) in still_open {
        for _ in 0..bars {
            placer.deactivate(index, placer.y - 8.0);
        }
    }

    let Placer { centers, mut messages, mut notes, mut activations, mut blocks, y: bottom, .. } = placer;
    let mut participants: Vec<PlacedParticipant> = diagram
        .participants
        .iter()
        .enumerate()
        .map(|(index, participant)| PlacedParticipant {
            participant: participant.clone(),
            center_x: centers[index],
            header: Rect::new(
                centers[index] - header_sizes[index].width / 2.0,
                0.0,
                header_sizes[index].width,
                header_height,
            ),
            label: labels[index].clone(),
        })
        .collect();

    // Normalize so nothing hangs off the left edge.
    let mut min_x = participants.iter().map(|p| p.header.min_x()).fold(f64::INFINITY, f64::min);
    let mut max_x = participants.iter().map(|p| p.header.max_x()).fold(f64::NEG_INFINITY, f64::max);
    if participants.is_empty() {
        (min_x, max_x) = (0.0, 0.0);
    }
    for note in &notes {
        min_x = min_x.min(note.frame.min_x());
        max_x = max_x.max(note.frame.max_x());
    }
    for block in &blocks {
        min_x = min_x.min(block.frame.min_x());
        max_x = max_x.max(block.frame.max_x());
    }
    for message in &messages {
        let reach = message.to_x + if message.is_self { SELF_LOOP_WIDTH } else { 0.0 };
        max_x = max_x.max(message.label_frame.max_x()).max(reach);
    }
    let dx = -min_x;
    for participant in &mut participants {
        participant.center_x += dx;
        participant.header = participant.header.offset(dx, 0.0);
    }
    for message in &mut messages {
        message.from_x += dx;
        message.to_x += dx;
        message.label_frame = message.label_frame.offset(dx, 0.0);
    }
    for note in &mut notes {
        note.frame = note.frame.offset(dx, 0.0);
    }
    for block in &mut blocks {
        block.frame = block.frame.offset(dx, 0.0);
    }
    for bar in &mut activations {
        *bar = bar.offset(dx, 0.0);
    }
    blocks.sort_by_key(|block| block.depth);

    SequenceGeometry {
        size: Size::new(max_x - min_x, bottom),
        participants,
        lifeline_top: header_height,
        lifeline_bottom: bottom,
        messages,
        notes,
        activations,
        blocks,
        title: diagram.title.clone(),
    }
}
