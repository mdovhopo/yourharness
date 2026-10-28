import Charts
import HarnessCore
import SwiftUI

/// The sidebar's usage box, opened up over the window: the limits, then what was spent over
/// a range of days, in tokens and in dollars at API prices, by model and by day.
struct UsageOverlay: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        ZStack {
            // A click outside closes it, as with ⌘K.
            Color.black.opacity(0.2)
                .ignoresSafeArea()
                .onTapGesture { app.tokenUsage.close() }
            UsageView(usage: app.tokenUsage)
                .background {
                    ZStack {
                        Color(nsColor: .windowBackgroundColor)
                        Color.canvas
                    }
                }
                .clipShape(.rect(cornerRadius: Theme.floatingRadius, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: Theme.floatingRadius, style: .continuous)
                        .strokeBorder(.separator.opacity(0.6))
                }
                .shadow(color: .black.opacity(0.22), radius: 28, y: 10)
                .padding(.horizontal, 28)
                .padding(.vertical, 22)
        }
    }
}

/// Tokens or dollars: what the chart and the shares measure.
enum UsageMetric: String, CaseIterable, Identifiable {
    case tokens, cost

    var id: Self { self }
    var label: String { self == .tokens ? "Tokens" : "Cost" }

    func value(_ spend: UsageReport.Spend) -> Double {
        self == .tokens ? Double(spend.tokens.total) : spend.cost
    }

    func format(_ value: Double) -> String {
        self == .tokens ? UsageFormat.tokens(Int(value.rounded())) : UsageFormat.cost(value)
    }

    /// On an axis: whole dollars once there are some.
    func axisLabel(_ value: Double) -> String {
        self == .cost && value >= 10 ? value.formatted(.currency(code: "USD").precision(.fractionLength(0))) : format(value)
    }
}

enum UsageFormat {
    /// "12.4M", "830K", "912".
    static func tokens(_ count: Int) -> String {
        count.formatted(.number.notation(.compactName).precision(.significantDigits(1...3)))
    }

    static func cost(_ dollars: Double) -> String {
        if dollars == 0 { return "$0" }
        if dollars < 0.01 { return "<$0.01" }
        return dollars.formatted(.currency(code: "USD").precision(.fractionLength(dollars >= 10_000 ? 0 : 2)))
    }
}

/// A model's place in the chart: its name and color. Colors follow the model, by how much it
/// has been used all told, so changing the range never repaints one; past seven, the rest
/// share "Other".
struct UsageSeries: Identifiable, Equatable {
    static let otherKey = "other"

    let key: String
    let name: String
    let color: Color
    var id: String { key }

    /// The categorical palette, in an order checked for color blindness, with its own steps
    /// for dark mode.
    private static let palette: [Color] = [
        (0x2A78D6, 0x3987E5), (0xEB6834, 0xD95926), (0x1BAF7A, 0x199E70), (0xEDA100, 0xC98500),
        (0xE87BA4, 0xD55181), (0x008300, 0x008300), (0x6250D6, 0x9085E9),
    ].map { Color(light: $0.0, dark: $0.1) }

    static let other = UsageSeries(key: otherKey, name: "Other", color: Color(light: 0x9A9A9F, dark: 0x6E6E73))

    /// The series for every model in `report`, in color order.
    static func all(in report: UsageReport) -> [UsageSeries] {
        let used = Set(report.models.map(\.model))
        let ranked = report.allModels + report.models.map(\.model).filter { !report.allModels.contains($0) }
        var series: [UsageSeries] = []
        var hasOther = false
        for (index, model) in ranked.enumerated() where used.contains(model) {
            if index < palette.count {
                series.append(UsageSeries(key: model, name: ModelPricing.displayName(model), color: palette[index]))
            } else {
                hasOther = true
            }
        }
        return hasOther ? series + [other] : series
    }

    /// Which series a model's usage goes to.
    static func key(for model: String, in series: [UsageSeries]) -> String {
        series.contains { $0.key == model } ? model : otherKey
    }
}

