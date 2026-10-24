import Charts
import HarnessCore
import SwiftUI

/// How memory is doing, in plain words first and figures after: what agents get each
/// session, what they read and search for, what never gets used, what looks duplicated.
struct MemoryInsightsView: View {
    @Environment(\.openWindow) private var openWindow
    let project: Project
    let library: MemoryLibrary

    var body: some View {
        if let stats = library.stats {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    tiles(stats)
                    HowItWorks(stats: stats) {
                        openWindow(id: "memory-agent", value: MemoryAgentKey(projectID: project.id, role: .reconciler))
                    }
                    HStack(alignment: .top, spacing: 16) {
                        ChartCard(title: "Memories over \(stats.window) days", detail: "In use at the end of each day") {
                            GrowthChart(days: stats.days)
                        }
                        ChartCard(title: "What agents did", detail: "Per day, last \(stats.window) days") {
                            ActivityChart(days: stats.days)
                        }
                    }
                    HStack(alignment: .top, spacing: 16) {
                        ChartCard(title: "By kind", detail: "Memories in use") { KindChart(byKind: stats.byKind) }
                        ChartCard(title: "By age", detail: "When they were first saved") { AgeChart(buckets: stats.ageBuckets) }
                    }
                    HStack(alignment: .top, spacing: 16) {
                        MemoryListCard(title: "Most read", empty: "No memory has been read yet.", items: stats.topRecalled, library: library) { item in
                            "\(item.recallCount)×"
                        }
                        MemoryListCard(title: "Never read", empty: "Everything older than a week has been read at least once.",
                                       items: Array(stats.neverRecalled.prefix(8)), library: library) { item in
                            item.createdAt.formatted(.relative(presentation: .named))
                        }
                    }
                    HStack(alignment: .top, spacing: 16) {
                        MemoryListCard(title: "Largest", empty: "", items: stats.largest, library: library) { item in
                            "\(item.bytes.formatted()) B"
                        }
                        MissedSearches(queries: stats.missedQueries)
                    }
                }
                .frame(maxWidth: 1000, alignment: .leading)
                .padding(24)
                .frame(maxWidth: .infinity)
            }
        }
    }

    private func tiles(_ stats: MemoryStats) -> some View {
        HStack(spacing: 12) {
            StatTile(label: "In use", value: stats.active.formatted(),
                     detail: stats.archived > 0 ? "\(stats.archived) archived" : "\(stats.pinned) pinned")
            StatTile(label: "Size", value: ByteCountFormatter.string(fromByteCount: Int64(stats.bytes), countStyle: .file),
                     detail: "\(stats.averageBytes.formatted()) B on average")
            StatTile(label: "Each session", value: stats.sessions == 0 ? "–" : "~\(MemoryStats.tokens(stats.averageInjectedBytes).formatted())",
                     detail: stats.sessions == 0 ? "no sessions yet" : "tokens, \(stats.sessions) session\(stats.sessions == 1 ? "" : "s")")
            StatTile(label: "Read", value: stats.recallRate.map { $0.formatted(.percent.precision(.fractionLength(0))) } ?? "–",
                     detail: "of memories, last \(stats.window) days")
            StatTile(label: "Searches", value: stats.searchHitRate.map { $0.formatted(.percent.precision(.fractionLength(0))) } ?? "–",
                     detail: stats.searches == 0 ? "none yet" : "found something, of \(stats.searches)")
        }
    }
}

private struct StatTile: View {
    let label: String
    let value: String
    let detail: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.callout).foregroundStyle(.secondary)
            Text(value).font(.system(size: 26, weight: .semibold).monospacedDigit())
                .contentTransition(.numericText())
            Text(detail).font(.caption).foregroundStyle(.tertiary).lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .card()
    }
}

/// What the numbers mean, in sentences, with what to do about them.
private struct HowItWorks: View {
    let stats: MemoryStats
    let reconcile: () -> Void

