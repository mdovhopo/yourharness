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
                .child(button("diagram-copy", "Copy Source").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))))
                .when(!showing_source, |d| {
                    let (big, copy, save) = (drawn.clone(), drawn.clone(), drawn.clone());
                    let theme = *theme;
                    d.child(button("diagram-copy-image", "Copy Image").on_click(move |_, _, cx| copy_image(copy.clone(), theme, cx)))
                        .child(button("diagram-save", "Save as PNG…").on_click(move |_, _, cx| save_png(save.clone(), theme, None, cx)))
                        .child(button("diagram-large", "Open Large").on_click(move |_, _, cx| open_large(big.clone(), cx)))
                }),
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
                    .child(shapes(drawn.clone(), *theme, 1.))
                    .children(label_elements(&drawn.geometry, theme, 1.)),
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

/// A piece of text over the diagram: its lines from (`x`, `y`), centred across `width` when
/// it has one. Drawn as elements in the window, as `<text>` in an exported image.
struct Label {
    lines: Vec<String>,
    x: f64,
    y: f64,
    width: Option<f64>,
    size: f32,
    line_height: f64,
    weight: FontWeight,
    color: Hsla,
}

/// A label's lines, centred in `frame` (with some room either side, as words may measure a
/// little wider than the layout thought).
fn centred(text: &WrappedText, frame: Rect, color: Hsla, bold: bool) -> Label {
    let top = frame.y + frame.height / 2. - text.lines.len() as f64 * LINE_HEIGHT / 2.;
    Label {
        lines: text.lines.clone(),
        x: frame.x - 20.,
        y: top,
        width: Some(frame.width + 40.),
        size: FONT_SIZE,
        line_height: LINE_HEIGHT,
        weight: if bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL },
        color,
    }
}

/// Text at a point, left-aligned.
fn at(text: String, x: f64, y: f64, color: Hsla, size: f32, bold: bool) -> Label {
    Label { lines: vec![text], x, y, width: None, size, line_height: LINE_HEIGHT, weight: if bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, color }
}

fn labels(geometry: &Geometry, theme: &Theme) -> Vec<Label> {
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
                        weight: FontWeight::MEDIUM,
                        color: theme.secondary,
                    });
                }
            }
            for n in &g.nodes {
                out.push(centred(&n.label, n.frame, theme.text, false));
            }
            for e in &g.edges {
                if let (Some(frame), false) = (e.label_frame, e.label.is_empty()) {
                    out.push(centred(&e.label, frame, theme.secondary, false));
                }
            }
        }
        Geometry::Sequence(g) => {
            for p in &g.participants {
                let mut frame = p.header;
                if p.participant.is_actor {
                    frame = Rect::new(frame.x, frame.y + 18., frame.width, frame.height - 18.);
                }
                out.push(centred(&p.label, frame, theme.text, true));
            }
            for m in &g.messages {
                if !m.label.is_empty() {
                    out.push(centred(&m.label, m.label_frame, theme.text, false));
                }
                if let Some(number) = m.number {
                    out.push(Label {
                        lines: vec![number.to_string()],
                        x: m.from_x - 8.,
                        y: m.y - 8.,
                        width: Some(16.),
                        size: 9.,
                        line_height: 16.,
                        weight: FontWeight::BOLD,
                        color: gpui_kit::white(),
                    });
                }
            }
            for note in &g.notes {
                out.push(centred(&note.text, note.frame, theme.text, false));
            }
            for b in &g.blocks {
                out.push(at(b.kind.name().to_string(), b.frame.x + 8., b.frame.y + 3., theme.secondary, 11.5, true));
                let tab = tab_width(b.kind.name());
                for (i, section) in b.sections.iter().enumerate() {
                    let text = section.label.lines.join(" ");
                    if text.is_empty() {
                        continue;
                    }
                    let x = if i == 0 { b.frame.x + tab + 8. } else { b.frame.x + 8. };
                    out.push(at(format!("[{text}]"), x, section.y + 3., theme.secondary, 11.5, false));
                }
            }
        }
    }
    out
}

