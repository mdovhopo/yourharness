import SwiftUI

/// ⌘K: a search panel over the window, as Spotlight looks. Type, ↑↓ to pick, ↩ to go.
struct SpotlightView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @Bindable var spotlight: Spotlight
    @FocusState private var focused: Bool

    private var environment: SpotlightEnvironment {
        SpotlightEnvironment(openWindow: { openWindow(id: "memory-agent", value: $0) }, openSettings: { openSettings() })
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass")
                    .font(.title2)
                    .foregroundStyle(.secondary)
                TextField("Search threads, memories, tasks…", text: $spotlight.query)
                    .textFieldStyle(.plain)
                    .font(.title2)
                    .focused($focused)
                    .onSubmit { go(spotlight.results.indices.contains(spotlight.selection) ? spotlight.results[spotlight.selection] : nil) }
                    .onKeyPress(.upArrow) { spotlight.move(-1); return .handled }
                    .onKeyPress(.downArrow) { spotlight.move(1); return .handled }
                    .onKeyPress(.escape) { spotlight.close(); return .handled }
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 14)
            if !spotlight.results.isEmpty {
                Divider()
                results
            }
        }
        .frame(width: 660)
        // A solid backing under the glass: results stay readable over any page.
        .background(Color(nsColor: .windowBackgroundColor).opacity(0.92), in: .rect(cornerRadius: 22))
        .glassEffect(.regular, in: .rect(cornerRadius: 22))
        .shadow(color: .black.opacity(0.18), radius: 30, y: 12)
        .onAppear { focused = true }
        .onChange(of: spotlight.query) { spotlight.refresh(app) }
    }

    private var results: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(spotlight.results.enumerated()), id: \.element.id) { index, result in
                        if index == 0 || spotlight.results[index - 1].section != result.section {
                            Text(result.section.title)
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.secondary)
                                .padding(.horizontal, 12)
                                .padding(.top, index == 0 ? 4 : 10)
                                .padding(.bottom, 2)
                        }
                        SpotlightRow(result: result, isSelected: index == spotlight.selection)
                            .id(result.id)
                            .onTapGesture { go(result) }
                            .onHover { if $0 { spotlight.selection = index } }
                    }
                }
                .padding(8)
            }
            .frame(maxHeight: 440)
            .fixedSize(horizontal: false, vertical: true)
            .onChange(of: spotlight.selection) { _, selection in
                guard spotlight.results.indices.contains(selection) else { return }
                proxy.scrollTo(spotlight.results[selection].id)
            }
        }
    }

    private func go(_ result: Spotlight.Result?) {
        spotlight.perform(result, app: app, environment: environment)
    }
}

private struct SpotlightRow: View {
    let result: Spotlight.Result
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: result.symbol)
                .font(.body)
                .foregroundStyle(isSelected ? AnyShapeStyle(.white) : result.tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.secondary))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text(result.title).lineLimit(1)
                    Text(result.subtitle)
                        .font(.callout)
                        .foregroundStyle(isSelected ? AnyShapeStyle(.white.opacity(0.8)) : AnyShapeStyle(.secondary))
                        .lineLimit(1)
                }
                if let snippet = result.snippet {
                    Text(snippet)
                        .font(.callout)
                        .foregroundStyle(isSelected ? AnyShapeStyle(.white.opacity(0.85)) : AnyShapeStyle(.secondary))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if let shortcut = result.shortcut {
                Text(shortcut)
                    .font(.callout.monospaced())
                    .foregroundStyle(isSelected ? AnyShapeStyle(.white.opacity(0.8)) : AnyShapeStyle(.tertiary))
            }
        }
        .foregroundStyle(isSelected ? .white : .primary)
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color.accentColor)
            }
        }
        .contentShape(.rect)
    }
}