private extension Color {
    init(light: UInt32, dark: UInt32) {
        self.init(nsColor: NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
        })
    }
}

struct UsageView: View {
    @Environment(AppModel.self) private var app
    @Bindable var usage: UsageModel
    @State private var metric = ProcessInfo.processInfo.environment["YH_USAGE_METRIC"].flatMap(UsageMetric.init(rawValue:)) ?? .tokens
    @State private var split = ProcessInfo.processInfo.environment["YH_USAGE_SPLIT"] != nil
    @State private var width: CGFloat = 1000

    private var report: UsageReport { usage.report }
    private var series: [UsageSeries] { UsageSeries.all(in: report) }
    private var isWide: Bool { width > 860 }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 24)
                .padding(.top, 18)
                .padding(.bottom, 14)
            Divider().opacity(0.6)
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    UsageLimits()
                    tiles
                    if isWide {
                        // One height for both: the chart takes what the list needs.
                        HStack(alignment: .top, spacing: 16) {
                            chartCard(fills: true)
                            UsageModelsCard(report: report, series: series, metric: metric, fills: true)
                                .frame(width: 300)
                        }
                        .fixedSize(horizontal: false, vertical: true)
                    } else {
                        chartCard(fills: false)
                        UsageModelsCard(report: report, series: series, metric: metric)
                    }
                    UsageDaysCard(report: report, series: series, isWide: isWide)
                    footnote
                }
                .frame(maxWidth: 1180, alignment: .leading)
                .padding(24)
                .frame(maxWidth: .infinity)
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
        }
        .task {
            // Open, it keeps up with the threads still working.
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                usage.refresh()
            }
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Usage")
                    .font(.title2.weight(.semibold))
                Text(subtitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 12)
            if let progress = usage.progress {
                HStack(spacing: 8) {
                    ProgressView(value: Double(progress.read), total: Double(max(progress.total, 1)))
                        .frame(width: 90)
                    Text("Reading \(progress.read.formatted()) of \(progress.total.formatted()) sessions")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
                .transition(.opacity)
            }
            Picker("Range", selection: $usage.range) {
                ForEach(UsageModel.Range.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .controlSize(.large)
            .fixedSize()
            Button("Refresh", systemImage: "arrow.clockwise") { usage.refresh() }
                .labelStyle(.iconOnly)
                .controlSize(.large)
                .symbolEffect(.rotate, options: .repeat(.continuous), isActive: usage.isReading)
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .disabled(usage.isReading)
                .help(usage.updatedAt.map { "Read Claude Code's history again\nUpdated \($0.formatted(.relative(presentation: .named)))" }
                    ?? "Read Claude Code's history again")
            Button("Close", systemImage: "xmark") { usage.close() }
                .labelStyle(.iconOnly)
                .controlSize(.large)
                .buttonStyle(.glass)
                .buttonBorderShape(.circle)
                .keyboardShortcut(.cancelAction)
                .help("Close (Esc)")
        }
        .animation(.smooth(duration: 0.25), value: usage.progress?.total)
    }

    private var subtitle: String {
        guard report.start != .distantPast else { return usage.range.title }
        let dates = "\(report.start.formatted(.dateTime.month(.abbreviated).day())) – \(report.end.formatted(.dateTime.month(.abbreviated).day().year()))"
        return "\(usage.range.title) · \(dates)"
    }

    // MARK: - Totals

    private var tiles: some View {
        let total = report.total
        let activeDays = report.days.count { $0.total.messages > 0 }
        let perDay = total.cost / Double(max(report.days.count, 1))
        return HStack(alignment: .top, spacing: 12) {
            UsageTile(label: "Tokens", value: UsageFormat.tokens(total.tokens.total)) {
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 1) {
                    GridRow {
                        tokenPart("Input", total.tokens.input)
                        tokenPart("Cache read", total.tokens.cacheRead)
                    }
                    GridRow {
                        tokenPart("Output", total.tokens.output)
                        tokenPart("Cache write", total.tokens.cacheWrite)
                    }
                }
            }
            UsageTile(label: "Estimated cost", value: UsageFormat.cost(total.cost)) {
                Text("at API prices, \(UsageFormat.cost(perDay)) a day")
                if total.unpricedTokens > 0 {
                    Text("\(UsageFormat.tokens(total.unpricedTokens)) tokens of models with no price")
                }
            }
            UsageTile(label: "Responses", value: total.messages.formatted()) {
                Text("on \(activeDays) of \(report.days.count) days")
            }
        }
        // As tall as the tallest.
        .fixedSize(horizontal: false, vertical: true)
    }

    private func tokenPart(_ label: String, _ count: Int) -> some View {
        HStack(spacing: 4) {
            Text(label).foregroundStyle(.tertiary)
            Text(UsageFormat.tokens(count)).monospacedDigit().foregroundStyle(.secondary)
        }
        .lineLimit(1)
    }

    // MARK: - Chart

    /// `fills`: as tall as it's given, the chart taking the room.
    private func chartCard(fills: Bool) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Per day").font(.headline)
                    Text(metric == .tokens ? "Tokens, cache included" : "Estimated at API prices")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Picker("Measure", selection: $metric) {
                    ForEach(UsageMetric.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Picker("Layout", selection: $split) {
                    Text("Stacked").tag(false)
                    Text("By Model").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            if report.total.messages == 0 {
                VStack(spacing: 6) {
                    Image(systemName: "chart.bar.xaxis")
                        .font(.title)
                        .foregroundStyle(.tertiary)
                    Text(usage.isReading ? "Reading Claude Code's history…" : "No usage in the \(usage.range.title.lowercased())")
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: 260)
            } else if split {
                UsageSplitChart(report: report, series: series, metric: metric, range: usage.range)
            } else {
                UsageStackedChart(report: report, series: series, metric: metric, range: usage.range)
                    .frame(minHeight: 260, maxHeight: fills ? .infinity : 260)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: fills ? .infinity : nil, alignment: .topLeading)
        .card()
    }

    private var footnote: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("Costs are what the same tokens would cost on Anthropic's API, not what your plan charges. From Claude Code's session files and this app's side chats and memory agents.")
            HStack(spacing: 4) {
                Text("Prices checked \(ModelPricing.checked):")
                Link("Anthropic API pricing", destination: ModelPricing.source)
            }
        }
        .font(.caption)
        .foregroundStyle(.tertiary)
    }
}

