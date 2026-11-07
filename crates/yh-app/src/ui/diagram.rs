//! A ```mermaid block in a reply, drawn natively: shapes, edges and lifelines on a canvas,
//! labels as text laid over it. Parsed and laid out once per source (`DIAGRAMS`), never per
//! frame; Show Source swaps in the code, Copy Source copies it. A diagram that doesn't parse
//! shows as code.

use crate::theme::{Theme, MONO};
use crate::ui::rows::RowContext;
use gpui_kit::{
    canvas, div, point, prelude::*, px, AnyElement, App, ClipboardItem, FontWeight, Hsla, PathBuilder, Pixels, Point,
    SharedString, Window,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use yh_core::diagrams::{
    self, Arrow, ArrowHead, DiagramColor, FlowchartGeometry, Geometry, Line, LineStyle, Rect, SequenceGeometry, Shape, ShapeStyle,
    TextMetrics, WrappedText,
};

const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f64 = 17.;
const PADDING: f64 = 16.;
/// How many laid-out diagrams are kept.
const KEPT: usize = 48;

struct Drawn {
    geometry: Geometry,
    kind: String,
}

thread_local! {
    /// Diagrams by their source: laid out once, drawn from here every frame.
    static DIAGRAMS: std::cell::RefCell<(HashMap<u64, Option<Rc<Drawn>>>, VecDeque<u64>)> = Default::default();
    /// Diagrams shown as their source.
    static SOURCE_SHOWN: std::cell::RefCell<HashSet<u64>> = Default::default();
}

fn key(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Measures with the system font at the diagram's size: each character's advance, worked out
/// once and summed (the layout asks for many words).
fn metrics(window: &Window) -> TextMetrics {
    let system: std::sync::Arc<gpui_kit::TextSystem> = (**window.text_system()).clone();
    let font = system.resolve_font(&gpui_kit::font(crate::theme::UI));
    let cache = std::sync::Mutex::new(HashMap::<char, f64>::new());
    TextMetrics::new(LINE_HEIGHT, move |text| {
        let mut cache = cache.lock().unwrap();
        let width: f64 = text
            .chars()
            .map(|c| *cache.entry(c).or_insert_with(|| system.advance(font, px(FONT_SIZE), c).map_or(7.5, |s| f32::from(s.width) as f64)))
            .sum();
        width.ceil()
    })
}

fn drawn(source: &str, window: &Window) -> Option<Rc<Drawn>> {
    let k = key(source);
    if let Some(known) = DIAGRAMS.with(|d| d.borrow().0.get(&k).cloned()) {
        return known;
    }
    let made = diagrams::parse(source).ok().map(|diagram| {
        let kind = match &diagram {
            diagrams::Diagram::Flowchart(_) => "Flowchart".to_string(),
            diagrams::Diagram::Sequence(s) => s.title.clone().unwrap_or_else(|| "Sequence".into()),
        };
        Rc::new(Drawn { geometry: diagrams::layout(&diagram, &metrics(window)), kind })
    });
    DIAGRAMS.with(|d| {
        let (map, order) = &mut *d.borrow_mut();
        map.insert(k, made.clone());
        order.push_back(k);
        while order.len() > KEPT {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    });
    made
}

/// The block as a diagram, or None to show it as code (it doesn't parse, or the reply is
/// still being written and its fence may not be closed).
pub fn diagram_block(source: &str, ctx: &RowContext, n: usize, theme: &Theme, window: &Window) -> Option<AnyElement> {
    if ctx.live || !diagrams::is_supported(source) {
        return None;
    }
    let drawn = drawn(source, window)?;
    let k = key(source);
    let showing_source = SOURCE_SHOWN.with(|s| s.borrow().contains(&k));
    let (thread, pos) = (ctx.thread.clone(), ctx.pos);
    let toggle = move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
        SOURCE_SHOWN.with(|s| {
            let mut s = s.borrow_mut();
            if !s.remove(&k) {
                s.insert(k);
            }
        });
        thread.update(cx, |t, cx| {
            t.touch_pos(pos);
            cx.notify();
        });
    };
    let copied = source.to_string();
    let button = |id: &'static str, label: &'static str| {
        div()
            .id((id, (k as usize) ^ n))
            .px(px(6.))
            .rounded(px(5.))
            .text_size(px(11.))
            .text_color(theme.secondary)
            .cursor_pointer()
            .hover(|s| s.bg(theme.selected))
            .child(label)
    };
    let header = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(12.))
        .pt(px(8.))
        .child(div().flex_1().text_size(px(11.5)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(drawn.kind.clone()))
        .child(
            div()
                .flex()
                .gap(px(4.))
                .when(!showing_source, |d| d.opacity(0.).group_hover("diagram", |s| s.opacity(1.)))
                .child(button("diagram-source", if showing_source { "Show Diagram" } else { "Show Source" }).on_click(toggle))
                .child(button("diagram-copy", "Copy Source").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone())))),
        );
    let body: AnyElement = if showing_source {
        div()
            .px(px(12.))
            .pb(px(10.))
            .font_family(MONO)
            .text_size(px(12.))
            .line_height(px(17.))
            .child(source.to_string())
            .into_any_element()
    } else {
        let size = drawn.geometry.size();
        let (width, height) = (size.width + PADDING * 2., size.height + PADDING * 2.);
        // Wider than the column: it scrolls sideways, at full size.
        div()
            .id(("diagram-scroll", (k as usize) ^ n))
            .w_full()
            .overflow_x_scroll()
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(width as f32))
                    .h(px(height as f32))
                    .mx_auto()
                    .child(shapes(drawn.clone(), *theme))
                    .children(labels(&drawn.geometry, theme)),
            )
            .into_any_element()
    };
    Some(
        div()
            .group("diagram")
            .flex()
            .flex_col()
            .border_1()
            .border_color(theme.separator)
            .rounded(px(10.))
            .bg(theme.card)
            .child(header)
            .child(body)
            .into_any_element(),
    )
}