    private var lines: [(symbol: String, text: String, isIssue: Bool)] {
        var lines: [(String, String, Bool)] = []
        let indexTokens = MemoryStats.tokens(stats.averageInjectedBytes)
        lines.append(("text.book.closed", stats.sessions == 0
            ? "Every new thread here starts with an index of these memories: one line each, pinned ones in full."
            : "Every session starts with an index of the memories, about \(indexTokens.formatted()) tokens. \(stats.sessions == 1 ? "1 session" : "\(stats.sessions) sessions") started in the last \(stats.window) days.", false))
        if stats.recallsInWindow > 0 {
            lines.append(("eye", "Agents read \(stats.recalledInWindow) of \(stats.active) memories in full, \(stats.recallsInWindow) times in all, when the index said they were relevant.", false))
        } else if stats.sessions > 0 {
            lines.append(("eye.slash", "No memory was read in full lately. The index line may be all agents needed, or titles and descriptions don't say enough to tell.", true))
        }
        if stats.searches > 0 {
            let missed = stats.searches - stats.searchesWithHits
            lines.append(("magnifyingglass", "Agents searched memory \(stats.searches) times; \(missed == 0 ? "every search found something" : "\(missed) found nothing (listed below: things they expected to know)").", missed > 0))
        }
        if !stats.neverRecalled.isEmpty {
            lines.append(("moon.zzz", "\(stats.neverRecalled.count) memor\(stats.neverRecalled.count == 1 ? "y has" : "ies have") never been read in over a week. Each still costs a line in every session.", true))
        }
        if !stats.likelyDuplicates.isEmpty {
            let titles = stats.likelyDuplicates.prefix(2).map { group in
                group.prefix(3).map { "“\($0.title)”" }.joined(separator: ", ") + (group.count > 3 ? " and \(group.count - 3) more" : "")
            }.joined(separator: "; ")
            lines.append(("square.on.square", "\(stats.likelyDuplicates.count) group\(stats.likelyDuplicates.count == 1 ? " looks" : "s look") like the same memory twice: \(titles).", true))
        }
        if stats.active > 3, stats.orphans * 2 > stats.active {
            lines.append(("link", "\(stats.orphans) memories aren't linked to any other. Links let an agent that reads one find the rest.", false))
        }
        if let largest = stats.largest.first, largest.bytes > 2_000 {
            lines.append(("text.alignleft", "“\(largest.title)” is the biggest at \(largest.bytes.formatted()) bytes; memories work best short and about one thing.", true))
        }
        return lines
    }

    var body: some View {
        let lines = lines
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("How your memory is working").font(.headline)
                Spacer()
                if lines.contains(where: \.isIssue) {
                    Button("Ask the Reconciler", systemImage: "wand.and.sparkles", action: reconcile)
                        .buttonStyle(.glass)
                }
            }
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                Label {
                    Text(line.text)
                } icon: {
                    Image(systemName: line.symbol)
                        .foregroundStyle(line.isIssue ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(16)
        .card()
    }
}

private struct ChartCard<Content: View>: View {
    let title: String
    let detail: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.headline)
            Text(detail).font(.caption).foregroundStyle(.secondary)
            content
                .frame(height: 170)
                .padding(.top, 8)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}

private struct GrowthChart: View {
    let days: [MemoryStats.Day]
    @State private var hovered: Date?