// MARK: - Limits

/// The 5-hour and weekly limits, as the sidebar shows them, with when each resets.
private struct UsageLimits: View {
    @Environment(AppModel.self) private var app

    private var windows: [(name: String, window: RateLimitInfo.Window?)] {
        let primary = ["five_hour", "seven_day"]
        let reported = app.rateLimit?.windows ?? []
        return primary.map { name in (name, reported.first { $0.name == name }) }
            + reported.filter { !primary.contains($0.name) }.map { ($0.name, $0) }
    }

    var body: some View {
        TimelineView(.everyMinute) { context in
            HStack(spacing: 12) {
                ForEach(windows, id: \.name) { item in
                    LimitCard(name: item.name, window: item.window, now: context.date)
                }
            }
        }
    }
}

private struct LimitCard: View {
    @Environment(AppModel.self) private var app
    let name: String
    let window: RateLimitInfo.Window?
    let now: Date

    private var hasReset: Bool { window?.resetsAt.map { $0 <= now } ?? false }
    private var utilization: Double? {
        guard let window else { return nil }
        return hasReset ? 0 : min(max(window.utilization, 0), 1)
    }

    private var resetText: String {
        guard let window else { return "No reading yet" }
        if hasReset { return "Reset since the last reading" }
        guard let resetsAt = window.resetsAt else { return " " }
        let when = Calendar.current.isDateInToday(resetsAt)
            ? resetsAt.formatted(date: .omitted, time: .shortened)
            : resetsAt.formatted(.dateTime.weekday(.abbreviated).hour().minute())
        return "Resets in \(LimitMeter.countdown(to: resetsAt, from: now)) · \(when)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(LimitMeter.title(name))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
                if name == "five_hour", let updated = app.rateLimitUpdatedAt {
                    Text("Read \(updated.formatted(.relative(presentation: .named)))")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
            }
            Text(utilization.map { $0.formatted(.percent.precision(.fractionLength(0))) } ?? "–")
                .font(.system(size: 26, weight: .semibold).monospacedDigit())
                .contentTransition(.numericText())
            GeometryReader { proxy in
                ZStack(alignment: .leading) {
                    Capsule().fill(.fill.tertiary)
                    if let utilization {
                        Capsule()
                            .fill(LimitMeter.tint(utilization))
                            .frame(width: max(proxy.size.width * utilization, 6))
                    }
                }
            }
            .frame(height: 6)
            Text(resetText)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .card()
    }
}