/// The labels as elements laid over the canvas, `scale` times their size.
fn label_elements(geometry: &Geometry, theme: &Theme, scale: f32) -> Vec<AnyElement> {
    labels(geometry, theme)
        .into_iter()
        .map(|l| {
            let s = scale as f64;
            div()
                .absolute()
                .left(px(((l.x + PADDING) * s) as f32))
                .top(px(((l.y + PADDING) * s) as f32))
                .when_some(l.width, |d, w| d.w(px((w * s) as f32)).items_center())
                .flex()
                .flex_col()
                .text_size(px(l.size * scale))
                .line_height(px((l.line_height * s) as f32))
                .font_weight(l.weight)
                .text_color(l.color)
                .children(l.lines.into_iter().map(|line| div().whitespace_nowrap().child(SharedString::from(line))))
                .into_any_element()
        })
        .collect()
}

fn tab_width(name: &str) -> f64 {
    name.len() as f64 * 7. + 16.
}

/// The diagram's lines and shapes, painted from its layout, `scale` times its size.
fn shapes(drawn: Rc<Drawn>, theme: Theme, scale: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut p = Painter { origin: bounds.origin, scale, window };
            match &drawn.geometry {
                Geometry::Flowchart(g) => paint_flowchart(&mut p, g, &theme),
                Geometry::Sequence(g) => paint_sequence(&mut p, g, &theme),
            }
        },
    )
    .absolute()
    .size_full()
}

/// A path in the diagram's own coordinates.
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    /// An elliptical arc (radii), clockwise, to a point.
    Arc((f64, f64), (f64, f64)),
    Close,
}

/// Where the shapes go: the window (`Painter`) or an SVG (`Svg`, for Copy Image and Save as PNG).
trait Canvas {
    /// Fills, then outlines (colour, width, dashed), a path.
    fn path(&mut self, segs: &[Seg], fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>);

    fn polyline(&mut self, points: &[(f64, f64)], color: Hsla, width: f32, dashed: bool) {
        if points.len() < 2 {
            return;
        }
        let mut segs = vec![Seg::Move(points[0].0, points[0].1)];
        segs.extend(points[1..].iter().map(|&(x, y)| Seg::Line(x, y)));
        self.path(&segs, None, Some((color, width, dashed)));
    }

    fn polygon(&mut self, points: &[(f64, f64)], fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        if points.is_empty() {
            return;
        }
        let mut segs = vec![Seg::Move(points[0].0, points[0].1)];
        segs.extend(points[1..].iter().map(|&(x, y)| Seg::Line(x, y)));
        segs.push(Seg::Close);
        self.path(&segs, fill, stroke);
    }

    fn rounded(&mut self, r: Rect, radius: f64, fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
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

    fn ellipse(&mut self, r: Rect, fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        let k = (r.width / 2., r.height / 2.);
        let segs = [Seg::Move(r.x, r.mid_y()), Seg::Arc(k, (r.max_x(), r.mid_y())), Seg::Arc(k, (r.x, r.mid_y())), Seg::Close];
        self.path(&segs, fill, stroke);
    }
}

struct Painter<'a> {
    origin: Point<Pixels>,
    scale: f32,
    window: &'a mut Window,
}

impl Painter<'_> {
    fn at(&self, x: f64, y: f64) -> Point<Pixels> {
        let s = self.scale as f64;
        point(self.origin.x + px(((x + PADDING) * s) as f32), self.origin.y + px(((y + PADDING) * s) as f32))
    }

    fn trace(&self, b: &mut PathBuilder, segs: &[Seg]) {
        for seg in segs {
            match *seg {
                Seg::Move(x, y) => b.move_to(self.at(x, y)),
                Seg::Line(x, y) => b.line_to(self.at(x, y)),
                Seg::Arc((rx, ry), (x, y)) => {
                    let radii = point(px(rx as f32 * self.scale), px(ry as f32 * self.scale));
                    b.arc_to(radii, px(0.), false, true, self.at(x, y))
                }
                Seg::Close => b.close(),
            }
        }
    }
}

