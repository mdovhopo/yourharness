import HarnessCore
import SwiftUI

struct UsageBar: View {
    let window: RateLimitInfo.Window
    var now = Date.now

    /// A saved reading from before the window reset says nothing about now.
    private var hasReset: Bool { window.resetsAt.map { $0 <= now } ?? false }
    private var utilization: Double { hasReset ? 0 : window.utilization }

    private var title: String {
        switch window.name {
        case "five_hour": "5-hour limit"
        case "seven_day": "Weekly limit"
        case "seven_day_opus": "Weekly Opus"
        default: window.name.replacing("_", with: " ").capitalized
        }
    }

    private var tint: Color {
        switch utilization {
        case ..<0.7: .accentColor
        case ..<0.9: .orange
        default: .red
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(title)
                Spacer()
                Text(utilization, format: .percent.precision(.fractionLength(0)))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .font(.caption)
            ProgressView(value: min(max(utilization, 0), 1))
                .tint(tint)
            if hasReset {
                Text("Reset · refreshes with your next message")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            } else if let resetsAt = window.resetsAt {
                Text("Resets \(resetsAt, format: .relative(presentation: .named))")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }
}