// MARK: - Tiles

/// A figure with its label, and what to know about it under it.
private struct UsageTile<Detail: View>: View {
    let label: String
    let value: String
    @ViewBuilder let detail: Detail

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.callout).foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 26, weight: .semibold).monospacedDigit())
                .contentTransition(.numericText())
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            VStack(alignment: .leading, spacing: 1) { detail }
                .font(.caption)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(14)
        .card()
    }
}

// MARK: - Charts

/// One bar's worth: a day and a series.
private struct UsagePoint: Identifiable {
    let day: Date
    let series: UsageSeries
    let value: Double
    var id: String { "\(day.timeIntervalSince1970)-\(series.key)" }
}

private func usagePoints(_ report: UsageReport, _ series: [UsageSeries], _ metric: UsageMetric) -> [UsagePoint] {
    report.days.flatMap { day in
        var values: [String: Double] = [:]
        for (model, spend) in day.models {
            values[UsageSeries.key(for: model, in: series), default: 0] += metric.value(spend)
        }
        return series.compactMap { item in values[item.key].map { UsagePoint(day: day.date, series: item, value: $0) } }
    }
}

private extension UsageModel.Range {
    /// How far apart the day labels are.
    var labelStride: (component: Calendar.Component, count: Int) {
        switch self {
        case .week: (.day, 1)
        case .month: (.day, 7)
        case .quarter: (.day, 14)
        case .halfYear: (.month, 1)
        case .year: (.month, 2)
        }
    }

    var labelFormat: Date.FormatStyle {
        switch self {
        case .week: .dateTime.weekday(.abbreviated)
        case .month, .quarter: .dateTime.month(.abbreviated).day()
        case .halfYear, .year: .dateTime.month(.abbreviated)
        }
    }
}

/// The day under the pointer, shared by a chart's layers. Only the band behind the bars and
/// the tooltip over them read it, so moving along a chart redraws those, not a year of bars
/// (which took two seconds a move).
@MainActor @Observable
private final class UsageHover {
    /// A day's start, as the report's days are.
    var day: Date?

    init(day: Date? = nil) {
        self.day = day
    }

    /// Development: `YH_USAGE_HOVER=<days ago>` hovers a day, for checking the tooltip without
    /// a mouse; the pointer is ignored then, so a snapshot shows that day wherever it is.
    static let isPinned = ProcessInfo.processInfo.environment["YH_USAGE_HOVER"] != nil

    static func initial() -> UsageHover {
        let day = ProcessInfo.processInfo.environment["YH_USAGE_HOVER"].flatMap(Int.init).flatMap {
            Calendar.current.date(byAdding: .day, value: -$0, to: Calendar.current.startOfDay(for: .now))
        }
        return UsageHover(day: day)
    }
}

private func usageChartEnd(_ report: UsageReport) -> Date {
    report.days.last.flatMap { Calendar.current.date(byAdding: .day, value: 1, to: $0.date) } ?? report.end
}

/// Behind the bars: a band on the hovered day.
private struct UsageHoverBand: View {
    let proxy: ChartProxy
    let hover: UsageHover
    var minimumWidth: CGFloat = 3

