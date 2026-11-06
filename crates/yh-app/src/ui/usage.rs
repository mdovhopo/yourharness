//! Usage (⇧⌘U), in place of the open thread: the 5-hour and weekly limits, then what was
//! spent over a range of days, in tokens or in dollars at API prices, by model and by day.
//! Read from Claude Code's session files into `usage.sqlite` off the main thread
//! (`yh_core::usage`). The chart is one canvas of quads; hovering finds the day from the
//! pointer, so moving along a year of bars redraws the band and the tooltip, nothing else.

use crate::state::workspace::Workspace;
use crate::theme::Theme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::Sizable;
use gpui_kit::{
    canvas, div, fill, point, prelude::*, px, rgb, size, Bounds, Context, Entity, FontWeight, Hsla, Pixels, SharedString, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use yh_core::usage::{civil, display_name, Report, UsageStore};

const RANGES: [(&str, usize); 5] = [("Week", 7), ("Month", 30), ("Quarter", 90), ("Half Year", 182), ("Year", 365)];

/// The categorical palette (light, dark), in the order checked for color blindness (the
/// Swift app's; validated with the dataviz script against both surfaces). Past seven
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
}

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
                cx.notify();
            });
        })
        .detach();
    }

    fn set_range(&mut self, days: usize, cx: &mut Context<Self>) {
        self.days = days;
        self.hovered = None;
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

impl Render for UsagePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let dark = theme.canvas.l < 0.5;
        let color = |c: (u32, u32)| -> Hsla { rgb(if dark { c.1 } else { c.0 }).into() };
        let mut ranges = div().flex().gap(px(2.));
        for (label, days) in RANGES {
            let b = Button::new(SharedString::from(format!("range-{days}"))).small().label(label);
            ranges = ranges.child((if self.days == days { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| this.set_range(days, cx))));
        }
        let metric = |id: &'static str, label: &'static str, cost: bool, cx: &mut Context<Self>| {
            let b = Button::new(id).small().label(label);
            (if self.cost == cost { b.primary() } else { b.ghost() }).on_click(cx.listener(move |this, _, _, cx| {
                this.cost = cost;
                cx.notify();
            }))
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(28.))
            .py(px(16.))
            .border_b_1()
            .border_color(theme.separator)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child("Usage"))
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.reading.clone().unwrap_or_else(|| "Every Claude Code session, here or in a terminal".into())),
                    ),
            )
            .child(ranges)
            .child(div().flex().gap(px(2.)).child(metric("metric-tokens", "Tokens", false, cx)).child(metric("metric-cost", "Cost", true, cx)));
        let Some(report) = &self.report else {
            return div().size_full().flex().flex_col().child(header).child(div().flex_1().flex().items_center().justify_center().text_color(theme.secondary).child("Reading…")).into_any_element();
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
        let max = stacks.iter().map(|s| s.iter().map(|(_, v)| v).sum::<f64>()).fold(0., f64::max).max(1e-9);
        let colors: Vec<Hsla> = series.iter().map(|(_, _, c)| *c).collect();
        let bounds_cell = self.chart.clone();
        let hovered = self.hovered;
        let band = theme.selected;
        let n = stacks.len();
        let chart = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width) / n.max(1) as f32;
                let h = f32::from(bounds.size.height);
                let bar = (w - if w > 6. { 2. } else { 0.5 }).max(0.5);
                if let Some(i) = hovered {
                    window.paint_quad(fill(Bounds::new(point(bounds.origin.x + px(i as f32 * w), bounds.origin.y), size(px(w), bounds.size.height)), band));
                }
                for (i, stack) in stacks.iter().enumerate() {
                    let x = bounds.origin.x + px(i as f32 * w + (w - bar) / 2.);
                    let mut y = f32::from(bounds.origin.y) + h;
                    let last = stack.len().saturating_sub(1);
                    for (j, (k, v)) in stack.iter().enumerate() {
                        let seg = (*v / max) as f32 * (h - 4.);
                        if seg < 0.5 {
                            continue;
                        }
                        // A 2px surface gap between stacked segments; the top one rounded.
                        let gap = if j < last && seg > 3. { 2. } else { 0. };
                        let quad = fill(Bounds::new(point(x, px(y - seg)), size(px(bar), px(seg - gap))), colors[*k]);
                        window.paint_quad(if j == last && bar > 4. { quad.corner_radii(px(2.)) } else { quad });
                        y -= seg;
                    }
                }
            },
        )
        .size_full();
        let cell = self.chart.clone();
        let chart_box = div()
            .id("usage-chart")
            .h(px(220.))
            .w_full()
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
        let first = report.first_day;
        let axis = div()
            .flex()
            .justify_between()
            .text_size(px(11.))
            .text_color(theme.secondary)
            .child(day_label(first))
            .child(day_label(first + n as i64 / 2))
            .child(day_label(first + n as i64 - 1));
        // The hovered day, or the range: one line per model.
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
        let (title, rows): (String, Vec<(Hsla, String, f64, i64)>) = match self.hovered.and_then(|i| report.days.get(i).map(|d| (i, d))) {
            Some((i, d)) => (format!("{} · {}", day_label(first + i as i64), self.format(self.value(&d.total))), rows_of(d.models.iter().collect())),
            None => (format!("Last {} days · {}", report.days.len(), self.format(self.value(&report.total))), rows_of(report.models.iter().map(|(m, s)| (m, s)).collect())),
        };
        let total_value = self.value(&report.total).max(1e-9);
        let mut table = div().flex().flex_col().gap(px(2.)).child(div().font_weight(FontWeight::SEMIBOLD).pb(px(4.)).child(title));
        for (c, name, v, toks) in rows {
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(2.))
                    .child(div().flex_none().w(px(10.)).h(px(10.)).rounded(px(2.)).bg(c))
                    .child(div().flex_1().child(name))
                    .child(div().w(px(90.)).text_right().child(self.format(v)))
                    .child(div().w(px(80.)).text_right().text_color(theme.secondary).child(if self.cost { tokens(toks as f64) } else { String::new() }))
                    .child(div().w(px(50.)).text_right().text_color(theme.secondary).child(if self.hovered.is_none() { format!("{:.0}%", v / total_value * 100.) } else { String::new() })),
            );
        }
        let figure = |label: &str, value: String, note: String| {
            div()
                .flex_1()
                .p(px(12.))
                .rounded(px(8.))
                .bg(theme.card)
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(11.)).text_color(theme.secondary).child(label.to_string()))
                .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(value))
                .child(div().text_size(px(11.)).text_color(theme.secondary).child(note))
        };
        let active_days = report.days.iter().filter(|d| d.total.messages > 0).count().max(1);
        let t = &report.total;
        let limits = self.workspace.read(cx).limits.clone();
        let mut limit_row = div().flex().gap(px(12.));
        if let Some(windows) = limits.as_ref().and_then(|l| l["unifiedWindows"].as_object().cloned()) {
            for (name, w) in windows {
                let used = w["utilization"].as_f64().unwrap_or(0.).clamp(0., 1.);
                let label = match name.as_str() {
                    "five_hour" => "5-hour limit".to_string(),
                    "seven_day" => "Weekly limit".to_string(),
                    other => other.replace('_', " "),
                };
                limit_row = limit_row.child(
                    div()
                        .flex_1()
                        .p(px(12.))
                        .rounded(px(8.))
                        .bg(theme.card)
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(div().flex().justify_between().child(label).child(format!("{:.0}%", used * 100.)))
                        .child(div().h(px(6.)).rounded(px(3.)).bg(theme.selected).child(div().h_full().rounded(px(3.)).w(gpui_kit::relative(used as f32)).bg(if used > 0.8 { theme.warning } else { theme.accent }))),
                );
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("usage-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(28.))
                    .py(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(limit_row)
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(figure("Tokens", tokens(t.tokens.total() as f64), format!("{} out · {} cache reads", tokens(t.tokens.output as f64), tokens(t.tokens.cache_read as f64))))
                            .child(figure("At API prices", format!("${:.2}", t.cost), if t.unpriced > 0 { format!("{} tokens of unpriced models", tokens(t.unpriced as f64)) } else { "what the tokens would cost on the API, not what you pay".into() }))
                            .child(figure("Replies", t.messages.to_string(), format!("{} a day on days used", self.format(self.value(t) / active_days as f64)))),
                    )
                    .child(div().flex().flex_col().gap(px(6.)).child(chart_box).child(axis))
                    .child(table),
            )
            .into_any_element()
    }
}
