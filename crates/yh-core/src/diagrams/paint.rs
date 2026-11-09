//! Painting a laid-out diagram: its shapes through a `Canvas` and its labels as text, in
//! colours that are roles (`Ink`) the drawer resolves for its own light or dark. The Mac paints
//! to its window and to exported images; the server records the same for the phone.

use super::{Arrow, ArrowHead, DiagramColor, FlowchartGeometry, Geometry, Line, LineStyle, Rect, SequenceGeometry, Shape, ShapeStyle, WrappedText};

/// Label text's size and line height (points).
pub const FONT_SIZE: f32 = 13.;
pub const LINE_HEIGHT: f64 = 17.;

/// What a colour is: the text colour (`Text`, faded by `alpha` for lines and fills), the secondary
/// text colour, the page (`Canvas`) and raised (`Card`) backgrounds, the accent, or an author's
/// own colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    Text,
    Secondary,
    Canvas,
    Card,
    Accent,
    Rgb(f32, f32, f32),
}

/// A colour to paint with: a role at an alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub role: Role,
    pub alpha: f32,
}

impl Ink {
    pub const fn text(alpha: f32) -> Ink {
        Ink { role: Role::Text, alpha }
    }
    pub const fn secondary() -> Ink {
        Ink { role: Role::Secondary, alpha: 1. }
    }
    pub const fn canvas() -> Ink {
        Ink { role: Role::Canvas, alpha: 1. }
    }
    pub const fn card() -> Ink {
        Ink { role: Role::Card, alpha: 1. }
    }
    pub const fn accent() -> Ink {
        Ink { role: Role::Accent, alpha: 1. }
    }
    pub const fn rgb(r: f32, g: f32, b: f32) -> Ink {
        Ink { role: Role::Rgb(r, g, b), alpha: 1. }
    }
    /// Nothing: a path stroked but not filled.
    pub const fn clear() -> Ink {
        Ink { role: Role::Rgb(0., 0., 0.), alpha: 0. }
    }
    /// Fainter by `factor`.
    pub fn opacity(self, factor: f32) -> Ink {
        Ink { alpha: self.alpha * factor, ..self }
    }
}

/// The diagram's shapes, painted onto `p`.
pub fn paint(p: &mut dyn Canvas, geometry: &Geometry) {
    match geometry {
        Geometry::Flowchart(g) => paint_flowchart(p, g),
        Geometry::Sequence(g) => paint_sequence(p, g),
    }
}

/// A piece of text over the diagram: its lines from (`x`, `y`), centred across `width` when
/// it has one. Drawn as elements in the window, as `<text>` in an exported image.
pub struct Label {
    pub lines: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub width: Option<f64>,
    pub size: f32,
    pub line_height: f64,
    pub weight: u16,
    pub color: Ink,
}

/// A label's lines, centred in `frame` (with some room either side, as words may measure a
/// little wider than the layout thought).
fn centred(text: &WrappedText, frame: Rect, color: Ink, bold: bool) -> Label {
    let top = frame.y + frame.height / 2. - text.lines.len() as f64 * LINE_HEIGHT / 2.;
    Label {
        lines: text.lines.clone(),
        x: frame.x - 20.,
        y: top,
        width: Some(frame.width + 40.),
        size: FONT_SIZE,
        line_height: LINE_HEIGHT,
        weight: if bold { 600 } else { 400 },
        color,
    }
}

/// Text at a point, left-aligned.
fn at(text: String, x: f64, y: f64, color: Ink, size: f32, bold: bool) -> Label {
    Label { lines: vec![text], x, y, width: None, size, line_height: LINE_HEIGHT, weight: if bold { 600 } else { 400 }, color }
}