/// A label's lines, centred in `frame`, laid over the canvas.
fn text_lines(text: &WrappedText, frame: Rect, color: Hsla, size: f32, bold: bool) -> AnyElement {
    let top = frame.y + frame.height / 2. - text.lines.len() as f64 * LINE_HEIGHT / 2.;
    div()
        .absolute()
        .left(px((frame.x + PADDING) as f32 - 20.))
        .top(px((top + PADDING) as f32))
        .w(px(frame.width as f32 + 40.))
        .flex()
        .flex_col()
        .items_center()
        .text_size(px(size))
        .line_height(px(LINE_HEIGHT as f32))
        .text_color(color)
        .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
        .children(text.lines.iter().map(|l| div().whitespace_nowrap().child(SharedString::from(l.clone()))))
        .into_any_element()
}

/// Text at a point, left-aligned.
fn text_at(text: impl Into<SharedString>, x: f64, y: f64, color: Hsla, size: f32, bold: bool) -> AnyElement {
    div()
        .absolute()
        .left(px((x + PADDING) as f32))
        .top(px((y + PADDING) as f32))
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(px(LINE_HEIGHT as f32))
        .text_color(color)
        .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
        .child(text.into())
        .into_any_element()
}

fn labels(geometry: &Geometry, theme: &Theme) -> Vec<AnyElement> {
    let mut out = vec![];
    match geometry {
        Geometry::Flowchart(g) => {
            for s in &g.subgraphs {
                if !s.title.is_empty() {
                    let frame = Rect::new(s.frame.x + 12., s.frame.y + 6., s.title.size.width, s.title.size.height);
                    out.push(text_lines_left(&s.title, frame, theme.secondary));
                }
            }
            for n in &g.nodes {
                out.push(text_lines(&n.label, n.frame, theme.text, FONT_SIZE, false));
            }
            for e in &g.edges {
                if let (Some(frame), false) = (e.label_frame, e.label.is_empty()) {
                    out.push(text_lines(&e.label, frame, theme.secondary, FONT_SIZE, false));
                }
            }
        }
        Geometry::Sequence(g) => {
            for p in &g.participants {
                let mut frame = p.header;
                if p.participant.is_actor {
                    frame = Rect::new(frame.x, frame.y + 18., frame.width, frame.height - 18.);
                }
                out.push(text_lines(&p.label, frame, theme.text, FONT_SIZE, true));
            }
            for m in &g.messages {
                if !m.label.is_empty() {
                    out.push(text_lines(&m.label, m.label_frame, theme.text, FONT_SIZE, false));
                }
                if let Some(number) = m.number {
                    out.push(
                        div()
                            .absolute()
                            .left(px((m.from_x + PADDING) as f32 - 8.))
                            .top(px((m.y + PADDING) as f32 - 8.))
                            .w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui_kit::white())
                            .child(number.to_string())
                            .into_any_element(),
                    );
                }
            }
            for note in &g.notes {
                out.push(text_lines(&note.text, note.frame, theme.text, FONT_SIZE, false));
            }
            for b in &g.blocks {
                out.push(text_at(b.kind.name(), b.frame.x + 8., b.frame.y + 3., theme.secondary, 11.5, true));
                let tab = tab_width(b.kind.name());
                for (i, section) in b.sections.iter().enumerate() {
                    let text = section.label.lines.join(" ");
                    if text.is_empty() {
                        continue;
                    }
                    let x = if i == 0 { b.frame.x + tab + 8. } else { b.frame.x + 8. };
                    out.push(text_at(format!("[{text}]"), x, section.y + 3., theme.secondary, 11.5, false));
                }
            }
        }
    }
    out
}