impl Canvas for Painter<'_> {
    fn path(&mut self, segs: &[Seg], fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        if let Some(fill) = fill {
            let mut b = PathBuilder::fill();
            self.trace(&mut b, segs);
            if let Ok(path) = b.build() {
                self.window.paint_path(path, fill);
            }
        }
        if let Some((color, width, dashed)) = stroke {
            let mut b = PathBuilder::stroke(px(width * self.scale));
            if dashed {
                b = b.dash_array(&[px(4. * self.scale), px(3. * self.scale)]);
            }
            self.trace(&mut b, segs);
            if let Ok(path) = b.build() {
                self.window.paint_path(path, color);
            }
        }
    }
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

fn paint_flowchart(p: &mut dyn Canvas, g: &FlowchartGeometry, theme: &Theme) {
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
        let draw = |p: &mut dyn Canvas, fill: Hsla, stroke: Option<(Hsla, f32, bool)>| match n.node.shape {
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
fn end_mark(p: &mut dyn Canvas, mark: Arrow, from: (f64, f64), to: (f64, f64), c: Hsla) {
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
fn head(p: &mut dyn Canvas, tip: (f64, f64), (ux, uy): (f64, f64), c: Hsla, filled: bool) {
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

fn paint_sequence(p: &mut dyn Canvas, g: &SequenceGeometry, theme: &Theme) {
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
            p.ellipse(Rect::new(m.from_x - 8., m.y - 8., 16., 16.), Some(theme.accent), None);
        }
    }
}

// MARK: - Copy Image, Save as PNG

/// Shapes as SVG paths, for an image of the diagram.
struct Svg {
    out: String,
}

fn svg_color(c: Hsla) -> (String, f32) {
    let rgba = c.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    (format!("#{:02x}{:02x}{:02x}", byte(rgba.r), byte(rgba.g), byte(rgba.b)), rgba.a)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl Canvas for Svg {
    fn path(&mut self, segs: &[Seg], fill: Option<Hsla>, stroke: Option<(Hsla, f32, bool)>) {
        use std::fmt::Write;
        let mut d = String::new();
        for seg in segs {
            let _ = match *seg {
                Seg::Move(x, y) => write!(d, "M{:.2} {:.2} ", x + PADDING, y + PADDING),
                Seg::Line(x, y) => write!(d, "L{:.2} {:.2} ", x + PADDING, y + PADDING),
                Seg::Arc((rx, ry), (x, y)) => write!(d, "A{rx:.2} {ry:.2} 0 0 1 {:.2} {:.2} ", x + PADDING, y + PADDING),
                Seg::Close => write!(d, "Z "),
            };
        }
        let fill = match fill {
            Some(c) => {
                let (hex, a) = svg_color(c);
                format!("fill=\"{hex}\" fill-opacity=\"{a:.3}\"")
            }
            None => "fill=\"none\"".into(),
        };
        let stroke = match stroke {
            Some((c, w, dashed)) => {
                let (hex, a) = svg_color(c);
                format!(
                    " stroke=\"{hex}\" stroke-opacity=\"{a:.3}\" stroke-width=\"{w}\" stroke-linejoin=\"round\"{}",
                    if dashed { " stroke-dasharray=\"4 3\"" } else { "" }
                )
            }
            None => String::new(),
        };
        let _ = writeln!(self.out, "<path d=\"{}\" {fill}{stroke}/>", d.trim_end());
    }
}

/// The diagram as an SVG on the canvas colour, labels as text.
fn svg_of(drawn: &Drawn, theme: &Theme) -> String {
    use std::fmt::Write;
    let size = drawn.geometry.size();
    let (w, h) = (size.width + PADDING * 2., size.height + PADDING * 2.);
    let (bg, _) = svg_color(theme.canvas);
    let mut svg = Svg { out: format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.2} {h:.2}\">\n<rect width=\"100%\" height=\"100%\" fill=\"{bg}\"/>\n") };
    match &drawn.geometry {
        Geometry::Flowchart(g) => paint_flowchart(&mut svg, g, theme),
        Geometry::Sequence(g) => paint_sequence(&mut svg, g, theme),
    }
    for l in labels(&drawn.geometry, theme) {
        let (hex, a) = svg_color(l.color);
        let (x, anchor) = match l.width {
            Some(width) => (l.x + width / 2., "middle"),
            None => (l.x, "start"),
        };
        for (i, line) in l.lines.iter().enumerate() {
            // The baseline, about where the window's text sits in its line.
            let baseline = l.y + i as f64 * l.line_height + l.line_height / 2. + l.size as f64 * 0.35;
            let _ = writeln!(
                svg.out,
                "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"Helvetica Neue, Helvetica, Arial, sans-serif\" font-size=\"{}\" font-weight=\"{}\" text-anchor=\"{anchor}\" fill=\"{hex}\" fill-opacity=\"{a:.3}\">{}</text>",
                x + PADDING,
                baseline + PADDING,
                l.size,
                l.weight.0 as i32,
                escape(line)
            );
        }
    }
    svg.out.push_str("</svg>\n");
    svg.out
}

/// An SVG drawn at twice its size, as PNG bytes. Slow (fonts are loaded): off the main thread.
fn png_of(svg: &str) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    let size = tree.size().to_int_size().scale_by(2.).ok_or("too big")?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("too big")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(2., 2.), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Copy Image: the diagram on the clipboard as a PNG.
fn copy_image(drawn: Rc<Drawn>, theme: Theme, cx: &mut App) {
    let svg = svg_of(&drawn, &theme);
    cx.spawn(async move |cx| {
        let png = cx.background_executor().spawn(async move { png_of(&svg) }).await;
        let _ = cx.update(|cx| match png {
            Ok(bytes) => cx.write_to_clipboard(ClipboardItem::new_image(&gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, bytes))),
            Err(e) => eprintln!("diagram image: {e}"),
        });
    })
    .detach();
}

