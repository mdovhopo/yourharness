//! A ```mermaid block in a reply, drawn natively: shapes, edges and lifelines on a canvas,
//! labels as text laid over it. Parsed and laid out once per source (`DIAGRAMS`), never per
//! frame; Show Source swaps in the code, Copy Source copies it. A diagram that doesn't parse
//! shows as code.

use crate::theme::{Theme, MONO};
use crate::ui::rows::RowContext;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    canvas, div, point, prelude::*, px, AnyElement, App, ClipboardItem, FontWeight, Hsla, PathBuilder, Pixels, Point,
    SharedString, Window,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use yh_core::diagrams::paint::{self as draw, Canvas, Ink, Role, Seg, FONT_SIZE, LINE_HEIGHT};
use yh_core::diagrams::{self, Geometry, TextMetrics};

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

/// Lucide's code: Show Source (the default icon set lacks it).
fn code_icon() -> Icon {
    const SVG: &str = concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, r#"<path d="m16 18 6-6-6-6"/><path d="m8 6-6 6 6 6"/>"#, "</svg>");
    Icon::default().data(SVG.as_bytes())
}

/// Lucide's image: Copy Image.
fn image_icon() -> Icon {
    const SVG: &str = concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, r#"<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>"#, "</svg>");
    Icon::default().data(SVG.as_bytes())
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
    // Icon buttons with tooltips, as the thread's toolbar: five text labels crowded the header.
    let button = |id: &'static str, icon: Icon, tip: &'static str| crate::ui::thread_toolbar::icon_button((id, (k as usize) ^ n), icon, tip, theme);
    let just_copied = crate::ui::rows::just_copied("diagram-copy", (k, n));
    let (copy_thread, copy_pos) = (ctx.thread.clone(), ctx.pos);
    let header = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .pl(px(12.))
        .pr(px(4.))
        .border_b_1()
        .border_color(theme.separator)
        .child(div().flex_1().text_size(px(crate::theme::text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).child(drawn.kind.clone()))
        .child(
            div()
                .flex()
                .gap(px(2.))
                .when(!showing_source && !just_copied, |d| d.opacity(0.).group_hover("diagram", |s| s.opacity(1.)))
                .child(if showing_source {
                    button("diagram-source", Icon::new(IconName::Network), "Show Diagram").on_click(toggle)
                } else {
                    button("diagram-source", code_icon(), "Show Source").on_click(toggle)
                })
                .child(
                    button("diagram-copy", Icon::new(if just_copied { IconName::Check } else { IconName::Copy }), if just_copied { "Copied" } else { "Copy Source" }).on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        crate::ui::rows::mark_copied(crate::ui::rows::copy_hash("diagram-copy", (k, n)), &copy_thread, copy_pos, cx);
                    }),
                )
                .when(!showing_source, |d| {
                    let (big, copy, save) = (drawn.clone(), drawn.clone(), drawn.clone());
                    let theme = *theme;
                    d.child(button("diagram-copy-image", image_icon(), "Copy Image").on_click(move |_, _, cx| copy_image(copy.clone(), theme, cx)))
                        .child(button("diagram-save", crate::ui::icons::download(), "Save as PNG…").on_click(move |_, _, cx| save_png(save.clone(), theme, None, cx)))
                        .child(button("diagram-large", Icon::new(IconName::Maximize), "Open Large").on_click(move |_, _, cx| open_large(big.clone(), cx)))
                }),
        );
    let body: AnyElement = if showing_source {
        div()
            .px(px(12.))
            .py(px(10.))
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
            .rounded(px(crate::theme::radius::MEDIUM))
            .overflow_hidden()
            .bg(theme.code)
            .child(header)
            .child(body)
            .into_any_element(),
    )
}

/// The labels as elements laid over the canvas, `scale` times their size.
fn label_elements(geometry: &Geometry, theme: &Theme, scale: f32) -> Vec<AnyElement> {
    draw::labels(geometry)
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
                .font_weight(FontWeight(l.weight as f32))
                .text_color(hsla(l.color, theme))
                .children(l.lines.into_iter().map(|line| div().whitespace_nowrap().child(SharedString::from(line))))
                .into_any_element()
        })
        .collect()
}

/// The diagram's lines and shapes, painted from its layout, `scale` times its size.
fn shapes(drawn: Rc<Drawn>, theme: Theme, scale: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut p = Painter { origin: bounds.origin, scale, window, theme };
            draw::paint(&mut p, &drawn.geometry);
        },
    )
    .absolute()
    .size_full()
}

struct Painter<'a> {
    origin: Point<Pixels>,
    scale: f32,
    window: &'a mut Window,
    theme: Theme,
}

/// A role's colour in this theme, at its alpha.
fn hsla(ink: Ink, theme: &Theme) -> Hsla {
    let base: Hsla = match ink.role {
        Role::Text => theme.text,
        Role::Secondary => theme.secondary,
        Role::Canvas => theme.canvas,
        Role::Card => theme.card,
        Role::Accent => theme.accent,
        Role::Rgb(r, g, b) => gpui_kit::Rgba { r, g, b, a: 1. }.into(),
    };
    base.opacity(ink.alpha)
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
    fn path(&mut self, segs: &[Seg], fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        let theme = self.theme;
        let fill = fill.map(|i| hsla(i, &theme));
        let stroke = stroke.map(|(i, w, d)| (hsla(i, &theme), w, d));
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
// MARK: - Copy Image, Save as PNG

/// Shapes as SVG paths, for an image of the diagram.
struct Svg {
    out: String,
    theme: Theme,
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
    fn path(&mut self, segs: &[Seg], fill: Option<Ink>, stroke: Option<(Ink, f32, bool)>) {
        let theme = self.theme;
        let fill = fill.map(|i| hsla(i, &theme));
        let stroke = stroke.map(|(i, w, d)| (hsla(i, &theme), w, d));
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
    let mut svg = Svg { out: format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.2} {h:.2}\">\n<rect width=\"100%\" height=\"100%\" fill=\"{bg}\"/>\n"), theme: *theme };
    draw::paint(&mut svg, &drawn.geometry);
    for l in draw::labels(&drawn.geometry) {
        let (hex, a) = svg_color(hsla(l.color, theme));
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
                l.weight,
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
