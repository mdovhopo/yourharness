//! Usage (⇧⌘U), in place of the open thread: the 5-hour and weekly limits, then what was
//! spent over a range of days, in tokens or in dollars at API prices, by model and by day.
//! Read from Claude Code's session files into `usage.sqlite` off the main thread
//! (`yh_core::usage`). The chart is one canvas of quads; hovering finds the day from the
//! pointer, so moving along a year of bars redraws the band and the tooltip, nothing else.

use crate::state::workspace::Workspace;
use crate::theme::{space, text, Theme};
use crate::ui::page_icons as icons;
use crate::ui::page_kit::{self, Segment};
use gpui_kit::component::{button::ButtonVariants, Disableable, Sizable};
use gpui_kit::{
    canvas, div, fill, point, prelude::*, px, rgb, size, Bounds, Context, Entity, FontWeight, Hsla, Pixels, SharedString, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use yh_core::usage::{civil, display_name, Report, UsageStore};

const RANGES: [(&str, usize); 5] = [("Week", 7), ("Month", 30), ("Quarter", 90), ("Half Year", 182), ("Year", 365)];
/// The ranges' labels on a narrow page, as Health and Stocks write them.
const SHORT_RANGES: [&str; 5] = ["W", "M", "3M", "6M", "Y"];

/// The categorical palette (light, dark), in the order checked for color blindness
/// (validated with the dataviz script against both surfaces). Past seven
/// models, the rest share Other.
const PALETTE: [(u32, u32); 7] =
    [(0x2A78D6, 0x3987E5), (0xEB6834, 0xD95926), (0x1BAF7A, 0x199E70), (0xEDA100, 0xC98500), (0xE87BA4, 0xD55181), (0x008300, 0x008300), (0x6250D6, 0x9085E9)];
const OTHER: (u32, u32) = (0x9A9A9F, 0x6E6E73);

pub struct UsagePage {
    workspace: Entity<Workspace>,
    store: Arc<UsageStore>,
    days: usize,
    cost: bool,
    report: Option<Report>,
    reading: Option<String>,
    hovered: Option<usize>,
    /// The chart's place, from its last paint: where the pointer is in days.
    chart: Rc<Cell<Bounds<Pixels>>>,
    probe_started: bool,
    /// The page's own width, measured where it's painted.
    width: page_kit::PageWidth,
    /// "By model": a small chart per model instead of one stacked chart.
    split: bool,
    /// The per-day table's page (0 the newest).
    day_page: usize,
    refreshing: bool,
    pub focus: gpui_kit::FocusHandle,
}

gpui_kit::actions!(usage, [CloseUsage]);

/// Rows a page of the per-day table holds.
const DAYS_PER_PAGE: usize = 14;
/// When the price table was last checked against Anthropic's page (`yh_core::usage::entry`).
const PRICES_CHECKED: &str = "2026-10-28";
const PRICES_SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";

impl UsagePage {
    pub fn new(workspace: Entity<Workspace>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Arc::new(UsageStore::new(&workspace.read(cx).data_dir));
        let days = match std::env::var("YH_USAGE_RANGE").as_deref() {
            Ok("week") => 7,
            Ok("quarter") => 90,
            Ok("halfYear") => 182,
            Ok("year") => 365,
            _ => 30,
        };
        let mut page = UsagePage {
            workspace,
            store,
            days,
            cost: std::env::var("YH_USAGE_METRIC").as_deref() == Ok("cost"),
            report: None,
            reading: Some("Reading Claude Code's history…".into()),
            hovered: std::env::var("YH_USAGE_HOVER").ok().and_then(|d| d.parse::<usize>().ok()).map(|ago| days.saturating_sub(ago + 1)),
            chart: Rc::new(Cell::new(Bounds::default())),
            probe_started: false,
            width: Default::default(),
            split: std::env::var("YH_USAGE_SPLIT").as_deref() == Ok("1"),
            day_page: std::env::var("YH_USAGE_DAY_PAGE").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            refreshing: true,
            focus: cx.focus_handle(),
        };
        page.refresh(cx);
        page
    }

    /// Reads what's new in the session files, then makes the report: both off the main thread.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (store, days) = (self.store.clone(), self.days);
        let config = yh_core::history::config_directory();
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let (read, report) = cx.background_executor().spawn(async move { (store.refresh(&config), store.report(days)) }).await;
            if std::env::var("YH_DATA_DIR").is_ok() {
                eprintln!("USAGE read {} of {} files ({} replies) and reported {days} days in {} ms", read.files_read, read.files_total, read.records, started.elapsed().as_millis());
            }
            let _ = this.update(cx, |page, cx| {
                page.report = Some(report);
                page.reading = None;
                page.refreshing = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn set_range(&mut self, days: usize, cx: &mut Context<Self>) {
        self.days = days;
        self.hovered = None;
        self.day_page = 0;
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let report = cx.background_executor().spawn(async move { store.report(days) }).await;
            let _ = this.update(cx, |page, cx| {
                if page.days == days {
                    page.report = Some(report);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// `YH_USAGE_PROBE=1` (test runs): once the report shows, moves the pointer across the chart
    /// and scrolls over it through the window's own event path (two rounds, the second on a
    /// year in dollars), then prints the main thread's longest stall: PASS under 100 ms. Each
    /// step waits a frame's time; whatever it waited past that, the main thread was busy.
    /// `YH_USAGE_PROBE_WAIT=<seconds>` before it starts, for attaching Time Profiler.
    fn start_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.probe_started || std::env::var("YH_USAGE_PROBE").as_deref() != Ok("1") {
            return;
        }
        self.probe_started = true;
        use gpui_kit::{Modifiers, MouseMoveEvent, PlatformInput, ScrollDelta, ScrollEvent, TouchPhase};
        let cell = self.chart.clone();
        cx.spawn_in(window, async move |this, cx| {
            let wait = std::env::var("YH_USAGE_PROBE_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            cx.background_executor().timer(std::time::Duration::from_secs_f64(wait)).await;
            let tick = std::time::Duration::from_millis(16);
            let (mut longest, mut steps) = (std::time::Duration::ZERO, 0);
            for round in 0..2 {
                if round == 1 {
                    let _ = this.update(cx, |p, cx| {
                        p.cost = true;
                        p.set_range(365, cx);
                    });
                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                }
                for i in 0..=120 {
                    let started = std::time::Instant::now();
                    let sent = cx.update(|window, cx| {
                        let b = cell.get();
                        let at = point(b.origin.x + b.size.width * (i as f32 / 120.), b.origin.y + b.size.height / 2.);
                        let m = Modifiers::default();
                        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: m }), cx);
                        if i % 12 == 0 {
                            let delta = ScrollDelta::Pixels(point(px(0.), px(if i % 24 == 0 { -12. } else { 12. })));
                            window.dispatch_event(PlatformInput::Scroll(ScrollEvent { position: at, delta, modifiers: m, touch_phase: TouchPhase::Moved, source: Default::default() }), cx);
                        }
                    });
                    if sent.is_err() {
                        return;
                    }
                    cx.background_executor().timer(tick).await;
                    longest = longest.max(started.elapsed().saturating_sub(tick));
                    steps += 1;
                }
            }
            let ms = longest.as_millis();
            eprintln!("USAGE PROBE {} {steps} steps, longest main-thread stall {ms} ms", if ms < 100 { "PASS" } else { "FAIL" });
        })
        .detach();
    }

    /// Esc: back to the thread.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            if ws.page == Some(crate::state::workspace::Page::Usage) {
                ws.toggle_usage(cx);
            }
        });
    }

    fn value(&self, s: &yh_core::usage::Spend) -> f64 {
        if self.cost {
            s.cost
        } else {
            s.tokens.total() as f64
        }
    }

    fn format(&self, v: f64) -> String {
        if self.cost {
            if v >= 100. {
                format!("${:.0}", v)
            } else {
                format!("${:.2}", v)
            }
        } else {
            tokens(v)
        }
    }
}

