// Shared visual tokens and compact components mirroring the Mac app's logo-inspired palette:
// violet primary accent (#6D4AFF light / #9575FA dark) for Claude and active controls,
// logo teal (#0D9488 light / #5EEAD4 dark) for Antigravity, compact 5h/7d limit grids,
// thread provider indicators (provider only), and sub-agent model pills.

import SwiftUI
import UIKit

enum Brand {
    /// Primary logo violet: #6D4AFF (light) / #9575FA (dark).
    static let accent = Color("AccentColor")

    /// Dynamic UIKit violet for UIKit-rendered transcript links, diagrams, and badges.
    nonisolated static let accentUI = UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0.584, green: 0.459, blue: 0.980, alpha: 1) // #9575FA
            : UIColor(red: 0.427, green: 0.290, blue: 1.000, alpha: 1) // #6D4AFF
    }

    /// Subtle violet tint fill for compact UIKit pills (sub-agent models).
    nonisolated static let accentSoftUI = UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0.584, green: 0.459, blue: 0.980, alpha: 0.18)
            : UIColor(red: 0.427, green: 0.290, blue: 1.000, alpha: 0.11)
    }

    /// Logo teal strand: #0D9488 (light) / #5EEAD4 (dark), used for Antigravity.
    nonisolated static let tealUI = UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0.369, green: 0.918, blue: 0.831, alpha: 1) // #5EEAD4
            : UIColor(red: 0.051, green: 0.580, blue: 0.533, alpha: 1) // #0D9488
    }

    static let teal = Color(uiColor: tealUI)

    /// Provider brand color: violet for Claude (nil), teal for Antigravity / other agents.
    static func providerColor(_ agent: String?) -> Color {
        agent == nil ? accent : teal
    }

    /// Display name for a thread's provider ("Claude" or "Antigravity").
    @MainActor static func providerLabel(_ agent: String?, store: Store? = nil) -> String {
        guard let agent, !agent.isEmpty else { return "Claude" }
        if let name = store?.agents.first(where: { $0.id == agent })?.name { return name }
        return agent.capitalized
    }

    /// Single-letter monogram for compact sidebar/list badges ("C" / "A").
    nonisolated static func providerMonogram(_ agent: String?) -> String {
        guard let agent, let first = agent.first else { return "C" }
        return String(first).uppercased()
    }

    /// Compact formatted label for a sub-agent's model (e.g. "claude-sonnet-4-6" -> "Sonnet 4.6",
    /// "haiku" -> "Haiku"). Returns nil for empty, default, or synthetic placeholders.
    nonisolated static func subagentModelLabel(_ raw: String?) -> String? {
        guard let raw else { return nil }
        let s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty, s != "<synthetic>" else { return nil }
        let lower = s.lowercased()
        switch lower {
        case "default", "auto": return nil
        case "haiku": return "Haiku"
        case "sonnet": return "Sonnet"
        case "opus": return "Opus"
        default: break
        }
        var trimmed = lower
        if trimmed.hasPrefix("claude-") { trimmed.removeFirst(7) }
        // Strip trailing 8-digit release date (-20251001).
        if let lastDash = trimmed.lastIndex(of: "-") {
            let suffix = trimmed[trimmed.index(after: lastDash)...]
            if suffix.count == 8, suffix.allSatisfy(\.isNumber) {
                trimmed = String(trimmed[..<lastDash])
            }
        }
        let parts = trimmed.split(separator: "-").map(String.init)
        if let first = parts.first, ["haiku", "sonnet", "opus"].contains(first) {
            let family = first.capitalized
            let nums = parts.dropFirst().filter { $0.allSatisfy(\.isNumber) }
            if !nums.isEmpty {
                return "\(family) \(nums.joined(separator: "."))"
            }
            return family
        }
        // Fallback: prettify hyphenated IDs (e.g. "gemini-3-pro" -> "Gemini 3 Pro").
        return parts.map { part in
            switch part {
            case "gpt": "GPT"
            case "pro": "Pro"
            case "flash": "Flash"
            default: part.capitalized
            }
        }.joined(separator: " ")
    }

    /// Short window tag matching the Mac's compact limits bar ("5h", "7d", "7d Opus", ...).
    nonisolated static func shortWindowLabel(_ raw: String) -> String {
        switch raw {
        case "five_hour": "5h"
        case "seven_day": "7d"
        case "seven_day_opus": "7d Opus"
        case "seven_day_sonnet": "7d Sonnet"
        default: raw.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    /// Compact countdown matching the Mac's limits footer ("3h 16m", "3d 18h", "42m", "reset").
    nonisolated static func shortCountdown(to resetsAt: Double, now: Date = Date()) -> String {
        let secs = Int(resetsAt - now.timeIntervalSince1970)
        if secs <= 0 { return "reset" }
        let mins = secs / 60
        let hours = mins / 60
        let days = hours / 24
        if days > 0 {
            let remH = hours % 24
            return remH > 0 ? "\(days)d \(remH)h" : "\(days)d"
        }
        if hours > 0 {
            let remM = mins % 60
            return remM > 0 ? "\(hours)h \(remM)m" : "\(hours)h"
        }
        if mins > 0 { return "\(mins)m" }
        return "<1m"
    }
}

/// Compact provider pill for thread headers: `[● Claude]` (violet) or `[● Antigravity]` (teal).
struct ProviderPill: View {
    let agent: String?
    var label: String? = nil

    private var color: Color { Brand.providerColor(agent) }
    private var name: String { label ?? Brand.providerLabel(agent) }

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
            Text(name)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(color)
                .lineLimit(1)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 2.5)
        .background(color.opacity(0.14), in: .capsule)
        .overlay(Capsule().strokeBorder(color.opacity(0.28), lineWidth: 0.5))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Provider: \(name)")
    }
}