fn tab_width(name: &str) -> f64 {
    name.len() as f64 * 7. + 16.
}

/// A subgraph's title: left-aligned from its corner.
fn text_lines_left(text: &WrappedText, frame: Rect, color: Hsla) -> AnyElement {
    div()
        .absolute()
        .left(px((frame.x + PADDING) as f32))
        .top(px((frame.y + PADDING) as f32))
        .flex()
        .flex_col()
        .text_size(px(12.))
        .line_height(px(LINE_HEIGHT as f32))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .children(text.lines.iter().map(|l| div().whitespace_nowrap().child(SharedString::from(l.clone()))))
        .into_any_element()
}

/// The diagram's lines and shapes, painted from its layout.
fn shapes(drawn: Rc<Drawn>, theme: Theme) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut p = Painter { origin: bounds.origin, window };
            match &drawn.geometry {
                Geometry::Flowchart(g) => paint_flowchart(&mut p, g, &theme),
                Geometry::Sequence(g) => paint_sequence(&mut p, g, &theme),
            }
        },
    )
    .absolute()
    .size_full()
}

struct Painter<'a> {
    origin: Point<Pixels>,
    window: &'a mut Window,
}

impl Painter<'_> {
    fn at(&self, x: f64, y: f64) -> Point<Pixels> {
        point(self.origin.x + px((x + PADDING) as f32), self.origin.y + px((y + PADDING) as f32))
    }

    /// Fills, then outlines, what `trace` draws.
    fn shape(&mut self, fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>, trace: impl Fn(&Self, &mut PathBuilder)) {
        if let Some(fill) = fill {
            let mut b = PathBuilder::fill();
            trace(self, &mut b);
            if let Ok(path) = b.build() {
                self.window.paint_path(path, fill);
            }
        }
        if let Some((color, width, dashed)) = stroke {
            let mut b = PathBuilder::stroke(px(width));
            if dashed {
                b = b.dash_array(&[px(4.), px(3.)]);
            }
            trace(self, &mut b);
            if let Ok(path) = b.build() {
                self.window.paint_path(path, color);
            }
        }
    }

    fn polyline(&mut self, points: &[(f64, f64)], color: Hsla, width: f32, dashed: bool) {
        if points.len() < 2 {
            return;
        }
        self.shape(None, Some((color, width, dashed)), |p, b| {
            b.move_to(p.at(points[0].0, points[0].1));
            for &(x, y) in &points[1..] {
                b.line_to(p.at(x, y));
            }
        });
    }

    fn polygon(&mut self, points: &[(f64, f64)], fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        self.shape(fill, stroke, |p, b| {
            let pts: Vec<Point<Pixels>> = points.iter().map(|&(x, y)| p.at(x, y)).collect();
            b.add_polygon(&pts, true);
        });
    }

    fn rounded(&mut self, r: Rect, radius: f64, fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        let radius = radius.min(r.width / 2.).min(r.height / 2.).max(0.);
        self.shape(fill, stroke, |p, b| trace_rounded(p, b, r, radius));
    }

    fn ellipse(&mut self, r: Rect, fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        self.shape(fill, stroke, |p, b| {
            let radii = point(px((r.width / 2.) as f32), px((r.height / 2.) as f32));
            b.move_to(p.at(r.x, r.mid_y()));
            b.arc_to(radii, px(0.), false, true, p.at(r.max_x(), r.mid_y()));
            b.arc_to(radii, px(0.), false, true, p.at(r.x, r.mid_y()));
            b.close();
        });
    }
}