impl UsagePage {
    /// A grid line's value, as short as it can be: "200M", "$50", "2.5K".
    fn tick(&self, v: f64) -> String {
        if v == 0. {
            return "0".into();
        }
        let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
        if self.cost {
            return format!("${}", trim(format!("{v:.2}")));
        }
        let (n, unit) = match v {
            v if v >= 1e9 => (v / 1e9, "B"),
            v if v >= 1e6 => (v / 1e6, "M"),
            v if v >= 1e3 => (v / 1e3, "K"),
            v => (v, ""),
        };
        format!("{}{unit}", trim(format!("{n:.1}")))
    }
}

/// "12.4M", "830K", "912".
fn tokens(v: f64) -> String {
    match v {
        v if v >= 1e9 => format!("{:.1}B", v / 1e9),
        v if v >= 1e6 => format!("{:.1}M", v / 1e6),
        v if v >= 1e3 => format!("{:.0}K", v / 1e3),
        v => format!("{v:.0}"),
    }
}

fn day_label(days: i64) -> String {
    let (_, m, d) = civil(days);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {d}", MONTHS[(m as usize).saturating_sub(1).min(11)])
}

/// A round number at or above `v` for the chart's top line: 1, 2, 2.5 or 5 times a power of ten.
fn nice_ceil(v: f64) -> f64 {
    if v <= 0. {
        return 1.;
    }
    let e = 10f64.powf(v.log10().floor());
    [1., 2., 2.5, 5., 10.].into_iter().map(|m| m * e).find(|x| *x >= v * 0.999).unwrap_or(10. * e)
}

