//! The pieces the pages (Memory, Usage, the project page) and their sheets are made of, so they
//! look like one app: a header with a large title, a subtitle and a toolbar row; a segmented
//! control for tabs; section titles, chips, cards, stat tiles and empty states. Plain divs on
//! the theme's tokens: no shadows, no animation, nothing that costs a frame.

use crate::theme::{radius, space, text, Theme};
use gpui_kit::component::Icon;
use gpui_kit::{div, prelude::*, px, AnyElement, App, ClickEvent, FontWeight, Hsla, SharedString, Window};

/// Whether the theme is the dark one (the palette has no flag of its own).
pub fn is_dark(theme: &Theme) -> bool {
    theme.canvas.l < 0.5
}

/// A page narrower than this has no room for its toolbar with labels: buttons go to icons
/// with tooltips, lists narrow.
pub const COMPACT_BELOW: f32 = 920.;
/// Narrower still (a 640 pt window beside the sidebar): tabs as icons, one column at a time.
pub const NARROW_BELOW: f32 = 640.;

/// How wide a page is: what it measured at its last paint (`PageWidth::probe`), or before
/// its first, the window less a sidebar.
#[derive(Clone, Default)]
pub struct PageWidth(std::rc::Rc<std::cell::Cell<f32>>);

impl PageWidth {
    pub fn get(&self, window: &Window) -> f32 {
        match self.0.get() {
            w if w > 0. => w,
            _ => f32::from(window.viewport_size().width) - 260.,
        }
    }

    pub fn compact(&self, window: &Window) -> bool {
        self.get(window) < COMPACT_BELOW
    }

    pub fn narrow(&self, window: &Window) -> bool {
        self.get(window) < NARROW_BELOW
    }