fn trace_rounded(p: &Painter, b: &mut PathBuilder, r: Rect, radius: f64) {
    if radius <= 0.5 {
        let pts = [p.at(r.x, r.y), p.at(r.max_x(), r.y), p.at(r.max_x(), r.max_y()), p.at(r.x, r.max_y())];
        b.add_polygon(&pts, true);
        return;
    }
    let radii = point(px(radius as f32), px(radius as f32));
    b.move_to(p.at(r.x + radius, r.y));
    b.line_to(p.at(r.max_x() - radius, r.y));
    b.arc_to(radii, px(0.), false, true, p.at(r.max_x(), r.y + radius));
    b.line_to(p.at(r.max_x(), r.max_y() - radius));
    b.arc_to(radii, px(0.), false, true, p.at(r.max_x() - radius, r.max_y()));
    b.line_to(p.at(r.x + radius, r.max_y()));
    b.arc_to(radii, px(0.), false, true, p.at(r.x, r.max_y() - radius));
    b.line_to(p.at(r.x, r.y + radius));
    b.arc_to(radii, px(0.), false, true, p.at(r.x + radius, r.y));
    b.close();
}

fn color(c: DiagramColor) -> Hsla {
    gpui_kit::Rgba { r: c.red as f32, g: c.green as f32, b: c.blue as f32, a: c.alpha as f32 }.into()
}

/// The node's fill: the author's colour as a tint over the canvas, so pastels picked for a
/// white page read in dark mode too and the label stays legible.
fn fills(style: &ShapeStyle, theme: &Theme) -> (Hsla, Option<Hsla>, (Hsla, f32, bool)) {
    let tint = style.fill.map(|f| color(f).opacity((f.alpha as f32).min(0.32)));
    let stroke = style.stroke.map(color).unwrap_or(theme.text.opacity(0.22));
    (theme.canvas, tint, (stroke, style.stroke_width.unwrap_or(1.) as f32, style.dashed))
}