pub fn labels(geometry: &Geometry) -> Vec<Label> {
    let mut out = vec![];
    match geometry {
        Geometry::Flowchart(g) => {
            for s in &g.subgraphs {
                if !s.title.is_empty() {
                    // A subgraph's title: left-aligned from its corner.
                    out.push(Label {
                        lines: s.title.lines.clone(),
                        x: s.frame.x + 12.,
                        y: s.frame.y + 6.,
                        width: None,
                        size: 12.,
                        line_height: LINE_HEIGHT,
                        weight: 500,
                        color: Ink::secondary(),
                    });
                }
            }
            for n in &g.nodes {
                out.push(centred(&n.label, n.frame, Ink::text(1.), false));
            }
            for e in &g.edges {
                if let (Some(frame), false) = (e.label_frame, e.label.is_empty()) {
                    out.push(centred(&e.label, frame, Ink::secondary(), false));
                }
            }
        }
        Geometry::Sequence(g) => {
            for p in &g.participants {
                let mut frame = p.header;
                if p.participant.is_actor {
                    frame = Rect::new(frame.x, frame.y + 18., frame.width, frame.height - 18.);
                }
                out.push(centred(&p.label, frame, Ink::text(1.), true));
            }
            for m in &g.messages {
                if !m.label.is_empty() {
                    out.push(centred(&m.label, m.label_frame, Ink::text(1.), false));
                }
                if let Some(number) = m.number {
                    out.push(Label {
                        lines: vec![number.to_string()],
                        x: m.from_x - 8.,
                        y: m.y - 8.,
                        width: Some(16.),
                        size: 9.,
                        line_height: 16.,
                        weight: 700,
                        color: Ink::rgb(1., 1., 1.),
                    });
                }
            }
            for note in &g.notes {
                out.push(centred(&note.text, note.frame, Ink::text(1.), false));
            }
            for b in &g.blocks {
                out.push(at(b.kind.name().to_string(), b.frame.x + 8., b.frame.y + 3., Ink::secondary(), 11.5, true));
                let tab = tab_width(b.kind.name());
                for (i, section) in b.sections.iter().enumerate() {
                    let text = section.label.lines.join(" ");
                    if text.is_empty() {
                        continue;
                    }
                    let x = if i == 0 { b.frame.x + tab + 8. } else { b.frame.x + 8. };
                    out.push(at(format!("[{text}]"), x, section.y + 3., Ink::secondary(), 11.5, false));
                }
            }
        }
    }
    out
}

pub fn tab_width(name: &str) -> f64 {
    name.len() as f64 * 7. + 16.
}

/// A path in the diagram's own coordinates.
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    /// An elliptical arc (radii), clockwise, to a point.
    Arc((f64, f64), (f64, f64)),
    Close,
}

/// Where the shapes go: the Mac's window or an exported image, or a recording for a client.
pub trait Canvas {
    /// Fills, then outlines (colour, width, dashed), a path.
    fn path(&mut self, segs: &[Seg], fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>);

    fn polyline(&mut self, points: &[(f64, f64)], color: Ink, width: f32, dashed: bool) {
        if points.len() < 2 {
            return;
        }
        let mut segs = vec![Seg::Move(points[0].0, points[0].1)];
        segs.extend(points[1..].iter().map(|&(x, y)| Seg::Line(x, y)));
        self.path(&segs, None, Some((color, width, dashed)));
    }

    fn polygon(&mut self, points: &[(f64, f64)], fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        if points.is_empty() {
            return;
        }
        let mut segs = vec![Seg::Move(points[0].0, points[0].1)];
        segs.extend(points[1..].iter().map(|&(x, y)| Seg::Line(x, y)));
        segs.push(Seg::Close);
        self.path(&segs, fill, stroke);
    }

    fn rounded(&mut self, r: Rect, radius: f64, fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        let radius = radius.min(r.width / 2.).min(r.height / 2.).max(0.);
        if radius <= 0.5 {
            return self.polygon(&[(r.x, r.y), (r.max_x(), r.y), (r.max_x(), r.max_y()), (r.x, r.max_y())], fill, stroke);
        }
        let k = (radius, radius);
        let segs = [
            Seg::Move(r.x + radius, r.y),
            Seg::Line(r.max_x() - radius, r.y),
            Seg::Arc(k, (r.max_x(), r.y + radius)),
            Seg::Line(r.max_x(), r.max_y() - radius),
            Seg::Arc(k, (r.max_x() - radius, r.max_y())),
            Seg::Line(r.x + radius, r.max_y()),
            Seg::Arc(k, (r.x, r.max_y() - radius)),
            Seg::Line(r.x, r.y + radius),
            Seg::Arc(k, (r.x + radius, r.y)),
            Seg::Close,
        ];
        self.path(&segs, fill, stroke);
    }