/// Save as PNG: where you pick (or `to`, for test runs), at twice its size.
fn save_png(drawn: Rc<Drawn>, theme: Theme, to: Option<std::path::PathBuf>, cx: &mut App) {
    let svg = svg_of(&drawn, &theme);
    let name = format!("{}.png", drawn.kind);
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let asked = to.is_none().then(|| cx.prompt_for_new_path(&home.join("Desktop"), Some(&name)));
    cx.spawn(async move |cx| {
        let path = match (to, asked) {
            (Some(path), _) => path,
            (None, Some(asked)) => match asked.await {
                Ok(Ok(Some(path))) => path,
                _ => return,
            },
            _ => return,
        };
        let done = cx
            .background_executor()
            .spawn(async move {
                let bytes = png_of(&svg)?;
                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                Ok::<_, String>(path)
            })
            .await;
        match done {
            Ok(path) => eprintln!("DIAGRAM saved {}", path.display()),
            Err(e) => eprintln!("DIAGRAM save failed: {e}"),
        }
    })
    .detach();
}

// MARK: - Open Large

/// A diagram in a window of its own: zoom in and out, fit it to the window, copy or save it.
pub struct DiagramWindow {
    drawn: Rc<Drawn>,
    zoom: f32,
    /// Sized to the window (as it opens, and after Fit).
    fit: bool,
}

const ZOOMS: (f32, f32) = (0.25, 4.);

impl DiagramWindow {
    fn natural(&self) -> (f32, f32) {
        let size = self.drawn.geometry.size();
        ((size.width + PADDING * 2.) as f32, (size.height + PADDING * 2.) as f32)
    }

    /// What it's drawn at: fitted to the room the window gives it, or as zoomed.
    fn scale(&self, window: &Window) -> f32 {
        if !self.fit {
            return self.zoom;
        }
        let (w, h) = self.natural();
        let room = window.viewport_size();
        let (rw, rh) = (f32::from(room.width) - 48., f32::from(room.height) - TOOLBAR - 48.);
        (rw / w).min(rh / h).clamp(ZOOMS.0, 2.)
    }

    fn zoom_by(&mut self, factor: f32, window: &Window, cx: &mut Context<Self>) {
        self.zoom = (self.scale(window) * factor).clamp(ZOOMS.0, ZOOMS.1);
        self.fit = false;
        cx.notify();
    }
}

const TOOLBAR: f32 = 40.;

