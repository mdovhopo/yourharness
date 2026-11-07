//! Text you can select and copy (⌘C), with links you can click: a `StyledText` (its highlights
//! kept) that takes part in the window's text selection (gpui-base's `TextSelection`, whose
//! layer the window's `Root` draws), as `SelectableText` does for plain text. Clicking a link
//! follows it; dragging from one selects. The pointer turns into a hand over links, and the row
//! is drawn again only when it goes on or off one (`InteractiveText` redraws its view at every
//! character the pointer crosses).

use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::{
    App, BorderStyle, Bounds, Corners, CursorStyle, DispatchPhase, Edges, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    HighlightStyle, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    SharedString, StyledText, TextRun, TextStyle, Window, transparent_black,
};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

type OnLink = Rc<dyn Fn(usize, &mut Window, &mut App)>;

thread_local! {
    /// Where each text was last laid out, by its text: for `YH_SELECT_PROBE` only.
    static PLACES: std::cell::RefCell<Option<std::collections::HashMap<SharedString, Bounds<Pixels>>>> = std::cell::RefCell::new(
        std::env::var("YH_SELECT_PROBE").is_ok().then(Default::default)
    );
}

/// Where `text` was last laid out (`YH_SELECT_PROBE` runs).
pub fn place_of(text: &str) -> Option<Bounds<Pixels>> {
    PLACES.with(|p| p.borrow().as_ref().and_then(|p| p.get(text).copied()))
}

pub struct SelText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    order: u64,
    color: Hsla,
    links: Vec<Range<usize>>,
    on_link: Option<OnLink>,
    /// Styles laid over the inherited text style when laid out, and ranges set in SF Mono
    /// (inline code): what `StyledText`'s own highlights can't do, as they can't change font.
    rich: Option<(Vec<(Range<usize>, HighlightStyle)>, Vec<Range<usize>>)>,
}

/// What a `SelText` keeps between frames.
#[derive(Clone)]
pub struct State {
    handle: TextSelectionHandle,
    /// The link the button went down on.
    down: Rc<Cell<Option<usize>>>,
    /// The link the pointer is over.
    hovered: Rc<Cell<Option<usize>>>,
}

impl SelText {
    /// `order` places it in reading order among the others (rows, then blocks in a row);
    /// `color` paints what's selected.
    pub fn new(id: impl Into<ElementId>, text: SharedString, styled: StyledText, order: u64, color: Hsla) -> SelText {
        SelText { id: id.into(), text, styled, order, color, links: vec![], on_link: None, rich: None }
    }

    /// Clickable ranges; `on_link` gets the index of the one clicked.
    pub fn links(mut self, ranges: Vec<Range<usize>>, on_link: impl Fn(usize, &mut Window, &mut App) + 'static) -> SelText {
        self.links = ranges;
        self.on_link = Some(Rc::new(on_link));
        self
    }

    /// Highlights, and ranges in the monospaced font, over whatever text style it inherits.
    pub fn rich(mut self, highlights: Vec<(Range<usize>, HighlightStyle)>, mono: Vec<Range<usize>>) -> SelText {
        self.rich = Some((highlights, mono));
        self
    }

    /// The runs for `rich`: cut wherever a style starts or ends, each piece the inherited
    /// style with every highlight over it, in SF Mono where a mono range covers it.
    fn rich_runs(text: &str, base: &TextStyle, highlights: &[(Range<usize>, HighlightStyle)], mono: &[Range<usize>]) -> Vec<TextRun> {
        let mut cuts: Vec<usize> = vec![0, text.len()];
        for r in highlights.iter().map(|(r, _)| r).chain(mono.iter()) {
            cuts.push(r.start.min(text.len()));
            cuts.push(r.end.min(text.len()));
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.retain(|c| text.is_char_boundary(*c));
        cuts.windows(2)
            .filter(|w| w[1] > w[0])
            .map(|w| {
                let mut style = base.clone();
                for (r, h) in highlights {
                    if r.start <= w[0] && w[1] <= r.end {
                        style = style.highlight(*h);
                    }
                }
                if mono.iter().any(|r| r.start <= w[0] && w[1] <= r.end) {
                    style.font_family = crate::theme::MONO.into();
                }
                style.to_run(w[1] - w[0])
            })
            .collect()
    }

    fn link_at(links: &[Range<usize>], layout: &gpui_kit::TextLayout, position: Point<Pixels>) -> Option<usize> {
        let ix = layout.index_for_position(position).ok()?;
        links.iter().position(|r| r.contains(&ix))
    }
}

impl IntoElement for SelText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The selection's rectangles from `start` to `end`: the rest of the first line, the lines
/// between, the start of the last.
fn quads(start: Point<Pixels>, end: Point<Pixels>, bounds: Bounds<Pixels>, line_height: Pixels) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(start, Point::new(end.x, end.y + line_height))];
    }
    let mut out = vec![Bounds::from_corners(start, Point::new(bounds.right(), start.y + line_height))];
    if end.y > start.y + line_height {
        out.push(Bounds::from_corners(Point::new(bounds.left(), start.y + line_height), Point::new(bounds.right(), end.y)));
    }
    out.push(Bounds::from_corners(Point::new(bounds.left(), end.y), Point::new(end.x, end.y + line_height)));
    out
}