    var body: some View {
        GeometryReader { geometry in
            if let day = hover.day, let plotFrame = proxy.plotFrame,
               let next = Calendar.current.date(byAdding: .day, value: 1, to: day),
               let start = proxy.position(forX: day), let end = proxy.position(forX: next) {
                let plot = geometry[plotFrame]
                Rectangle()
                    .fill(.secondary.opacity(0.16))
                    .frame(width: max(end - start, minimumWidth), height: plot.height)
                    .position(x: plot.minX + (start + end) / 2, y: plot.midY)
            }
        }
        .allowsHitTesting(false)
    }
}

/// Over the bars: follows the pointer into `hover`, and shows `tooltip` for the day.
private struct UsageHoverLayer<Tooltip: View>: View {
    let proxy: ChartProxy
    let hover: UsageHover
    let report: UsageReport
    @ViewBuilder let tooltip: (UsageReport.Day) -> Tooltip
    @State private var tooltipSize = CGSize.zero

    var body: some View {
        GeometryReader { geometry in
            if let plotFrame = proxy.plotFrame {
                let plot = geometry[plotFrame]
                Rectangle()
                    .fill(.clear)
                    .contentShape(.rect)
                    .frame(width: plot.width, height: plot.height)
                    .position(x: plot.midX, y: plot.midY)
                    .onContinuousHover { phase in
                        guard !UsageHover.isPinned else { return }
                        switch phase {
                        case .active(let location):
                            let date: Date? = proxy.value(atX: location.x)
                            let day = date.flatMap { report.day(containing: $0)?.date }
                            if hover.day != day { hover.day = day }
                        case .ended:
                            if hover.day != nil { hover.day = nil }
                        }
                    }
                if let day = hover.day, let entry = report.day(containing: day),
                   let next = Calendar.current.date(byAdding: .day, value: 1, to: day),
                   let start = proxy.position(forX: day), let end = proxy.position(forX: next) {
                    let center = plot.minX + (start + end) / 2
                    let half = tooltipSize.width / 2
                    tooltip(entry)
                        .fixedSize()
                        .onGeometryChange(for: CGSize.self) { $0.size } action: { tooltipSize = $0 }
                        // Beside the day, kept inside the chart.
                        .position(x: min(max(center, plot.minX + half), plot.maxX - half),
                                  y: plot.minY + tooltipSize.height / 2 + 4)
                        .allowsHitTesting(false)
                }
            }
        }
    }
}

private struct UsageStackedChart: View {
    let report: UsageReport
    let series: [UsageSeries]
    let metric: UsageMetric
    let range: UsageModel.Range
    @State private var hover = UsageHover.initial()

    var body: some View {
        let points = usagePoints(report, series, metric)
        Chart(points) { point in
            BarMark(x: .value("Day", point.day, unit: .day), y: .value(metric.label, point.value))
                .foregroundStyle(by: .value("Model", point.series.name))
        }
        .chartForegroundStyleScale(domain: series.map(\.name), range: series.map(\.color))
        .chartLegend(position: .bottom, alignment: .leading, spacing: 10)
        .chartXScale(domain: report.start...usageChartEnd(report))
        .chartYAxis {
            AxisMarks(position: .leading) { value in
                AxisGridLine().foregroundStyle(.separator.opacity(0.5))
                AxisValueLabel { if let number = value.as(Double.self) { Text(metric.axisLabel(number)) } }
            }
        }
        .chartXAxis {
            AxisMarks(values: .stride(by: range.labelStride.component, count: range.labelStride.count)) {
                AxisValueLabel(format: range.labelFormat, centered: range == .week)
            }
        }
        .chartBackground { proxy in UsageHoverBand(proxy: proxy, hover: hover) }
        .chartOverlay { proxy in
            UsageHoverLayer(proxy: proxy, hover: hover, report: report) { day in
                UsageDayTooltip(day: day, series: series, metric: metric)
            }
        }
        .animation(.smooth(duration: 0.3), value: metric)
        .onAppear { UsageProbe.register(hover) { $0.day } }
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { if UsageProbe.isOn { UsageProbe.chartFrame = $0 } }
    }
}