    var body: some View {
        Chart(days) { day in
            AreaMark(x: .value("Day", day.day, unit: .day), y: .value("Memories", day.total))
                .foregroundStyle(Color.claude.opacity(0.16))
                .interpolationMethod(.monotone)
            LineMark(x: .value("Day", day.day, unit: .day), y: .value("Memories", day.total))
                .foregroundStyle(Color.claude)
                .lineStyle(StrokeStyle(lineWidth: 2))
                .interpolationMethod(.monotone)
            if let hovered, Calendar.current.isDate(hovered, inSameDayAs: day.day) {
                RuleMark(x: .value("Day", day.day, unit: .day))
                    .foregroundStyle(.secondary.opacity(0.4))
                    .annotation(position: .top, overflowResolution: .init(x: .fit, y: .disabled)) {
                        Tooltip(title: day.day.formatted(.dateTime.month().day()), lines: ["\(day.total) memories"])
                    }
                PointMark(x: .value("Day", day.day, unit: .day), y: .value("Memories", day.total))
                    .foregroundStyle(Color.claude)
                    .symbolSize(60)
            }
        }
        .chartXSelection(value: $hovered)
        .chartYAxis { AxisMarks(position: .leading) { AxisGridLine().foregroundStyle(.separator.opacity(0.5)); AxisValueLabel() } }
        .chartXAxis { AxisMarks(values: .stride(by: .day, count: 7)) { AxisValueLabel(format: .dateTime.month(.abbreviated).day()) } }
    }
}

private struct ActivityChart: View {
    let days: [MemoryStats.Day]
    @State private var hovered: Date?

    private struct Point: Identifiable {
        let day: Date
        let series: String
        let count: Int
        var id: String { "\(day.timeIntervalSince1970)-\(series)" }
    }

    private static let series = ["Saved", "Read", "Searched"]

    private var points: [Point] {
        days.flatMap { day in
            [Point(day: day.day, series: "Saved", count: day.created + day.updated),
             Point(day: day.day, series: "Read", count: day.recalled),
             Point(day: day.day, series: "Searched", count: day.searches)]
        }
    }

    var body: some View {
        Chart(points) { point in
            BarMark(x: .value("Day", point.day, unit: .day), y: .value("Count", point.count))
                .foregroundStyle(by: .value("What", point.series))
                .cornerRadius(2)
            if let hovered, Calendar.current.isDate(hovered, inSameDayAs: point.day), point.series == "Saved",
               let day = days.first(where: { Calendar.current.isDate($0.day, inSameDayAs: point.day) }) {
                RuleMark(x: .value("Day", point.day, unit: .day))
                    .foregroundStyle(.secondary.opacity(0.25))
                    .annotation(position: .top, overflowResolution: .init(x: .fit, y: .disabled)) {
                        Tooltip(title: day.day.formatted(.dateTime.month().day()),
                                lines: ["\(day.created + day.updated) saved", "\(day.recalled) read",
                                        "\(day.searches) searches", "\(day.sessions) sessions"])
                    }
            }
        }
        .chartForegroundStyleScale(domain: Self.series, range: [Color.purple, Color.claude, Color.teal])
        .chartLegend(position: .bottom, alignment: .leading)
        .chartXSelection(value: $hovered)
        .chartYAxis { AxisMarks(position: .leading) { AxisGridLine().foregroundStyle(.separator.opacity(0.5)); AxisValueLabel() } }
        .chartXAxis { AxisMarks(values: .stride(by: .day, count: 7)) { AxisValueLabel(format: .dateTime.month(.abbreviated).day()) } }
    }
}

private struct KindChart: View {
    let byKind: [String: Int]

