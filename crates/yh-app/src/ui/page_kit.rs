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

/// A window too narrow for a page's toolbar with labels (beside the sidebar): buttons go to
/// icons with tooltips, lists narrow.
pub fn is_compact(window: &Window) -> bool {
    window.viewport_size().width < gpui_kit::px(1180.)
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
    selected: bool,
    on_click: OnClick,
}

impl Segment {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, selected: bool, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        Segment { id: id.into(), label: label.into(), icon: None, badge: None, selected, on_click: Box::new(on_click) }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A count after the label (tasks open, PRs).
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

}

/// A segmented control, as macOS draws one: a quiet track, the chosen segment raised on it.
pub fn segmented(segments: Vec<Segment>, theme: &Theme) -> gpui_kit::Div {
    let dark = is_dark(theme);
    let track = if dark { gpui_kit::white().opacity(0.06) } else { gpui_kit::black().opacity(0.05) };
    let raised = if dark { gpui_kit::white().opacity(0.16) } else { gpui_kit::white() };
    let edge = if dark { gpui_kit::white().opacity(0.06) } else { gpui_kit::black().opacity(0.1) };
    let mut row = div().flex_none().flex().items_center().gap(px(2.)).p(px(2.)).rounded(px(radius::MEDIUM + 1.)).bg(track);
    for s in segments {
        let selected = s.selected;
        let mut seg = div()
            .id(s.id.clone())
            .h(px(24.))
            .px(px(space::S + 2.))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(radius::MEDIUM - 1.))
            .border_1()
            .text_size(px(text::BODY))
            .whitespace_nowrap()
            .cursor_pointer();
        seg = if selected {
            seg.bg(raised).border_color(edge).text_color(theme.text).font_weight(FontWeight::MEDIUM)
        } else {
            seg.border_color(gpui_kit::transparent_black()).text_color(theme.secondary).hover(|d| d.text_color(theme.text).bg(theme.hover))
        };
        let on_click = s.on_click;
        seg = seg
            .when_some(s.icon, |d, i| d.child(i.size(px(14.)).text_color(if selected { theme.text } else { theme.secondary })))
            .child(s.label)
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
        .when_some(note, |d, n| d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).line_clamp(2).child(n)))
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