fn paint_flowchart(p: &mut Painter, g: &FlowchartGeometry, theme: &Theme) {
    for s in &g.subgraphs {
        let fill = s.subgraph.style.fill.map(|f| color(f).opacity((f.alpha as f32).min(0.18))).unwrap_or(theme.text.opacity(0.035));
        let stroke = s.subgraph.style.stroke.map(color).unwrap_or(theme.text.opacity(0.12));
        p.rounded(s.frame, 8., Some(fill), Some((stroke, 1., s.subgraph.style.dashed)));
    }
    let edge_color = theme.text.opacity(0.5);
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
            p.rounded(frame, 4., Some(theme.card), None);
        }
    }
    for n in &g.nodes {
        let (base, tint, stroke) = fills(&n.node.style, theme);
        let r = n.frame;
        let draw = |p: &mut Painter, fill: Hsla, stroke: Option<(Hsla, f32, bool)>| match n.node.shape {
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
        draw(p, gpui_kit::transparent_black(), Some(stroke));
    }
}

/// An arrowhead, circle or cross at the end of a line coming from `from` to `to`.
fn end_mark(p: &mut Painter, mark: Arrow, from: (f64, f64), to: (f64, f64), c: Hsla) {
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
fn head(p: &mut Painter, tip: (f64, f64), (ux, uy): (f64, f64), c: Hsla, filled: bool) {
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

fn paint_sequence(p: &mut Painter, g: &SequenceGeometry, theme: &Theme) {
    let line = theme.text.opacity(0.55);
    for b in &g.blocks {
        p.rounded(b.frame, 6., Some(theme.text.opacity(0.035)), Some((theme.text.opacity(0.18), 1., false)));
        let tab = Rect::new(b.frame.x, b.frame.y, tab_width(b.kind.name()), LINE_HEIGHT + 6.);
        p.rounded(tab, 4., Some(theme.text.opacity(0.06)), None);
        for section in b.sections.iter().skip(1) {
            p.polyline(&[(b.frame.x, section.y), (b.frame.max_x(), section.y)], theme.text.opacity(0.25), 1., true);
        }
    }
    for part in &g.participants {
        p.polyline(&[(part.center_x, g.lifeline_top), (part.center_x, g.lifeline_bottom)], theme.text.opacity(0.18), 1., true);
    }
    for a in &g.activations {
        p.rounded(*a, 1., Some(theme.card), Some((theme.text.opacity(0.3), 1., false)));
    }
    for part in &g.participants {
        let h = part.header;
        if part.participant.is_actor {
            // A stick figure over the name.
            let (x, top) = (part.center_x, h.y + 1.);
            let c = theme.text.opacity(0.6);
            p.ellipse(Rect::new(x - 3.5, top, 7., 7.), None, Some((c, 1.2, false)));
            p.polyline(&[(x, top + 7.), (x, top + 12.5)], c, 1.2, false);
            p.polyline(&[(x - 5., top + 9.), (x + 5., top + 9.)], c, 1.2, false);
            p.polyline(&[(x - 4., top + 17.), (x, top + 12.5), (x + 4., top + 17.)], c, 1.2, false);
            let below = Rect::new(h.x, h.y + 18., h.width, h.height - 18.);
            p.rounded(below, 6., Some(theme.canvas), Some((theme.text.opacity(0.22), 1., false)));
        } else {
            p.rounded(h, 6., Some(theme.canvas), Some((theme.text.opacity(0.22), 1., false)));
        }
    }
    let note_fill: Hsla = gpui_kit::Hsla::from(gpui_kit::rgb(0xffcc00)).opacity(0.16);
    let note_stroke: Hsla = gpui_kit::Hsla::from(gpui_kit::rgb(0xffcc00)).opacity(0.55);
    for note in &g.notes {
        p.rounded(note.frame, 3., Some(note_fill), Some((note_stroke, 1., false)));
    }
    for m in &g.messages {
        let dotted = m.message.line == LineStyle::Dotted;
        let filled = m.message.head == ArrowHead::Filled;
        let mark = |p: &mut Painter, tip: (f64, f64), dir: (f64, f64)| match m.message.head {
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
            p.ellipse(Rect::new(m.from_x - 8., m.y - 8., 16., 16.), Some(theme.accent), None);
        }
    }
}
