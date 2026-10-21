import HarnessCore
import SwiftUI

/// A run of task tool calls in the transcript, shown as the plan they leave behind. The
/// newest is the whole checklist; earlier ones are a line saying where things stood.
struct PlanStepView: View {
    let plan: TaskPlan
    let isLatest: Bool

    var body: some View {
        if isLatest {
            VStack(alignment: .leading, spacing: 0) {
                PlanHeader(plan: plan)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                Divider()
                PlanChecklist(plan: plan)
                    .padding(12)
            }
            .card()
        } else {
            HStack(spacing: 8) {
                Image(systemName: "checklist")
                    .foregroundStyle(.secondary)
                    .frame(width: 18)
                Text("Plan")
                    .fontWeight(.medium)
                Text(summary)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .font(.callout)
            .padding(.horizontal, 12)
        }
    }

    private var summary: String {
        var parts = ["\(plan.completedCount) of \(plan.items.count) done"]
        if let current = plan.current { parts.append(current.activeForm ?? current.subject) }
        return parts.joined(separator: " · ")
    }
}

/// The plan while Claude works, above the composer: where it is, with the list a click away.
struct PlanStrip: View {
    let plan: TaskPlan
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(.snappy(duration: 0.2)) { expanded.toggle() } } label: {
                PlanHeader(plan: plan, showsChevron: true, expanded: expanded)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
            }
            .buttonStyle(.plain)
            if expanded {
                PlanChecklist(plan: plan)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 9)
                    .transition(.opacity)
            }
        }
        .card(radius: Theme.innerRadius + 4)
    }
}

private struct PlanHeader: View {
    let plan: TaskPlan
    var showsChevron = false
    var expanded = false

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if plan.isFinished {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                } else {
                    ProgressRing(fraction: plan.items.isEmpty ? 0 : Double(plan.completedCount) / Double(plan.items.count))
                }
            }
            .frame(width: 16, height: 16)
            if let current = plan.current {
                Text(current.activeForm ?? current.subject)
                    .fontWeight(.medium)
                    .lineLimit(1)
            } else {
                Text(plan.isFinished ? "Plan done" : "Plan")
                    .fontWeight(.medium)
            }
            Spacer(minLength: 8)
            Text("\(plan.completedCount) of \(plan.items.count)")
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
            if showsChevron {
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
            }
        }
        .contentShape(.rect)
    }
}

private struct PlanChecklist: View {
    let plan: TaskPlan

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(plan.items) { item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: Self.symbol(item.status))
                        .foregroundStyle(Self.tint(item.status))
                        .frame(width: 16)
                    Text(item.status == .inProgress ? item.activeForm ?? item.subject : item.subject)
                        .fontWeight(item.status == .inProgress ? .medium : .regular)
                        .foregroundStyle(item.status == .completed ? .secondary : .primary)
                        .strikethrough(item.status == .completed, color: .secondary.opacity(0.6))
                }
            }
        }
        .font(.callout)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private static func symbol(_ status: TaskPlan.Status) -> String {
        switch status {
        case .pending: "circle"
        case .inProgress: "circle.dotted.circle"
        case .completed: "checkmark.circle.fill"
        }
    }

    private static func tint(_ status: TaskPlan.Status) -> AnyShapeStyle {
        switch status {
        case .pending: AnyShapeStyle(.tertiary)
        case .inProgress: AnyShapeStyle(.tint)
        case .completed: AnyShapeStyle(.green)
        }
    }
}

private struct ProgressRing: View {
    let fraction: Double

    var body: some View {
        ZStack {
            Circle().stroke(.quaternary, lineWidth: 2.5)
            Circle()
                .trim(from: 0, to: fraction)
                .stroke(.tint, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .animation(.smooth, value: fraction)
    }
}