impl Render for DiagramWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let scale = self.scale(window);
        let (w, h) = self.natural();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(3.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(theme.text)
                .cursor_pointer()
                .hover(|s| s.bg(theme.selected))
                .child(label)
        };
        let (copy, save) = (self.drawn.clone(), self.drawn.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.canvas)
            .text_color(theme.text)
            .child(
                div()
                    .h(px(TOOLBAR))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(div().flex_1().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(self.drawn.kind.clone()))
                    .child(button("zoom-out", "−".into()).on_click(cx.listener(|this, _, window, cx| this.zoom_by(1. / 1.25, window, cx))))
                    .child(div().w(px(44.)).flex().justify_center().text_size(px(12.)).text_color(theme.secondary).child(format!("{:.0}%", scale * 100.)))
                    .child(button("zoom-in", "+".into()).on_click(cx.listener(|this, _, window, cx| this.zoom_by(1.25, window, cx))))
                    .child(button("zoom-fit", "Fit".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.fit = true;
                        cx.notify();
                    })))
                    .child(button("zoom-actual", "Actual Size".into()).on_click(cx.listener(|this, _, _, cx| {
                        this.fit = false;
                        this.zoom = 1.;
                        cx.notify();
                    })))
                    .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(theme.separator))
                    .child(button("copy-image", "Copy Image".into()).on_click(move |_, _, cx| copy_image(copy.clone(), theme, cx)))
                    .child(button("save-png", "Save as PNG…".into()).on_click(move |_, _, cx| save_png(save.clone(), theme, None, cx))),
            )
            .child(
                div().id("diagram-body").flex_1().min_h_0().overflow_scroll().child(
                    // Centred while it's smaller than the window; scrolled when it's bigger.
                    div().min_w_full().min_h_full().flex().items_center().justify_center().p(px(24.)).child(
                        div()
                            .relative()
                            .flex_none()
                            .w(px(w * scale))
                            .h(px(h * scale))
                            .child(shapes(self.drawn.clone(), theme, scale))
                            .children(label_elements(&self.drawn.geometry, &theme, scale)),
                    ),
                ),
            )
    }
}

/// Opens the diagram in its own window, fitted to it.
fn open_large(drawn: Rc<Drawn>, cx: &mut App) {
    use gpui_kit::{size, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
    let s = drawn.geometry.size();
    let (w, h) = (((s.width + PADDING * 2.) as f32 + 48.).clamp(480., 1400.), ((s.height + PADDING * 2.) as f32 + TOOLBAR + 48.).clamp(320., 900.));
    let background = std::env::var("YH_BACKGROUND").as_deref() == Ok("1");
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(w), px(h)), cx))),
        titlebar: Some(TitlebarOptions { title: Some(drawn.kind.clone().into()), ..Default::default() }),
        focus: !background,
        window_min_size: Some(size(px(360.), px(240.))),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        if background {
            crate::platform::order_back(window);
        }
        cx.new(|_| DiagramWindow { drawn, zoom: 1., fit: true })
    });
    let Ok((handle, _)) = opened else { return eprintln!("DIAGRAM couldn't open a window") };
    // `YH_DIAGRAM_SNAPSHOT=<png>` (test runs): the window drawn into a file a moment after.
    if let Ok(file) = std::env::var("YH_DIAGRAM_SNAPSHOT") {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = handle.update(cx, |_, window, _| crate::snapshot(window, &file));
        })
        .detach();
    }
}

/// `YH_OPEN_DIAGRAM=<file.mmd>` (test runs): the file's diagram in the large window, as Open
/// Large does; with `YH_DIAGRAM_EXPORT=<file.png>` also saved there, as Save as PNG does.
pub fn open_file(path: &str, window: &Window, cx: &mut App) {
    let Ok(source) = std::fs::read_to_string(path) else { return eprintln!("DIAGRAM can't read {path}") };
    let Some(drawn) = drawn(&source, window) else { return eprintln!("DIAGRAM doesn't parse: {path}") };
    if let Ok(to) = std::env::var("YH_DIAGRAM_EXPORT") {
        save_png(drawn.clone(), Theme::get(cx), Some(to.into()), cx);
    }
    open_large(drawn, cx);
}