    fn ellipse(&mut self, r: Rect, fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        let k = (r.width / 2., r.height / 2.);
        let segs = [Seg::Move(r.x, r.mid_y()), Seg::Arc(k, (r.max_x(), r.mid_y())), Seg::Arc(k, (r.x, r.mid_y())), Seg::Close];
        self.path(&segs, fill, stroke);
    }
}

fn color(c: DiagramColor) -> Ink {
    Ink { role: Role::Rgb(c.red as f32, c.green as f32, c.blue as f32), alpha: c.alpha as f32 }
}

/// The node's fill: the author's colour as a tint over the canvas, so pastels picked for a
/// white page read in dark mode too and the label stays legible.
fn fills(style: &ShapeStyle) -> (Ink, Option<Ink>, (Ink, f32, bool)) {
    let tint = style.fill.map(|f| color(f).opacity((f.alpha as f32).min(0.32)));
    let stroke = style.stroke.map(color).unwrap_or(Ink::text(0.22));
    (Ink::canvas(), tint, (stroke, style.stroke_width.unwrap_or(1.) as f32, style.dashed))
}

fn paint_flowchart(p: &mut dyn Canvas, g: &FlowchartGeometry) {
    for s in &g.subgraphs {
        let fill = s.subgraph.style.fill.map(|f| color(f).opacity((f.alpha as f32).min(0.18))).unwrap_or(Ink::text(0.035));
        let stroke = s.subgraph.style.stroke.map(color).unwrap_or(Ink::text(0.12));
        p.rounded(s.frame, 8., Some(fill), Some((stroke, 1., s.subgraph.style.dashed)));
    }
    let edge_color = Ink::text(0.5);
    for e in &g.edges {
        if e.edge.line == Line::Invisible || e.points.len() < 2 {
            continue;
        }
        let points: Vec<(f64, f64)> = e.points.iter().map(|q| (q.x, q.y)).collect();
        let width = if e.edge.line == Line::Thick { 2.5 } else { 1.2 };
        p.polyline(&points, edge_color, width, e.edge.line == Line::Dotted);
        let n = points.len();
        end_mark(p, e.edge.end, points[n - 2], points[n - 1], edge_color);
        end_mark(p, e.edge.start, points[1], points[0], edge_color);
        if let (Some(frame), false) = (e.label_frame, e.label.is_empty()) {
            p.rounded(frame, 4., Some(Ink::card()), None);
        }
    }
    for n in &g.nodes {
        let (base, tint, stroke) = fills(&n.node.style);
        let r = n.frame;
        let draw = |p: &mut dyn Canvas, fill: Ink, stroke: Option<(Ink, f32, bool)>| match n.node.shape {
            Shape::Rectangle => p.rounded(r, 2., Some(fill), stroke),
            Shape::Rounded => p.rounded(r, 8., Some(fill), stroke),
            Shape::Stadium => p.rounded(r, r.height / 2., Some(fill), stroke),
            Shape::Subroutine => {
                p.rounded(r, 2., Some(fill), stroke);
                if let Some((c, w, _)) = stroke {
                    p.polyline(&[(r.x + 8., r.y), (r.x + 8., r.max_y())], c, w, false);
                    p.polyline(&[(r.max_x() - 8., r.y), (r.max_x() - 8., r.max_y())], c, w, false);
                }
            }
            Shape::Cylinder => {
                let ry = (r.height / 6.).min(8.);
                p.rounded(Rect::new(r.x, r.y + ry, r.width, r.height - 2. * ry), 0., Some(fill), None);
                p.ellipse(Rect::new(r.x, r.max_y() - 2. * ry, r.width, 2. * ry), Some(fill), stroke);
                p.rounded(Rect::new(r.x, r.y + ry, r.width, r.height - 2. * ry), 0., Some(fill), None);
                if let Some((c, w, d)) = stroke {
                    p.polyline(&[(r.x, r.y + ry), (r.x, r.max_y() - ry)], c, w, d);
                    p.polyline(&[(r.max_x(), r.y + ry), (r.max_x(), r.max_y() - ry)], c, w, d);
                }
                p.ellipse(Rect::new(r.x, r.y, r.width, 2. * ry), Some(fill), stroke);
            }
            Shape::Circle => p.ellipse(r, Some(fill), stroke),
            Shape::DoubleCircle => {
                p.ellipse(r, Some(fill), stroke);
                p.ellipse(r.inset(4., 4.), None, stroke);
            }
            Shape::Diamond => p.polygon(&[(r.mid_x(), r.y), (r.max_x(), r.mid_y()), (r.mid_x(), r.max_y()), (r.x, r.mid_y())], Some(fill), stroke),
            Shape::Hexagon => {
                let d = (r.height / 2.).min(r.width / 4.);
                p.polygon(
                    &[(r.x + d, r.y), (r.max_x() - d, r.y), (r.max_x(), r.mid_y()), (r.max_x() - d, r.max_y()), (r.x + d, r.max_y()), (r.x, r.mid_y())],
                    Some(fill),
                    stroke,
                )
            }
            Shape::Parallelogram => {
                let d = (r.height * 0.35).min(r.width / 4.);
                p.polygon(&[(r.x + d, r.y), (r.max_x(), r.y), (r.max_x() - d, r.max_y()), (r.x, r.max_y())], Some(fill), stroke)
            }
            Shape::ParallelogramAlt => {
                let d = (r.height * 0.35).min(r.width / 4.);
                p.polygon(&[(r.x, r.y), (r.max_x() - d, r.y), (r.max_x(), r.max_y()), (r.x + d, r.max_y())], Some(fill), stroke)
            }
            Shape::Trapezoid => {
                let d = (r.height * 0.35).min(r.width / 4.);
                p.polygon(&[(r.x + d, r.y), (r.max_x() - d, r.y), (r.max_x(), r.max_y()), (r.x, r.max_y())], Some(fill), stroke)
            }
            Shape::TrapezoidAlt => {
                let d = (r.height * 0.35).min(r.width / 4.);
                p.polygon(&[(r.x, r.y), (r.max_x(), r.y), (r.max_x() - d, r.max_y()), (r.x + d, r.max_y())], Some(fill), stroke)
            }
            Shape::Flag => {
                let d = (r.height / 3.).min(r.width / 4.);
                p.polygon(&[(r.x, r.y), (r.max_x(), r.y), (r.max_x(), r.max_y()), (r.x, r.max_y()), (r.x + d, r.mid_y())], Some(fill), stroke)
            }
        };
        draw(p, base, None);
        if let Some(tint) = tint {
            draw(p, tint, None);
        }
        // The outline alone: fill none, drawn over.
        draw(p, Ink::clear(), Some(stroke));
    }
}