    /// Laid over the page (absolute, full size): reads its width where it's painted, and when
    /// that crosses `COMPACT_BELOW` or `NARROW_BELOW` tells the page to draw again, after this
    /// frame. Costs nothing else: no layout of its own, no redraw while the width stays in a band.
    pub fn probe<V: 'static>(&self, view: gpui_kit::WeakEntity<V>) -> impl IntoElement {
        let cell = self.0.clone();
        let band = |w: f32| (w >= NARROW_BELOW) as u8 + (w >= COMPACT_BELOW) as u8;
        gpui_kit::canvas(
            move |bounds, _, cx| {
                let w = f32::from(bounds.size.width);
                let old = cell.replace(w);
                if old <= 0. || band(old) != band(w) {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        let _ = view.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// A page's header: an icon in a tile, the large title and a subtitle line, the toolbar on the
/// right, and under them (when given) the page's tabs. The title gives way first: it truncates
/// before the toolbar ever wraps.
pub fn page_header(icon: Icon, title: impl Into<SharedString>, subtitle: AnyElement, toolbar: AnyElement, tabs: Option<AnyElement>, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(space::M))
        .px(px(space::XL))
        .pt(px(space::L + space::XS))
        .pb(px(space::M))
        .border_b_1()
        .border_color(theme.separator)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(space::M))
                .child(icon_tile(icon, 38., theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(div().text_size(px(text::LARGE_TITLE)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).truncate().child(title.into()))
                        .child(div().min_w_0().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(subtitle)),
                )
                .child(div().flex_none().flex().items_center().gap(px(space::XS)).child(toolbar)),
        )
        .when_some(tabs, |d, tabs| d.child(div().flex().items_center().child(tabs)))
}

/// An icon in a soft rounded square: the page's mark beside its title.
pub fn icon_tile(icon: Icon, size: f32, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .rounded(px(radius::MEDIUM + 2.))
        .bg(theme.card)
        .border_1()
        .border_color(theme.separator)
        .flex()
        .items_center()
        .justify_center()
        .child(icon.size(px((size * 0.47).round())).text_color(theme.secondary))
}

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One segment of a segmented control.
pub struct Segment {
    id: SharedString,
    label: SharedString,
    icon: Option<Icon>,
    badge: Option<SharedString>,
    /// Drawn as its icon alone, the label as a tooltip (a narrow page).
    icon_only: bool,
    selected: bool,
    on_click: OnClick,
}

impl Segment {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, selected: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Segment { id: id.into(), label: label.into(), icon: None, badge: None, icon_only: false, selected, on_click: Box::new(on_click) }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Its icon alone, the label in a tooltip, when `on` (and it has an icon).
    pub fn icon_only(mut self, on: bool) -> Self {
        self.icon_only = on && self.icon.is_some();
        self
    }

    /// A count after the label (tasks open, PRs).
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

}

/// A segmented control, as macOS draws one and as Settings does: a quiet track, the chosen
/// segment raised on it (white in light, a lighter grey in dark), its icon in the accent.
pub fn segmented(segments: Vec<Segment>, theme: &Theme) -> gpui_kit::Div {
    let raised = if is_dark(theme) { theme.bubble } else { theme.canvas };
    let mut row = div().flex_none().flex().items_center().gap(px(2.)).p(px(2.)).rounded(px(radius::MEDIUM + 1.)).bg(theme.selected);
    for s in segments {
        let selected = s.selected;
        let mut seg = div()
            .id(s.id.clone())
            .h(px(26.))
            .px(px(space::M))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(radius::MEDIUM - 1.))
            .border_1()
            .text_size(px(text::BODY))
            .whitespace_nowrap()
            .cursor_pointer();
        seg = if selected {
            seg.bg(raised).border_color(theme.separator).text_color(theme.text).font_weight(FontWeight::MEDIUM)
        } else {
            seg.border_color(gpui_kit::transparent_black()).text_color(theme.secondary).hover(|d| d.text_color(theme.text).bg(theme.hover))
        };
        let on_click = s.on_click;
        let label = s.label;
        seg = seg
            .when_some(s.icon, |d, i| d.child(i.size(px(14.)).text_color(if selected { theme.accent } else { theme.secondary })))
            .when(s.icon_only, |d| {
                let tip = label.clone();
                d.px(px(space::S + 2.)).tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            })
            .when(!s.icon_only, |d| d.child(label))
            .when_some(s.badge, |d, b| d.child(div().text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.tertiary).child(b)))
            .on_click(move |e, w, cx| on_click(e, w, cx));
        row = row.child(seg);
    }
    row
}

/// A section's title inside a page: headline weight, its count quieter beside it.
pub fn section_title(title: impl Into<SharedString>, count: Option<usize>, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(6.))
        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(title.into()))
        .when_some(count.filter(|c| *c > 0), |d, c| d.child(div().text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child(c.to_string())))
}

/// A small label in a colour: state, kind. Text in the colour, on a wash of it.
pub fn chip(label: impl Into<SharedString>, color: Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(radius::SMALL))
        .text_size(px(text::CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .bg(color.opacity(0.13))
        .child(label.into())
}

/// A neutral chip: secondary text on a quiet fill.
pub fn quiet_chip(label: impl Into<SharedString>, theme: &Theme) -> gpui_kit::Div {
    div().flex_none().px(px(6.)).py(px(1.)).rounded(px(radius::SMALL)).text_size(px(text::CAPTION)).font_weight(FontWeight::MEDIUM).text_color(theme.secondary).bg(theme.selected).child(label.into())
}

/// A dot in a colour, for a kind or a state beside its words.
pub fn dot(color: Hsla, size: f32) -> gpui_kit::Div {
    div().flex_none().w(px(size)).h(px(size)).rounded_full().bg(color)
}

/// A card: what groups unlike things, or a list of rows.
pub fn card(theme: &Theme) -> gpui_kit::Div {
    div().rounded(px(radius::LARGE)).bg(theme.card).border_1().border_color(theme.separator)
}

/// A figure: a label, the value large, a note under it.
pub fn stat_tile(label: impl Into<SharedString>, value: impl Into<SharedString>, note: Option<String>, theme: &Theme) -> gpui_kit::Div {
    card(theme)
        .flex_1()
        .min_w(px(140.))
        .px(px(space::L))
        .py(px(space::M))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).truncate().child(label.into()))
        .child(div().text_size(px(text::LARGE_TITLE)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).truncate().child(value.into()))
        .when_some(note, |d, n| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).line_clamp(2).text_ellipsis().child(n)))
}

