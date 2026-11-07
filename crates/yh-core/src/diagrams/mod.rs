//! Mermaid diagrams drawn natively: flowcharts and sequence diagrams parsed from source, then
//! laid out into plain geometry (frames, polylines, wrapped label lines) for the app to draw.
//! Text is measured through `TextMetrics`, which the app fills in for its font.

mod flowchart;
mod flowchart_layout;
mod layered;
mod sequence;
mod sequence_layout;
mod text;

#[cfg(test)]
mod tests;

pub use flowchart::{Arrow, Direction, Edge, Flowchart, Line, Node, Shape, Subgraph};
pub use flowchart_layout::{
    FlowchartGeometry, MAX_EDGE_LABEL_WIDTH, MAX_LABEL_WIDTH, PlacedEdge, PlacedNode,
    PlacedSubgraph, node_size,
};
pub use layered::{Boundary, clip};
pub use sequence::{
    ArrowHead, Block, BlockKind, Item, LineStyle, Message, Note, NotePlacement, Participant,
    Section, SequenceDiagram,
};
pub use sequence_layout::{
    PlacedBlock, PlacedBlockSection, PlacedMessage, PlacedNote, PlacedParticipant,
    SequenceGeometry,
};
pub use text::{DiagramColor, MermaidError, ShapeStyle, TextMetrics, WrappedText};

/// A point, in points, y down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

/// A width and a height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size { width: 0.0, height: 0.0 };

    pub const fn new(width: f64, height: f64) -> Self {
        Size { width, height }
    }
}

/// A rectangle by its top-left corner and size; widths and heights are never negative.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Rect { x, y, width, height }
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }
    pub fn min_y(&self) -> f64 {
        self.y
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
    pub fn center(&self) -> Point {
        Point::new(self.mid_x(), self.mid_y())
    }
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn offset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Shrunk by `dx` on the left and right and `dy` at the top and bottom (grown when negative).
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width - 2.0 * dx, self.height - 2.0 * dy)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.min_x().min(other.min_x());
        let y = self.min_y().min(other.min_y());
        let right = self.max_x().max(other.max_x());
        let bottom = self.max_y().max(other.max_y());
        Rect::new(x, y, right - x, bottom - y)
    }

    /// Whether they share any area; touching edges don't count.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min_x() < other.max_x()
            && other.min_x() < self.max_x()
            && self.min_y() < other.max_y()
            && other.min_y() < self.max_y()
    }

    /// Whether `other` lies wholly inside, edges included.
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.min_x() >= self.min_x()
            && other.max_x() <= self.max_x()
            && other.min_y() >= self.min_y()
            && other.max_y() <= self.max_y()
    }

    /// Half-open, as Core Graphics does: the left and top edges are in, the right and bottom out.
    pub fn contains_point(&self, point: Point) -> bool {
        point.x >= self.min_x()
            && point.x < self.max_x()
            && point.y >= self.min_y()
            && point.y < self.max_y()
    }
}

/// A parsed Mermaid diagram of a kind we draw natively.
#[derive(Clone, Debug, PartialEq)]
pub enum Diagram {
    Flowchart(Flowchart),
    Sequence(SequenceDiagram),
}

/// A laid out diagram, ready to draw.
#[derive(Clone, Debug)]
pub enum Geometry {
    Flowchart(FlowchartGeometry),
    Sequence(SequenceGeometry),
}

impl Geometry {
    /// The whole drawing's size; everything in it sits between zero and this.
    pub fn size(&self) -> Size {
        match self {
            Geometry::Flowchart(geometry) => geometry.size,
            Geometry::Sequence(geometry) => geometry.size,
        }
    }
}

fn header(source: &str) -> String {
    text::statements(&strip_front_matter(source))
        .into_iter()
        .next()
        .map(|line| line.to_lowercase())
        .unwrap_or_default()
}

/// Whether we draw this kind at all; other Mermaid diagrams stay as code.
pub fn is_supported(source: &str) -> bool {
    let header = header(source);
    header.starts_with("flowchart")
        || header.starts_with("graph")
        || header.starts_with("sequencediagram")
}

/// Parses a flowchart or a sequence diagram. Never panics: anything it can't read is an error
/// with a message fit to show under the code.
pub fn parse(source: &str) -> Result<Diagram, MermaidError> {
    let source = strip_front_matter(source);
    let header = text::statements(&source)
        .into_iter()
        .next()
        .map(|line| line.to_lowercase())
        .unwrap_or_default();
    if header.starts_with("sequencediagram") {
        return sequence::parse(&source).map(Diagram::Sequence);
    }
    if header.starts_with("flowchart") || header.starts_with("graph") {
        return flowchart::parse(&source).map(Diagram::Flowchart);
    }
    Err(MermaidError::new("This kind of Mermaid diagram isn't drawn yet."))
}

/// Lays a parsed diagram out with the app's text measurements.
pub fn layout(diagram: &Diagram, metrics: &TextMetrics) -> Geometry {
    match diagram {
        Diagram::Flowchart(chart) => Geometry::Flowchart(flowchart_layout::layout(chart, metrics)),
        Diagram::Sequence(diagram) => {
            Geometry::Sequence(sequence_layout::layout(diagram, metrics))
        }
    }
}

/// `---\ntitle: …\n---` and `%%{init: …}%%` lines carry config we don't use.
pub(crate) fn strip_front_matter(source: &str) -> String {
    let mut lines: Vec<&str> = text::lines(source).collect();
    let start = lines.iter().take_while(|line| text::trim(line).is_empty()).count();
    lines.drain(..start);
    if lines.first().map(|line| text::trim(line)) == Some("---")
        && let Some(close) = lines.iter().skip(1).position(|line| text::trim(line) == "---")
    {
        lines.drain(..=close + 1);
    }
    lines
        .into_iter()
        .filter(|line| !text::trim(line).starts_with("%%{"))
        .collect::<Vec<_>>()
        .join("\n")
}