impl Element for SelText {
    type RequestLayoutState = State;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, State) {
        let state = window.with_element_state(id.expect("SelText has an id"), |kept: Option<State>, _| {
            let state = kept.unwrap_or_else(|| State { handle: TextSelectionHandle::new(self.text.clone(), cx), down: Default::default(), hovered: Default::default() });
            (state.clone(), state)
        });
        if let Some((highlights, mono)) = self.rich.take() {
            let runs = Self::rich_runs(&self.text, &window.text_style(), &highlights, &mono);
            self.styled = StyledText::new(self.text.clone()).with_runs(runs);
        }
        let (layout, ()) = self.styled.request_layout(id, inspector_id, window, cx);
        (layout, state)
    }

    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut State, window: &mut Window, cx: &mut App) -> Hitbox {
        self.styled.prepaint(id, inspector_id, bounds, &mut (), window, cx);
        PLACES.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.insert(self.text.clone(), bounds);
            }
        });
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.order)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(&state.handle, window, cx);
        state.handle.register(registration, window, cx);
        hitbox
    }

    fn paint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut State, hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        let layout = self.styled.layout().clone();
        // The text first, then what's selected over it: an inline code's background would hide it.
        self.styled.paint(id, inspector_id, bounds, &mut (), &mut (), window, cx);
        let projection = state.handle.update_runs(&[TextSelectionRun::new(self.text.clone(), layout.clone(), bounds).with_document_order(self.order)], cx);
        for range in projection.ranges().iter().flatten() {
            let (Some(start), Some(end)) = (layout.position_for_index(range.start), layout.position_for_index(range.end)) else { continue };
            for b in quads(start, end, layout.bounds(), layout.line_height()) {
                window.paint_quad(PaintQuad {
                    bounds: b,
                    background: self.color.into(),
                    corner_radii: Corners::default(),
                    border_widths: Edges::default(),
                    border_color: transparent_black(),
                    border_style: BorderStyle::default(),
                });
            }
        }
        if let Some(on_link) = self.on_link.clone() {
            let links = Rc::new(std::mem::take(&mut self.links));
            if state.hovered.get().is_some() {
                window.set_cursor_style(CursorStyle::PointingHand, hitbox);
            }
            let view = window.current_view();
            {
                let (hitbox, layout, links, hovered) = (hitbox.clone(), layout.clone(), links.clone(), state.hovered.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let over = if hitbox.is_hovered(window) { Self::link_at(&links, &layout, event.position) } else { None };
                    // On or off a link: the hand comes or goes. Within one, nothing.
                    if over.is_some() != hovered.get().is_some() {
                        hovered.set(over);
                        cx.notify(view);
                    } else {
                        hovered.set(over);
                    }
                });
            }
            {
                let (hitbox, layout, links, down) = (hitbox.clone(), layout.clone(), links.clone(), state.down.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && event.click_count == 1 {
                        down.set(if hitbox.is_hovered(window) { Self::link_at(&links, &layout, event.position) } else { None });
                    }
                });
            }
            {
                let (hitbox, down) = (hitbox.clone(), state.down.clone());
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    let Some(at) = down.take() else { return };
                    // Let go on the link it went down on: followed. Dragged off it: a selection.
                    if hitbox.is_hovered(window) && Self::link_at(&links, &layout, event.position) == Some(at) {
                        on_link(at, window, cx);
                    }
                });
            }
        }
    }
}
