import Foundation
import HarnessBackend

/// Patches since the last frame, for the page. Streamed text for the same row arrives as many
/// small appends; consecutive ones are joined, so a frame carries one per row.
@MainActor
final class Outbox {
    private var items: [Wire] = []
    /// Called when the first patch since the last flush arrives.
    var onPending: (() -> Void)?

    func push(_ patch: Wire) {
        let wasEmpty = items.isEmpty
        if patch["t"] as? String == "app", var last = items.last, last["t"] as? String == "app",
           last["th"] as? String == patch["th"] as? String, last["id"] as? String == patch["id"] as? String,
           last["f"] as? String == patch["f"] as? String {
            last["s"] = (last["s"] as? String ?? "") + (patch["s"] as? String ?? "")
            last["rv"] = patch["rv"]
            items[items.count - 1] = last
        } else {
            items.append(patch)
        }
        if wasEmpty { onPending?() }
    }

    func drain() -> Data? {
        guard !items.isEmpty else { return nil }
        defer { items.removeAll(keepingCapacity: true) }
        return try? JSONSerialization.data(withJSONObject: items)
    }
}

struct HostError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}