/// Small multiples: a row per model on one scale, so each one's days can be read on their own.
private struct UsageSplitChart: View {
    let report: UsageReport
    let series: [UsageSeries]
    let metric: UsageMetric
    let range: UsageModel.Range
    @State private var hover = UsageHover.initial()

    var body: some View {
        let points = usagePoints(report, series, metric)
        let rows = Dictionary(grouping: points) { $0.series.key }
        let top = max(points.map(\.value).max() ?? 0, metric == .tokens ? 1 : 0.01)
        VStack(spacing: 10) {
            ForEach(Array(series.enumerated()), id: \.element.id) { index, item in
                let rows = rows[item.key] ?? []
                let isLast = index == series.count - 1
                HStack(alignment: .top, spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Circle().fill(item.color).frame(width: 8, height: 8)
                            Text(item.name).font(.callout.weight(.medium)).lineLimit(1)
                        }
                        UsageSplitLabel(hover: hover, metric: metric, total: rows.reduce(0) { $0 + $1.value },
                                        byDay: Dictionary(rows.map { ($0.day, $0.value) }) { $0 + $1 })
                    }
                    .frame(width: 130, alignment: .leading)
                    Chart(rows) { point in
                        BarMark(x: .value("Day", point.day, unit: .day), y: .value(metric.label, point.value))
                            .foregroundStyle(item.color)
                            .cornerRadius(range == .week || range == .month ? 3 : 1)
                    }
                    .chartXScale(domain: report.start...usageChartEnd(report))
                    .chartYScale(domain: 0...top)
                    .chartYAxis {
                        // One scale for every row, labeled once.
                        AxisMarks(position: .leading, values: [top]) { value in
                            AxisGridLine().foregroundStyle(.separator.opacity(0.4))
                            AxisValueLabel {
                                if let number = value.as(Double.self) {
                                    Text(metric.axisLabel(number)).opacity(index == 0 ? 1 : 0)
                                }
                            }
                        }
                    }
                    .chartXAxis {
                        if isLast {
                            AxisMarks(values: .stride(by: range.labelStride.component, count: range.labelStride.count)) {
                                AxisValueLabel(format: range.labelFormat, centered: range == .week)
                            }
                        } else {
                            AxisMarks(values: .stride(by: range.labelStride.component, count: range.labelStride.count)) {
                                AxisGridLine().foregroundStyle(.clear)
                            }
                        }
                    }
                    .chartBackground { proxy in UsageHoverBand(proxy: proxy, hover: hover, minimumWidth: 2) }
                    .chartOverlay { proxy in UsageHoverLayer(proxy: proxy, hover: hover, report: report) { _ in EmptyView() } }
                    .frame(height: isLast ? 64 : 46)
                    .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                        if UsageProbe.isOn, index == 0 { UsageProbe.chartFrame = $0 }
                    }
                }
            }
        }
        .onAppear { UsageProbe.register(hover) { $0.day } }
    }
}

/// A model's range total under its name, or the hovered day's.
private struct UsageSplitLabel: View {
    let hover: UsageHover
    let metric: UsageMetric
    let total: Double
    let byDay: [Date: Double]

    var body: some View {
        Group {
            if let day = hover.day {
                Text("\(day.formatted(.dateTime.month(.abbreviated).day())): \(metric.format(byDay[day] ?? 0))")
            } else {
                Text(metric.format(total))
            }
        }
        .font(.caption.monospacedDigit())
        .foregroundStyle(.secondary)
    }
}

private struct UsageDayTooltip: View {
    let day: UsageReport.Day
    let series: [UsageSeries]
    let metric: UsageMetric