impl Render for UsagePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let dark = page_kit::is_dark(&theme);
        let color = |c: (u32, u32)| -> Hsla { rgb(if dark { c.1 } else { c.0 }).into() };
        let page = cx.entity().downgrade();
        // A narrow page: short range labels, the metric as icons, so the tabs fit one row.
        let narrow = self.width.narrow(window);
        let ranges: Vec<Segment> = RANGES
            .into_iter()
            .zip(SHORT_RANGES)
            .map(|((label, days), short)| {
                let page = page.clone();
                Segment::new(SharedString::from(format!("range-{days}")), if narrow { short } else { label }, self.days == days, move |_, _, cx| {
                    let _ = page.update(cx, |p, cx| p.set_range(days, cx));
                })
            })
            .collect();
        let metric = |id: &'static str, label: &'static str, cost: bool, icon: gpui_kit::component::Icon| {
            let page = page.clone();
            Segment::new(id, label, self.cost == cost, move |_, _, cx| {
                let _ = page.update(cx, |p, cx| {
                    p.cost = cost;
                    cx.notify();
                });
            })
            .icon(icon)
            .icon_only(narrow)
        };
        let tabs = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(space::S))
            .child(page_kit::segmented(ranges, &theme))
            .child(div().flex_1())
            .child(page_kit::segmented(vec![metric("metric-tokens", "Tokens", false, icons::hash()), metric("metric-cost", "Cost", true, icons::coins())], &theme))
            .into_any_element();
        let subtitle = div().truncate().child(self.reading.clone().unwrap_or_else(|| "Every Claude Code session, here or in a terminal".into())).into_any_element();
        let refreshing = self.refreshing;
        let toolbar = crate::ui::thread_toolbar::icon_button("usage-refresh", gpui_kit::component::Icon::new(gpui_kit::component::IconName::RefreshCw), "Read Claude Code's history again", &theme)
            .loading(refreshing)
            .disabled(refreshing)
            .on_click(cx.listener(|p, _, _, cx| {
                p.refreshing = true;
                p.refresh(cx);
                cx.notify();
            }))
            .into_any_element();
        let header = page_kit::page_header(icons::chart_column(), "Usage", subtitle, toolbar, Some(tabs), &theme);
        if self.report.is_some() {
            self.start_probe(window, cx);
        }
        let Some(report) = &self.report else {
            return div()
                .size_full()
                .relative()
                .track_focus(&self.focus)
                .key_context("UsagePage")
                .on_action(cx.listener(|p, _: &CloseUsage, _, cx| p.close(cx)))
                .flex()
                .flex_col()
                .child(header)
                .child(self.width.probe(page.clone()))
                .child(div().flex_1().child(page_kit::empty_state(icons::chart_column(), "Reading Claude Code's History", "Every session's replies, here or in a terminal, added up by day and by model.", None, &theme)))
                .into_any_element();
        };
        // Colors follow the model, by when it was first used, never by this range's ranking.
        let used: Vec<&String> = report.models.iter().map(|(m, _)| m).collect();
        let mut series: Vec<(String, String, Hsla)> = vec![];
        let mut other = false;
        for (i, model) in report.all_models.iter().enumerate().filter(|(_, m)| used.contains(m)) {
            if i < PALETTE.len() {
                series.push((model.clone(), display_name(model), color(PALETTE[i])));
            } else {
                other = true;
            }
        }
        if other {
            series.push(("other".into(), "Other".into(), color(OTHER)));
        }
        let key_of = |model: &str| -> usize { series.iter().position(|(k, _, _)| k == model).unwrap_or(series.len().saturating_sub(1)) };
        // Each day's stack, in series order.
        let stacks: Vec<Vec<(usize, f64)>> = report
            .days
            .iter()
            .map(|d| {
                let mut by: Vec<f64> = vec![0.; series.len()];
                for (m, s) in &d.models {
                    by[key_of(m)] += self.value(s);
                }
                by.into_iter().enumerate().filter(|(_, v)| *v > 0.).collect()
            })
            .collect();
        // The split charts read the days too; the stacked chart's painter takes its own copy.
        let split_stacks = stacks.clone();
        let peak = stacks.iter().map(|s| s.iter().map(|(_, v)| v).sum::<f64>()).fold(0., f64::max);
        // The chart's top: a round number over the busiest day, so the grid reads cleanly.
        let max = nice_ceil(peak).max(1e-9);
        let colors: Vec<Hsla> = series.iter().map(|(_, _, c)| *c).collect();
        let bounds_cell = self.chart.clone();
        let hovered = self.hovered;
        let (band, grid, surface) = (theme.hover, theme.separator, theme.card);
        let n = stacks.len();
        const PLOT_H: f32 = 200.;
        let chart = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width) / n.max(1) as f32;
                let h = f32::from(bounds.size.height);
                // Hairline grid at the top, the middle and the baseline.
                for f in [0., 0.5, 1.] {
                    let y = f32::from(bounds.origin.y) + (h - 1.) * f;
                    window.paint_quad(fill(Bounds::new(point(bounds.origin.x, px(y)), size(bounds.size.width, px(1.))), grid));
                }
                // Thin columns: at most 24 px, the rest of each day's slot is air.
                let bar = (w - if w > 6. { 3. } else { 0.5 }).clamp(0.5, 24.);
                if let Some(i) = hovered {
                    window.paint_quad(fill(Bounds::new(point(bounds.origin.x + px(i as f32 * w), bounds.origin.y), size(px(w), bounds.size.height)), band).corner_radii(px(3.)));
                }
                let r = (bar / 2.).min(4.);
                for (i, stack) in stacks.iter().enumerate() {
                    let x = bounds.origin.x + px(i as f32 * w + (w - bar) / 2.);
                    let mut y = f32::from(bounds.origin.y) + h - 1.;
                    let segs: Vec<(usize, f32)> = stack.iter().map(|(k, v)| (*k, (*v / max) as f32 * (h - 1.))).filter(|(_, s)| *s >= 0.5).collect();
                    let last = segs.len().saturating_sub(1);
                    for (j, (k, seg)) in segs.into_iter().enumerate() {
                        let quad = fill(Bounds::new(point(x, px(y - seg)), size(px(bar), px(seg))), colors[k]);
                        // The data end rounded, square at the baseline.
                        window.paint_quad(if j == last && bar > 3. { quad.corner_radii(gpui_kit::Corners { top_left: px(r), top_right: px(r), bottom_left: px(0.), bottom_right: px(0.) }) } else { quad });
                        y -= seg;
                        if j < last && seg > 3. {
                            // A 2 px gap in the card's colour between segments.
                            window.paint_quad(fill(Bounds::new(point(x, px(y - 1.)), size(px(bar), px(2.))), if hovered == Some(i) { band } else { surface }));
                        }
                    }
                }
            },
        )
        .size_full();
        let cell = self.chart.clone();
        let chart_box = div()
            .id("usage-chart")
            .flex_1()
            .min_w_0()
            .h(px(PLOT_H))
            .child(chart)
            .on_mouse_move(cx.listener(move |this, e: &gpui_kit::MouseMoveEvent, _, cx| {
                let b = cell.get();
                let x = f32::from(e.position.x - b.origin.x);
                let day = if b.size.width > px(0.) && x >= 0. { Some(((x / f32::from(b.size.width)) * n as f32) as usize).filter(|d| *d < n) } else { None };
                if day != this.hovered {
                    this.hovered = day;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over && this.hovered.is_some() {
                    this.hovered = None;
                    cx.notify();
                }
            }));
        // The grid's values, in a gutter at the left; the days under the plot.
        const GUTTER: f32 = 52.;
        let tick = |f: f32, v: f64| div().absolute().right(px(space::S)).top(px((PLOT_H - 1.) * f - 7.)).text_size(px(text::CAPTION)).text_color(theme.tertiary).child(self.tick(v));
        let gutter = div().relative().flex_none().w(px(GUTTER)).h(px(PLOT_H)).child(tick(0., max)).child(tick(0.5, max / 2.)).child(tick(1., 0.));
        let first = report.first_day;
        let axis_color = theme.tertiary;
        let axis = move |pad: f32| {
            div()
                .pl(px(pad))
                .flex()
                .justify_between()
                .text_size(px(text::CAPTION))
                .text_color(axis_color)
                .child(day_label(first))
                .child(day_label(first + n as i64 / 2))
                .child(day_label(first + n as i64 - 1))
        };
        let legend = div().pl(px(GUTTER)).flex().flex_wrap().gap(px(space::M)).text_size(px(text::CAPTION)).text_color(theme.secondary).children(
            series.iter().map(|(_, name, c)| div().flex().items_center().gap(px(5.)).child(div().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(*c)).child(name.clone())),
        );
        // A row per series (models past seven together as Other), the biggest first.
        let rows_of = |spends: Vec<(&String, &yh_core::usage::Spend)>| -> Vec<(Hsla, String, f64, i64)> {
            let mut by: Vec<(f64, i64)> = vec![(0., 0); series.len()];
            for (m, s) in spends {
                let k = key_of(m);
                by[k].0 += self.value(s);
                by[k].1 += s.tokens.total();
            }
            let mut rows: Vec<(Hsla, String, f64, i64)> =
                by.into_iter().enumerate().filter(|(_, (v, t))| *v > 0. || *t > 0).map(|(k, (v, t))| (series[k].2, series[k].1.clone(), v, t)).collect();
            rows.sort_by(|a, b| b.2.total_cmp(&a.2).then(b.3.cmp(&a.3)));
            rows
        };
        // The hovered day, or the range: one line per model.
        let (title, total, rows): (String, f64, Vec<(Hsla, String, f64, i64)>) = match self.hovered.and_then(|i| report.days.get(i).map(|d| (i, d))) {
            Some((i, d)) => (day_label(first + i as i64), self.value(&d.total), rows_of(d.models.iter().collect())),
            None => (format!("Last {} days", report.days.len()), self.value(&report.total), rows_of(report.models.iter().map(|(m, s)| (m, s)).collect())),
        };
        let metric_name = if self.cost { "Cost at API prices" } else { "Tokens" };
        // By model: a row per series, its own scale, the days' columns in its colour.
        let split_charts = if !self.split {
            div()
        } else {
            let mut col = div().flex().flex_col().gap(px(space::M));
            for (k, (_, name, c)) in series.iter().enumerate() {
                let values: Vec<f64> = split_stacks.iter().map(|st| st.iter().filter(|(i, _)| *i == k).map(|(_, v)| *v).sum()).collect();
                let top = values.iter().cloned().fold(0., f64::max);
                if top <= 0. {
                    continue;
                }
                let sum: f64 = values.iter().sum();
                let (c, grid) = (*c, theme.separator);
                let n = values.len();
                let mini = canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let w = f32::from(bounds.size.width) / n.max(1) as f32;
                        let h = f32::from(bounds.size.height);
                        window.paint_quad(fill(Bounds::new(point(bounds.origin.x, bounds.origin.y + px(h - 1.)), size(bounds.size.width, px(1.))), grid));
                        let bar = (w - if w > 6. { 3. } else { 0.5 }).clamp(0.5, 24.);
                        for (i, v) in values.iter().enumerate() {
                            let seg = (*v / top) as f32 * (h - 1.);
                            if seg < 0.5 {
                                continue;
                            }
                            let x = bounds.origin.x + px(i as f32 * w + (w - bar) / 2.);
                            window.paint_quad(fill(Bounds::new(point(x, bounds.origin.y + px(h - 1. - seg)), size(px(bar), px(seg))), c));
                        }
                    },
                )
                .size_full();
                col = col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(text::FOOTNOTE))
                                .child(div().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(c))
                                .child(div().flex_1().child(name.clone()))
                                .child(div().text_color(theme.secondary).child(format!("peak {} · {}", self.format(top), self.format(sum)))),
                        )
                        .child(div().h(px(56.)).child(mini)),
                );
            }
            col.child(axis(0.))
        };
        let chart_card = page_kit::card(&theme)
            .px(px(space::L))
            .pt(px(space::M + 2.))
            .pb(px(space::M))
            .flex()
            .flex_col()
            .gap(px(space::S + 2.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::S))
                    .child(page_kit::section_title(format!("{metric_name} by day"), None, &theme))
                    .child(div().flex_1())
                    .child(div().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("{title} · ")))
                    .child(div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(self.format(total)))
                    .child(div().ml(px(space::S)).child(page_kit::segmented(
                        vec![
                            {
                                let page = page.clone();
                                Segment::new("layout-stacked", "Stacked", !self.split, move |_, _, cx| {
                                    let _ = page.update(cx, |p, cx| {
                                        p.split = false;
                                        cx.notify();
                                    });
                                })
                            },
                            {
                                let page = page.clone();
                                Segment::new("layout-split", "By Model", self.split, move |_, _, cx| {
                                    let _ = page.update(cx, |p, cx| {
                                        p.split = true;
                                        cx.notify();
                                    });
                                })
                            },
                        ],
                        &theme,
                    ))),
            )
            .when(!self.split, |d| d.child(div().pt(px(space::XS)).flex().child(gutter).child(chart_box)).child(axis(GUTTER)).child(legend))
            .when(self.split, |d| d.child(split_charts));
        let share_total = rows.iter().map(|r| r.2).sum::<f64>().max(1e-9);
        let mut table = page_kit::card(&theme).px(px(space::L)).py(px(space::M)).flex().flex_col().child(
            div()
                .pb(px(6.))
                .flex()
                .items_baseline()
                .gap(px(space::S))
                .child(page_kit::section_title(format!("{title} by model"), None, &theme))
                .child(div().flex_1())
                .child(div().text_size(px(text::FOOTNOTE)).font_weight(FontWeight::SEMIBOLD).child(self.format(total))),
        );
        if rows.is_empty() {
            table = table.child(div().py(px(space::S)).text_size(px(text::FOOTNOTE)).text_color(theme.tertiary).child("Nothing used on this day."));
        }
        for (i, (c, name, v, toks)) in rows.into_iter().enumerate() {
            let share = (v / share_total) as f32;
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S + 2.))
                    .py(px(7.))
                    .when(i > 0, |d| d.border_t_1().border_color(theme.separator))
                    .child(div().flex_none().w(px(8.)).h(px(8.)).rounded(px(2.)).bg(c))
                    .child(div().w(px(150.)).flex_none().truncate().child(name))
                    // The share as a thin bar in the model's colour: the eye reads it before the number.
                    .child(div().flex_1().min_w(px(40.)).h(px(4.)).rounded(px(2.)).bg(theme.selected).child(div().h_full().rounded(px(2.)).w(gpui_kit::relative(share)).bg(c.opacity(0.85))))
                    .child(div().w(px(44.)).flex_none().text_right().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(format!("{:.0}%", share * 100.)))
                    .child(div().w(px(80.)).flex_none().text_right().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(if self.cost { tokens(toks as f64) } else { String::new() }))
                    .child(div().w(px(80.)).flex_none().text_right().font_weight(FontWeight::MEDIUM).child(self.format(v))),
            );
        }
        let active_days = report.days.iter().filter(|d| d.total.messages > 0).count().max(1);
        let t = &report.total;
        let limits = self.workspace.read(cx).limits.clone();
        let mut limit_row = div().flex().flex_wrap().gap(px(space::M));
        let mut any_limit = false;
        if let Some(windows) = limits.as_ref().and_then(|l| l["unifiedWindows"].as_object().cloned()) {
            for (name, w) in windows {
                any_limit = true;
                let used = w["utilization"].as_f64().unwrap_or(0.).clamp(0., 1.);
                let label = match name.as_str() {
                    "five_hour" => "5-hour limit".to_string(),
                    "seven_day" => "Weekly limit".to_string(),
                    other => other.replace('_', " "),
                };
                // The fill carries how close it is; the track is a lighter step of the same hue.
                let tint = if used > 0.8 { theme.warning } else { theme.accent };
                limit_row = limit_row.child(
                    page_kit::card(&theme)
                        .flex_1()
                        .min_w(px(200.))
                        .px(px(space::L))
                        .py(px(space::M))
                        .flex()
                        .flex_col()
                        .gap(px(space::S))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(icons::gauge().size(px(14.)).text_color(theme.secondary))
                                .child(div().flex_1().text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child(label))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{:.0}%", used * 100.))),
                        )
                        .child(div().h(px(6.)).rounded(px(3.)).bg(tint.opacity(0.16)).child(div().h_full().rounded(px(3.)).w(gpui_kit::relative(used as f32)).bg(tint))),
                );
            }
        }
        let figures = div()
            .flex()
            .flex_wrap()
            .gap(px(space::M))
            .child(page_kit::stat_tile(
                "Tokens",
                tokens(t.tokens.total() as f64),
                Some(format!(
                    "Input {} · Output {} · Cache read {} · Cache write {}",
                    tokens(t.tokens.input as f64),
                    tokens(t.tokens.output as f64),
                    tokens(t.tokens.cache_read as f64),
                    tokens((t.tokens.cache_5m + t.tokens.cache_1h) as f64)
                )),
                &theme,
            ))
            .child(page_kit::stat_tile("At API prices", format!("${:.2}", t.cost), Some(if t.unpriced > 0 { format!("{} tokens of unpriced models", tokens(t.unpriced as f64)) } else { "What the tokens would cost on the API, not what you pay".into() }), &theme))
            .child(page_kit::stat_tile("Replies", t.messages.to_string(), Some(format!("{} a day on days used", self.format(self.value(t) / active_days as f64))), &theme));
        // By day: days with usage, the newest first, a page at a time.
        let used_days: Vec<(usize, &yh_core::usage::Day)> = report.days.iter().enumerate().filter(|(_, d)| d.total.messages > 0).rev().collect();
        let pages = used_days.len().div_ceil(DAYS_PER_PAGE).max(1);
        let day_page = self.day_page.min(pages - 1);
        let shown: Vec<(usize, &yh_core::usage::Day)> = used_days.iter().skip(day_page * DAYS_PER_PAGE).take(DAYS_PER_PAGE).cloned().collect();
        let wide = !narrow;
        let cell = |t: String, w: f32| div().w(px(w)).flex_none().text_right().child(t);
        let mut by_day = page_kit::card(&theme).px(px(space::L)).py(px(space::M)).flex().flex_col().child(
            div()
                .pb(px(6.))
                .flex()
                .items_center()
                .gap(px(space::S))
                .child(div().flex().flex_col().child(page_kit::section_title("By day".to_string(), None, &theme)).child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("{} day{} with usage, the newest first", used_days.len(), if used_days.len() == 1 { "" } else { "s" }))))
                .child(div().flex_1())
                .when(pages > 1, |d| {
                    let (p1, p2) = (page.clone(), page.clone());
                    d.child(div().text_size(px(text::CAPTION)).text_color(theme.secondary).child(format!("{}–{} of {}", day_page * DAYS_PER_PAGE + 1, day_page * DAYS_PER_PAGE + shown.len(), used_days.len())))
                        .child(gpui_kit::component::button::Button::new("days-newer").xsmall().ghost().icon(gpui_kit::component::IconName::ChevronLeft).tooltip("Newer").disabled(day_page == 0).on_click(move |_, _, cx| {
                            let _ = p1.update(cx, |p, cx| {
                                p.day_page = day_page.saturating_sub(1);
                                cx.notify();
                            });
                        }))
                        .child(gpui_kit::component::button::Button::new("days-older").xsmall().ghost().icon(gpui_kit::component::IconName::ChevronRight).tooltip("Older").disabled(day_page + 1 >= pages).on_click(move |_, _, cx| {
                            let _ = p2.update(cx, |p, cx| {
                                p.day_page = day_page + 1;
                                cx.notify();
                            });
                        }))
                }),
        );
        if shown.is_empty() {
            by_day = by_day.child(div().py(px(space::S)).text_size(px(text::FOOTNOTE)).text_color(theme.secondary).child("No usage in this range."));
        } else {
            by_day = by_day.child(
                div()
                    .flex()
                    .gap(px(space::M))
                    .pb(px(6.))
                    .text_size(px(text::CAPTION))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.secondary)
                    .child(div().w(px(110.)).flex_none().child("Day"))
                    .child(cell("Tokens".into(), 64.))
                    .when(wide, |d| d.child(cell("Input".into(), 60.)).child(cell("Output".into(), 60.)).child(cell("Cache read".into(), 76.)).child(cell("Cache write".into(), 76.)))
                    .child(cell("Cost".into(), 64.))
                    .child(div().flex_1().pl(px(space::S)).child("Top model")),
            );
            for (i, d) in &shown {
                let tk = &d.total.tokens;
                let top = d.models.iter().max_by(|a, b| self.value(a.1).total_cmp(&self.value(b.1))).map(|(m, _)| m.clone());
                let more = d.models.len().saturating_sub(1);
                let top_color = top.as_ref().map(|m| series.get(key_of(m)).map(|s| s.2).unwrap_or(theme.tertiary));
                by_day = by_day.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::M))
                        .py(px(6.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .text_size(px(text::FOOTNOTE))
                        .child(div().w(px(110.)).flex_none().child(day_label(first + *i as i64)))
                        .child(cell(tokens(tk.total() as f64), 64.))
                        .when(wide, |row| {
                            row.child(cell(tokens(tk.input as f64), 60.).text_color(theme.secondary))
                                .child(cell(tokens(tk.output as f64), 60.).text_color(theme.secondary))
                                .child(cell(tokens(tk.cache_read as f64), 76.).text_color(theme.secondary))
                                .child(cell(tokens((tk.cache_5m + tk.cache_1h) as f64), 76.).text_color(theme.secondary))
                        })
                        .child(cell(format!("${:.2}", d.total.cost), 64.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pl(px(space::S))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .when_some(top_color, |r, c| r.child(div().w(px(7.)).h(px(7.)).rounded_full().bg(c)))
                                .child(div().truncate().child(top.as_deref().map(display_name).unwrap_or_else(|| "–".into())))
                                .when(more > 0, |r| r.child(div().text_color(theme.tertiary).child(format!("+{more}")))),
                        ),
                );
            }
        }
        let footnote = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .text_size(px(text::CAPTION))
            .text_color(theme.tertiary)
            .child("Costs are what the same tokens would cost on Anthropic's API, not what your plan charges. From Claude Code's session files and this app's side chats and memory agents.")
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(format!("Prices checked {PRICES_CHECKED}:"))
                    .child(div().id("usage-prices").text_color(theme.accent).child("Anthropic API pricing").on_click(|_, _, cx| cx.open_url(PRICES_SOURCE))),
            );
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context("UsagePage")
            .on_action(cx.listener(|p, _: &CloseUsage, _, cx| p.close(cx)))
            .flex()
            .flex_col()
            .child(header)
            .child(self.width.probe(page.clone()))
            .child(
                div().id("usage-body").flex_1().min_h_0().overflow_y_scroll().child(
                    div()
                        .max_w(px(1040.))
                        .w_full()
                        .mx_auto()
                        .px(px(if narrow { space::L } else { space::XL }))
                        .pt(px(space::L + space::XS))
                        .pb(px(space::XL))
                        .flex()
                        .flex_col()
                        .gap(px(space::M + 2.))
                        .when(any_limit, |d| d.child(limit_row))
                        .child(figures)
                        .child(chart_card)
                        .child(table)
                        .child(by_day)
                        .child(footnote),
                ),
            )
            .into_any_element()
    }
}