    var body: some View {
        Chart(MemoryKind.allCases, id: \.self) { kind in
            BarMark(x: .value("Memories", byKind[kind.rawValue] ?? 0), y: .value("Kind", kind.label))
                .foregroundStyle(kind.color)
                .cornerRadius(4)
                .annotation(position: .trailing) {
                    Text("\(byKind[kind.rawValue] ?? 0)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                }
        }
        .chartXAxis(.hidden)
        .chartYAxis { AxisMarks { AxisValueLabel() } }
    }
}

private struct AgeChart: View {
    let buckets: [MemoryStats.Bucket]

    var body: some View {
        Chart(buckets) { bucket in
            BarMark(x: .value("Age", bucket.label), y: .value("Memories", bucket.count))
                .foregroundStyle(Color.claude.gradient)
                .cornerRadius(4)
                .annotation(position: .top) {
                    Text("\(bucket.count)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                }
        }
        .chartYAxis(.hidden)
    }
}

private struct Tooltip: View {
    let title: String
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption.weight(.semibold))
            ForEach(lines, id: \.self) { Text($0).font(.caption.monospacedDigit()) }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.regularMaterial, in: .rect(cornerRadius: 6))
    }
}

private struct MemoryListCard: View {
    let title: String
    let empty: String
    let items: [MemoryStats.Summary]
    let library: MemoryLibrary
    let trailing: (MemoryStats.Summary) -> String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            if items.isEmpty {
                Text(empty).font(.callout).foregroundStyle(.tertiary)
            } else {
                ForEach(items) { item in
                    Button {
                        library.selection = item.id
                        library.showsArchived = false
                        library.tab = .memories
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: MemoryKind.of(item.kind).symbol).foregroundStyle(MemoryKind.of(item.kind).color).frame(width: 18)
                            Text(item.title).lineLimit(1)
                            Spacer()
                            Text(trailing(item)).font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                        }
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .card()
    }
}

private struct MissedSearches: View {
    let queries: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Searched for, not found").font(.headline)
            if queries.isEmpty {
                Text("Every search found something.").font(.callout).foregroundStyle(.tertiary)
            } else {
                Text("What agents expected memory to know. Worth a memory if it comes up again.")
                    .font(.caption).foregroundStyle(.secondary)
                FlowChips(items: queries)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .card()
    }
}

private struct FlowChips: View {
    let items: [String]

    var body: some View {
        // A few rows of chips; long lists wrap by row of three.
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(stride(from: 0, to: items.count, by: 3)), id: \.self) { start in
                HStack(spacing: 6) {
                    ForEach(items[start..<min(start + 3, items.count)], id: \.self) { Chip($0) }
                }
            }
        }
    }
}

// MARK: - Activity

/// Everything that happened in the memory, newest first, as sentences.
struct MemoryActivityView: View {
    @Environment(AppModel.self) private var app
    let library: MemoryLibrary
    @State private var filter = Filter.all

    enum Filter: String, CaseIterable, Identifiable {
        case all, changes, recall, sessions
        var id: String { rawValue }
        var label: String {
            switch self {
            case .all: "All"
            case .changes: "Changes"
            case .recall: "Reads & Searches"
            case .sessions: "Sessions"
            }
        }

        func includes(_ kind: HarnessDatabase.MemoryEvent.Kind) -> Bool {
            switch self {
            case .all: true
            case .changes: ![.recalled, .searched, .injected].contains(kind)
            case .recall: kind == .recalled || kind == .searched
            case .sessions: kind == .injected
            }
        }
    }

    static func dayLabel(_ day: Date) -> String {
        if Calendar.current.isDateInToday(day) { return "Today" }
        if Calendar.current.isDateInYesterday(day) { return "Yesterday" }
        return day.formatted(.dateTime.weekday(.wide).month().day())
    }

    var body: some View {
        let events = library.events.filter { filter.includes($0.kind) }
        let days = Dictionary(grouping: events) { Calendar.current.startOfDay(for: $0.at) }.sorted { $0.key > $1.key }
        VStack(spacing: 0) {
            Picker("Show", selection: $filter) {
                ForEach(Filter.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(maxWidth: 460)
            .padding(12)
            if events.isEmpty {
                ContentUnavailableView("Nothing Yet", systemImage: "clock",
                                       description: Text("What agents and you do with memory shows here: saves, reads, searches, sessions."))
                    .frame(maxHeight: .infinity)
            } else {
                ScrollView {
                    // Lazy row by row: a busy day is hundreds of events.
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(days, id: \.key) { day, events in
                            Text(Self.dayLabel(day))
                                .font(.subheadline.weight(.semibold))
                                .foregroundStyle(.secondary)
                                .padding(.top, day == days.first?.key ? 0 : 18)
                                .padding(.bottom, 6)
                            ForEach(events) { event in
                                EventRow(event: event, library: library)
                                    .overlay(alignment: .bottom) {
                                        if event.id != events.last?.id { Divider().padding(.leading, 40) }
                                    }
                                    .background(.fill.quaternary, in: UnevenRoundedRectangle(
                                        topLeadingRadius: event.id == events.first?.id ? Theme.cardRadius : 0,
                                        bottomLeadingRadius: event.id == events.last?.id ? Theme.cardRadius : 0,
                                        bottomTrailingRadius: event.id == events.last?.id ? Theme.cardRadius : 0,
                                        topTrailingRadius: event.id == events.first?.id ? Theme.cardRadius : 0, style: .continuous))
                            }
                        }
                    }
                    .frame(maxWidth: 820, alignment: .leading)
                    .padding(.horizontal, 24)
                    .padding(.bottom, 24)
                    .frame(maxWidth: .infinity)
                }
            }
        }
    }
}

private struct EventRow: View {
    @Environment(AppModel.self) private var app
    let event: HarnessDatabase.MemoryEvent
    let library: MemoryLibrary