    var body: some View {
        var values: [String: Double] = [:]
        for (model, spend) in day.models { values[UsageSeries.key(for: model, in: series), default: 0] += metric.value(spend) }
        // Top to bottom, as the bar stacks them.
        let rows = series.filter { (values[$0.key] ?? 0) > 0 }.reversed()
        return VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(day.date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()))
                    .font(.caption.weight(.semibold))
                Spacer(minLength: 12)
                Text(metric.format(metric.value(day.total)))
                    .font(.caption.weight(.semibold).monospacedDigit())
            }
            ForEach(rows) { item in
                HStack(spacing: 6) {
                    RoundedRectangle(cornerRadius: 2).fill(item.color).frame(width: 8, height: 8)
                    Text(item.name)
                    Spacer(minLength: 12)
                    Text(metric.format(values[item.key] ?? 0)).monospacedDigit()
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            if rows.isEmpty {
                Text("Nothing this day").font(.caption).foregroundStyle(.secondary)
            }
        }
        .frame(minWidth: 150)
        .padding(.horizontal, 9)
        .padding(.vertical, 7)
        // Thick enough that the bars and the band behind don't show through the figures.
        .background(.thickMaterial, in: .rect(cornerRadius: Theme.innerRadius))
        .overlay { RoundedRectangle(cornerRadius: Theme.innerRadius).strokeBorder(.separator.opacity(0.5)) }
    }
}

// MARK: - By model

private struct UsageModelsCard: View {
    let report: UsageReport
    let series: [UsageSeries]
    let metric: UsageMetric
    /// As tall as it's given, beside the chart.
    var fills = false
    @State private var showsAll = false

    /// Shown before the rest fold into one row: about as many as sit beside the chart.
    private static let shown = 6

    var body: some View {
        let whole = metric.value(report.total)
        let models = report.models.sorted { metric.value($0.spend) > metric.value($1.spend) }
        let folds = models.count > Self.shown + 1
        let visible = folds && !showsAll ? Array(models.prefix(Self.shown)) : models
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("By model").font(.headline)
                Text("Share of \(metric == .tokens ? "tokens" : "cost")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if report.models.isEmpty {
                Text("No models used in this range.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 80)
            }
            ForEach(visible) { item in
                let color = series.first { $0.key == UsageSeries.key(for: item.model, in: series) }?.color ?? UsageSeries.other.color
                row(ModelPricing.displayName(item.model), spend: item.spend, color: color, whole: whole)
            }
            if folds {
                let rest = models.dropFirst(Self.shown)
                Button {
                    withAnimation(.smooth(duration: 0.25)) { showsAll.toggle() }
                } label: {
                    if showsAll {
                        Label("Show fewer", systemImage: "chevron.up")
                            .font(.callout)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        var spend = UsageReport.Spend()
                        let _ = rest.forEach { spend += $0.spend }
                        row("\(rest.count) more models", spend: spend, color: UsageSeries.other.color, whole: whole, isFold: true)
                    }
                }
                .buttonStyle(.plain)
                .contentShape(.rect)
                .help(showsAll ? "" : rest.map { ModelPricing.displayName($0.model) }.joined(separator: ", "))
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: fills ? .infinity : nil, alignment: .topLeading)
        .card()
        .animation(.smooth(duration: 0.3), value: metric)
        .onChange(of: report.start) { showsAll = false }
    }

    private func row(_ name: String, spend: UsageReport.Spend, color: Color, whole: Double, isFold: Bool = false) -> some View {
        let share = whole > 0 ? metric.value(spend) / whole : 0
        return VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                if isFold {
                    Image(systemName: "chevron.down").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                } else {
                    Circle().fill(color).frame(width: 8, height: 8)
                }
                Text(name)
                    .font(.callout.weight(isFold ? .regular : .medium))
                    .foregroundStyle(isFold ? .secondary : .primary)
                    .lineLimit(1)
                Spacer(minLength: 6)
                Text(UsageFormat.tokens(spend.tokens.total))
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .fixedSize()
                Text(spend.unpricedTokens > 0 && spend.cost == 0 ? "–" : UsageFormat.cost(spend.cost))
                    .font(.callout.monospacedDigit())
                    .fixedSize()
                    .frame(minWidth: 64, alignment: .trailing)
                    .help(spend.unpricedTokens > 0 ? "Not in the pricing table" : "")
            }
            HStack(spacing: 8) {
                GeometryReader { proxy in
                    ZStack(alignment: .leading) {
                        Capsule().fill(.fill.tertiary)
                        Capsule().fill(color).frame(width: max(proxy.size.width * share, share > 0 ? 4 : 0))
                    }
                }
                .frame(height: 5)
                Text(share.formatted(.percent.precision(.fractionLength(0))))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 36, alignment: .trailing)
            }
        }
    }
}

