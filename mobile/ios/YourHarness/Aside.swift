// A side chat (`/btw`): a question asked beside a thread, as the Mac's right sidebar has it.
// Claude sees the thread; nothing is added to it. Its rows are drawn by the same transcript as
// a thread's; nothing of it is saved on the Mac, so it lasts while the Mac keeps it.

import SwiftUI

struct AsideSheet: View {
    let threadID: String
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var model: TranscriptModel?
    @State private var question = ""
    @FocusState private var focused: Bool

    private var running: Bool { store.asideLive?.running ?? false }

    var body: some View {
        NavigationStack {
            Group {
                if let model {
                    TranscriptRepresentable(model: model, store: store, bottomInset: 0)
                }
            }
            .overlay {
                if store.asideLive != nil, model?.count == 0, !running {
                    ContentUnavailableView("Ask on the Side", systemImage: "bubble.left.and.text.bubble.right", description: Text("Claude sees this thread and answers here. Nothing is added to the thread, and its turn goes on."))
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) { field }
            .navigationTitle("Side Chat")
            .navigationSubtitle(store.threads[threadID]?.title ?? "")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done", role: .confirm) { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .onAppear { model = store.openAside(of: threadID) }
        .onDisappear { if let model { store.close(aside: model) } }
    }

    private var field: some View {
        VStack(spacing: 6) {
            if running {
                StatusLine(activity: store.asideLive?.activity, tokens: nil, window: nil)
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Ask on the side", text: $question, axis: .vertical)
                    .lineLimit(1...4)
                    .focused($focused)
                    .padding(.vertical, 9)
                    .padding(.horizontal, 14)
                    .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 20))
                Button {
                    store.askAside(question, in: threadID)
                    question = ""
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 38, height: 38)
                }
                .buttonStyle(.glassProminent)
                .buttonBorderShape(.circle)
                .disabled(question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !store.canAct)
                .accessibilityLabel("Ask")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