    private var presentation: (symbol: String, text: String) {
        let who = MemoryActorName.of(event.actor)
        let what = "“\(library.byID[event.memoryID ?? ""]?.title ?? event.title ?? "a memory")”"
        switch event.kind {
        case .created: return ("plus.circle", "\(who) saved \(what)")
        case .updated: return ("pencil.circle", "\(who) updated \(what)")
        case .recalled: return ("eye", "\(who) read \(what)")
        case .searched:
            let count = event.count ?? 0
            return ("magnifyingglass", "\(who) searched for “\(event.query ?? "")”: " + (count == 0 ? "nothing found" : count == 1 ? "1 found" : "\(count) found"))
        case .injected:
            let bytes = event.detail.flatMap(Int.init) ?? 0
            return ("text.book.closed", "A session started with \(event.count ?? 0) memories in its index (~\(MemoryStats.tokens(bytes).formatted()) tokens)")
        case .archived: return ("archivebox", "\(who) archived \(what)" + (event.detail.map { ": \($0)" } ?? ""))
        case .restored: return ("arrow.uturn.backward.circle", "\(who) restored \(what)")
        case .deleted: return ("trash", "\(who) deleted \(what)")
        case .linked: return ("link", "\(who) linked \(what) to “\(library.byID[event.detail ?? ""]?.title ?? "another memory")”")
        case .unlinked: return ("link.badge.minus", "\(who) unlinked \(what)")
        case .merged: return ("arrow.triangle.merge", "\(who) merged memories into \(what)")
        case .pinned: return ("pin", "\(who) pinned \(what)")
        case .unpinned: return ("pin.slash", "\(who) unpinned \(what)")
        case .imported: return ("square.and.arrow.down", "\(who) imported memories: \(event.detail ?? "")")
        case .exported: return ("square.and.arrow.up", "\(who) exported \(event.count ?? 0) memories to \(event.detail ?? "a file")")
        }
    }

    var body: some View {
        let presentation = presentation
        let thread = event.threadID.flatMap { id in app.threads.first { $0.id.uuidString == id } }
        HStack(spacing: 10) {
            Image(systemName: presentation.symbol)
                .foregroundStyle(.secondary)
                .frame(width: 20)
            Text(presentation.text)
                .lineLimit(2)
            Spacer(minLength: 8)
            if let thread {
                Button(thread.title) { app.selectedThreadID = thread.id }
                    .buttonStyle(.link)
                    .lineLimit(1)
                    .frame(maxWidth: 200, alignment: .trailing)
                    .help("Open the thread this happened in")
            }
            Text(event.at, format: .dateTime.hour().minute())
                .font(.callout.monospacedDigit())
                .foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .contentShape(.rect)
        .onTapGesture {
            guard let id = event.memoryID, library.byID[id] != nil else { return }
            library.selection = id
            library.showsArchived = library.byID[id]?.status != .active
            library.tab = .memories
        }
    }
}