// MARK: - By day

private struct UsageDaysCard: View {
    let report: UsageReport
    let series: [UsageSeries]
    let isWide: Bool
    @State private var page = 0

    private static let pageSize = 10

    /// Days with usage, the newest first.
    private var days: [UsageReport.Day] { report.days.filter { $0.total.messages > 0 }.reversed() }
    private var pages: Int { max(1, (days.count + Self.pageSize - 1) / Self.pageSize) }

    var body: some View {
        let days = days
        let page = min(page, pages - 1)
        let shown = days.dropFirst(page * Self.pageSize).prefix(Self.pageSize)
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("By day").font(.headline)
                    Text("\(days.count) day\(days.count == 1 ? "" : "s") with usage, the newest first")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if pages > 1 {
                    Text("\((page * Self.pageSize + 1).formatted())–\((page * Self.pageSize + shown.count).formatted()) of \(days.count.formatted())")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                    ControlGroup {
                        Button("Newer", systemImage: "chevron.left") { self.page = page - 1 }
                            .disabled(page == 0)
                        Button("Older", systemImage: "chevron.right") { self.page = page + 1 }
                            .disabled(page >= pages - 1)
                    }
                    .controlGroupStyle(.navigation)
                    .fixedSize()
                }
            }
            if days.isEmpty {
                Text("No usage in this range.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 60)
            } else {
                Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 0) {
                    GridRow {
                        Text("Day")
                        Text("Tokens").gridColumnAlignment(.trailing)
                        if isWide {
                            Text("Input").gridColumnAlignment(.trailing)
                            Text("Output").gridColumnAlignment(.trailing)
                            Text("Cache read").gridColumnAlignment(.trailing)
                            Text("Cache write").gridColumnAlignment(.trailing)
                        }
                        Text("Cost").gridColumnAlignment(.trailing)
                        Text("Top model")
                    }
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 6)
                    ForEach(shown) { day in
                        Divider().gridCellUnsizedAxes(.horizontal)
                        GridRow {
                            Text(day.date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()))
                            Text(UsageFormat.tokens(day.total.tokens.total)).monospacedDigit()
                            if isWide {
                                Group {
                                    Text(UsageFormat.tokens(day.total.tokens.input))
                                    Text(UsageFormat.tokens(day.total.tokens.output))
                                    Text(UsageFormat.tokens(day.total.tokens.cacheRead))
                                    Text(UsageFormat.tokens(day.total.tokens.cacheWrite))
                                }
                                .monospacedDigit()
                                .foregroundStyle(.secondary)
                            }
                            Text(UsageFormat.cost(day.total.cost)).monospacedDigit()
                            topModel(day)
                        }
                        .font(.callout)
                        .padding(.vertical, 7)
                    }
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
        .onChange(of: report.start) { self.page = 0 }
    }

    @ViewBuilder
    private func topModel(_ day: UsageReport.Day) -> some View {
        if let model = day.topModel {
            let key = UsageSeries.key(for: model, in: series)
            HStack(spacing: 6) {
                Circle().fill(series.first { $0.key == key }?.color ?? UsageSeries.other.color).frame(width: 7, height: 7)
                Text(ModelPricing.displayName(model)).lineLimit(1)
                if day.models.count > 1 {
                    Text("+\(day.models.count - 1)").foregroundStyle(.tertiary)
                        .help(day.models.keys.filter { $0 != model }.map(ModelPricing.displayName).sorted().joined(separator: ", "))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            Text("–").foregroundStyle(.tertiary)
        }
    }
}