/// What a page or tab shows with nothing in it: an icon, a title, a line or two, actions.
pub fn empty_state(icon: Icon, title: impl Into<SharedString>, body: impl Into<SharedString>, actions: Option<AnyElement>, theme: &Theme) -> gpui_kit::Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(space::S))
        .p(px(space::XL))
        .child(div().mb(px(space::XS)).child(icon_tile(icon, 40., theme)))
        .child(div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(title.into()))
        .child(div().max_w(px(380.)).text_center().text_size(px(text::FOOTNOTE)).line_height(px(17.)).text_color(theme.secondary).child(body.into()))
        .when_some(actions, |d, a| d.child(div().mt(px(space::S)).flex().items_center().gap(px(space::S)).child(a)))
}

/// A row of a settings-like group: label left, control or value right.
pub fn form_row(label: impl Into<SharedString>, control: AnyElement, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::M))
        .child(div().w(px(110.)).flex_none().text_size(px(text::BODY)).text_color(theme.secondary).child(label.into()))
        .child(div().flex_1().min_w_0().child(control))
}

/// A small heading over a group in a sheet (sources, model…).
pub fn group_label(title: impl Into<SharedString>, theme: &Theme) -> gpui_kit::Div {
    div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.secondary).child(title.into())
}

/// A multi-line field inside a page that scrolls: the wheel moves the field while it can go
/// that way, and the page once the field is at its end (as the transcript's capped boxes
/// do). The field keeps every wheel event over it once its
/// text overflows, so this watches from the capture phase where each event was before it, and
/// after the event, if neither the field nor the page moved, moves the page by it.
pub fn wheel_passes_to_page(
    field: &gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
    page: &gpui_kit::ScrollHandle,
    content: impl IntoElement,
) -> impl IntoElement {
    use gpui_kit::{DispatchPhase, ScrollEvent};
    let (field, page) = (field.clone(), page.clone());
    div().relative().child(content).child(
        gpui_kit::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (field, page) = (field.clone(), page.clone());
                window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !bounds.contains(&e.position) {
                        return;
                    }
                    let delta = e.delta.pixel_delta(window.line_height());
                    if delta.y.abs() < delta.x.abs() || delta.y == px(0.) {
                        return;
                    }
                    let (field_was, page_was) = (field.read(cx).scroll_offset(), page.offset());
                    let (field, page) = (field.clone(), page.clone());
                    window.defer(cx, move |window, cx| {
                        if page.offset() != page_was || field.read(cx).scroll_offset() != field_was {
                            return;
                        }
                        let y = (page_was.y + delta.y).clamp(-page.max_offset().y, px(0.));
                        if y != page_was.y {
                            page.set_offset(gpui_kit::point(page_was.x, y));
                            window.refresh();
                        }
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// A menu's footnote ("Applies from your next message."): caption size, tertiary, wrapping,
/// in line with the rows' titles, and never lit or clicked.
pub fn menu_note(note: impl Into<SharedString>) -> gpui_kit::component::menu::PopupMenuItem {
    let note: SharedString = note.into();
    gpui_kit::component::menu::PopupMenuItem::element(move |_, cx| {
        let theme = Theme::get(cx);
        div()
            .py(px(2.))
            .max_w(px(260.))
            .whitespace_normal()
            .text_size(px(text::CAPTION))
            .line_height(px(14.))
            .text_color(theme.tertiary)
            .child(note.clone())
    })
    .disabled(true)
}