/// Compact 16×16 monogram badge for thread rows & search results ("C" violet / "A" teal).
struct ProviderBadge: View {
    let agent: String?

    private var color: Color { Brand.providerColor(agent) }

    var body: some View {
        Text(Brand.providerMonogram(agent))
            .font(.system(size: 10, weight: .bold, design: .rounded))
            .foregroundStyle(color)
            .frame(width: 18, height: 18)
            .background(color.opacity(0.15), in: .rect(cornerRadius: 5, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 5, style: .continuous)
                    .strokeBorder(color.opacity(0.28), lineWidth: 0.5)
            )
            .accessibilityHidden(true)
    }
}

/// Compact violet pill for sub-agent models ("Sonnet 4.6", "Haiku 4.5").
struct ModelPill: View {
    let label: String

    var body: some View {
        Text(label)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(Brand.accent)
            .padding(.horizontal, 7)
            .padding(.vertical, 2.5)
            .background(Brand.accent.opacity(0.14), in: .capsule)
            .overlay(Capsule().strokeBorder(Brand.accent.opacity(0.28), lineWidth: 0.5))
            .lineLimit(1)
    }
}

/// Compact 2-column grid of usage limit windows showing both percentages, bars, and reset
/// countdowns side by side (e.g. `5h 42%  3h 16m` | `7d 68%  3d 18h`), matching the Mac footer.
struct CompactLimitsGrid: View {
    let windows: [LimitWindow]
    var dense: Bool = false
    var accentColor: Color = Brand.accent

    private let columns = [
        GridItem(.flexible(), spacing: 12),
        GridItem(.flexible(), spacing: 12),
    ]

    var body: some View {
        LazyVGrid(columns: windows.count == 1 ? [GridItem(.flexible())] : columns, alignment: .leading, spacing: dense ? 8 : 10) {
            ForEach(windows) { window in
                CompactLimitCell(window: window, dense: dense, accentColor: accentColor)
            }
        }
        .padding(.vertical, dense ? 2 : 4)
    }
}

private struct CompactLimitCell: View {
    let window: LimitWindow
    let dense: Bool
    let accentColor: Color

    private var stale: Bool { window.resetsAt.map { $0 < Date().timeIntervalSince1970 } ?? false }
    private var used: Double { min(max(window.used, 0), 1) }
    private var tint: Color { stale ? .gray : used >= 0.9 ? .red : used >= 0.7 ? .orange : accentColor }

    var body: some View {
        VStack(alignment: .leading, spacing: dense ? 4 : 5) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(Brand.shortWindowLabel(window.name))
                    .font(dense ? .caption2.weight(.semibold) : .caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(window.used, format: .percent.precision(.fractionLength(0)))
                    .font(dense ? .caption2.weight(.bold) : .caption.weight(.bold))
                    .monospacedDigit()
                    .foregroundStyle(!stale && used >= 0.9 ? .red : .primary)
                Spacer(minLength: 2)
                if let at = window.resetsAt {
                    Text(Brand.shortCountdown(to: at))
                        .font(dense ? .caption2 : .caption2)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            GeometryReader { box in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color(.tertiarySystemFill))
                    Capsule()
                        .fill(tint)
                        .frame(width: max(box.size.width * used, used > 0 ? 5 : 0))
                }
            }
            .frame(height: dense ? 4 : 5)
        }
        .opacity(stale ? 0.65 : 1)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilityText)
    }

    private var accessibilityText: String {
        let pct = Int((used * 100).rounded())
        let reset = window.resetsAt.map { stale ? ", reset elapsed" : ", resets in \(Brand.shortCountdown(to: $0))" } ?? ""
        return "\(LimitRow.name(window.name)), \(pct) percent used\(reset)"
    }
}
