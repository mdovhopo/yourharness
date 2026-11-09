// The Macs the phone knows. Every one stays connected, so switching is instant and the list
// shows which need you; a Mac not shown is sent only its thread list and statuses, no rows.

import Foundation
import Observation

@Observable @MainActor
final class Fleet {
    struct Saved: Codable, Hashable {
        var address: String
        var name: String?
    }

    private(set) var stores: [Store]
    private(set) var current: Store
    /// The dock's tab (`YH_PHONE_TAB=usage|mac|search` at launch).
    var tab: AppTab
    /// `YH_SERVER` (addresses, commas between) is for tests: nothing is saved then.
    @ObservationIgnored private let saves: Bool

    init() {
        let env = ProcessInfo.processInfo.environment
        let saved: [Saved]
        if let list = env["YH_SERVER"] {
            saved = list.split(separator: ",").map { Saved(address: String($0)) }
            saves = false
        } else {
            let defaults = UserDefaults.standard
            if let data = defaults.data(forKey: "macs"), let macs = try? JSONDecoder().decode([Saved].self, from: data), !macs.isEmpty {
                saved = macs
            } else {
                // Before several Macs: the one address.
                saved = [Saved(address: defaults.string(forKey: "server") ?? Store.defaultAddress)]
            }
            saves = true
        }
        let first = env["YH_PHONE_MAC"].flatMap(Int.init) ?? UserDefaults.standard.integer(forKey: "currentMac")
        let start = saved.indices.contains(first) ? first : 0
        let stores = saved.enumerated().map { i, mac in Store(address: mac.address, name: mac.name, selects: i == start) }
        self.stores = stores
        current = stores[start]
        tab = env["YH_PHONE_TAB"].flatMap(AppTab.init(rawValue:)) ?? .threads
        stores.forEach(wire)
        current.shown = true
        current.becomeCurrent()
    }

    /// Shows another Mac; its open thread, if it had one, comes back with it.
    func select(_ store: Store) {
        guard store !== current, stores.contains(where: { $0 === store }) else { return }
        current.shown = false
        current = store
        store.shown = true
        store.becomeCurrent()
        Perf.shared.log("MAC \(store.displayName)")
        save()
    }

    /// A Mac added by its address (`ws://…?token=…`), and shown; one already known is just shown.
    @discardableResult
    func add(_ address: String) -> Bool {
        let address = address.trimmingCharacters(in: .whitespaces)
        guard let url = URL(string: address), url.scheme == "ws" || url.scheme == "wss" else { return false }
        if let known = stores.first(where: { $0.serverAddress == address }) {
            select(known)
            return true
        }
        let store = Store(address: address)
        wire(store)
        stores.append(store)
        select(store)
        save()
        return true
    }

    /// Forgets a Mac; the last one stays.
    func remove(_ store: Store) {
        guard stores.count > 1, let i = stores.firstIndex(where: { $0 === store }) else { return }
        store.disconnect()
        stores.remove(at: i)
        if current === store { select(stores[min(i, stores.count - 1)]) }
        save()
    }

    /// Threads waiting on you on Macs not shown.
    var waitingElsewhere: Int {
        stores.filter { $0 !== current }.reduce(0) { $0 + $1.waitingCount }
    }

    private func wire(_ store: Store) {
        store.onShow = { [weak self] in self?.tab = .threads }
        store.onHello = { [weak self] in self?.save() }
    }

    private func save() {
        guard saves else { return }
        let macs = stores.map { Saved(address: $0.serverAddress, name: $0.serverName) }
        UserDefaults.standard.set(try? JSONEncoder().encode(macs), forKey: "macs")
        UserDefaults.standard.set(stores.firstIndex { $0 === current } ?? 0, forKey: "currentMac")
    }
}