/// An arrowhead, circle or cross at the end of a line coming from `from` to `to`.
fn end_mark(p: &mut dyn Canvas, mark: Arrow, from: (f64, f64), to: (f64, f64), c: Ink) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    match mark {
        Arrow::None => {}
        Arrow::Arrow => head(p, to, (ux, uy), c, true),
        Arrow::Circle => p.ellipse(Rect::new(to.0 - ux * 4. - 4., to.1 - uy * 4. - 4., 8., 8.), Some(c), None),
        Arrow::Cross => {
            let (cx, cy) = (to.0 - ux * 5., to.1 - uy * 5.);
            let (nx, ny) = (-uy, ux);
            let s = 4.;
            p.polyline(&[(cx - (ux + nx) * s, cy - (uy + ny) * s), (cx + (ux + nx) * s, cy + (uy + ny) * s)], c, 1.5, false);
            p.polyline(&[(cx - (ux - nx) * s, cy - (uy - ny) * s), (cx + (ux - nx) * s, cy + (uy - ny) * s)], c, 1.5, false);
        }
    }
}

/// A head at `tip` pointing along `(ux, uy)`: filled, or two open strokes.
fn head(p: &mut dyn Canvas, tip: (f64, f64), (ux, uy): (f64, f64), c: Ink, filled: bool) {
    let (length, half) = (9., 4.5);
    let base = (tip.0 - ux * length, tip.1 - uy * length);
    let (nx, ny) = (-uy, ux);
    let a = (base.0 + nx * half, base.1 + ny * half);
    let b = (base.0 - nx * half, base.1 - ny * half);
    if filled {
        p.polygon(&[tip, a, b], Some(c), None);
    } else {
        p.polyline(&[a, tip, b], c, 1.3, false);
    }
}

fn paint_sequence(p: &mut dyn Canvas, g: &SequenceGeometry) {
    let line = Ink::text(0.55);
    for b in &g.blocks {
        p.rounded(b.frame, 6., Some(Ink::text(0.035)), Some((Ink::text(0.18), 1., false)));
        let tab = Rect::new(b.frame.x, b.frame.y, tab_width(b.kind.name()), LINE_HEIGHT + 6.);
        p.rounded(tab, 4., Some(Ink::text(0.06)), None);
        for section in b.sections.iter().skip(1) {
            p.polyline(&[(b.frame.x, section.y), (b.frame.max_x(), section.y)], Ink::text(0.25), 1., true);
        }
    }
    for part in &g.participants {
        p.polyline(&[(part.center_x, g.lifeline_top), (part.center_x, g.lifeline_bottom)], Ink::text(0.18), 1., true);
    }
    for a in &g.activations {
        p.rounded(*a, 1., Some(Ink::card()), Some((Ink::text(0.3), 1., false)));
    }
    for part in &g.participants {
        let h = part.header;
        if part.participant.is_actor {
            // A stick figure over the name.
            let (x, top) = (part.center_x, h.y + 1.);
            let c = Ink::text(0.6);
            p.ellipse(Rect::new(x - 3.5, top, 7., 7.), None, Some((c, 1.2, false)));
            p.polyline(&[(x, top + 7.), (x, top + 12.5)], c, 1.2, false);
            p.polyline(&[(x - 5., top + 9.), (x + 5., top + 9.)], c, 1.2, false);
            p.polyline(&[(x - 4., top + 17.), (x, top + 12.5), (x + 4., top + 17.)], c, 1.2, false);
            let below = Rect::new(h.x, h.y + 18., h.width, h.height - 18.);
            p.rounded(below, 6., Some(Ink::canvas()), Some((Ink::text(0.22), 1., false)));
        } else {
            p.rounded(h, 6., Some(Ink::canvas()), Some((Ink::text(0.22), 1., false)));
        }
    }
    let note_fill: Ink = Ink::rgb(1., 0.8, 0.).opacity(0.16);
    let note_stroke: Ink = Ink::rgb(1., 0.8, 0.).opacity(0.55);
    for note in &g.notes {
        p.rounded(note.frame, 3., Some(note_fill), Some((note_stroke, 1., false)));
    }
    for m in &g.messages {
        let dotted = m.message.line == LineStyle::Dotted;
        let filled = m.message.head == ArrowHead::Filled;
        let mark = |p: &mut dyn Canvas, tip: (f64, f64), dir: (f64, f64)| match m.message.head {
            ArrowHead::None => {}
            ArrowHead::Filled | ArrowHead::Open => head(p, tip, dir, line, filled),
            ArrowHead::Cross => {
                let (x, y) = (tip.0 - dir.0 * 5., tip.1);
                p.polyline(&[(x - 4., y - 4.), (x + 4., y + 4.)], line, 1.5, false);
                p.polyline(&[(x - 4., y + 4.), (x + 4., y - 4.)], line, 1.5, false);
            }
        };
        if m.is_self {
            let (x, y, w) = (m.from_x, m.y, 32.);
            p.polyline(&[(x, y), (x + w, y), (x + w, y + 20.), (x, y + 20.)], line, 1.2, dotted);
            mark(p, (x, y + 20.), (-1., 0.));
        } else {
            p.polyline(&[(m.from_x, m.y), (m.to_x, m.y)], line, 1.2, dotted);
            let dir = if m.to_x >= m.from_x { 1. } else { -1. };
            mark(p, (m.to_x, m.y), (dir, 0.));
            if m.message.bidirectional {
                mark(p, (m.from_x, m.y), (-dir, 0.));
            }
        }
        if m.number.is_some() {
            p.ellipse(Rect::new(m.from_x - 8., m.y - 8., 16., 16.), Some(Ink::accent()), None);
        }
    }
}

