// The Macs the phone knows. Every one stays connected, so switching is instant and the list
// shows which need you; a Mac not shown is sent only its thread list and statuses, no rows.

import Foundation
import Observation
#if canImport(UIKit)
import UIKit
#endif

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
        if let link = URL(string: address), link.scheme == "yourharness" { return pair(link) }
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

    /// A pairing link from the Mac's QR code (`yourharness://pair?host=…&port=…&code=…&name=…`):
    /// connects with the code, and once the Mac answers with this phone's token, keeps the Mac
    /// with that. Pairing again with a Mac already listed replaces it.
    @discardableResult
    func pair(_ link: URL) -> Bool {
        guard link.scheme == "yourharness", link.host() == "pair",
              let items = URLComponents(url: link, resolvingAgainstBaseURL: false)?.queryItems else { return false }
        func item(_ name: String) -> String? { items.first { $0.name == name }?.value }
        guard let host = item("host"), let port = item("port"), let code = item("code") else { return false }
        let base = "ws://\(host):\(port)/"
        let device = Self.deviceName.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "Phone"
        // The guess a new install starts with goes now; the Mac it paired with before, once this
        // pairing works (a used code leaves the old pairing as it was), and any earlier try.
        let guess = { (s: Store) in s.serverAddress == Store.defaultAddress && !s.everConnected }
        let earlier = { (s: Store) in s.pairing && s.serverAddress.hasPrefix(base) }
        stores.filter { guess($0) || earlier($0) }.forEach { $0.disconnect() }
        stores.removeAll { guess($0) || earlier($0) }
        let store = Store(address: "\(base)?pair=\(code)&device=\(device)", name: item("name"))
        wire(store)
        store.onPaired = { [weak self, weak store] token in
            guard let self, let store else { return }
            store.paired(address: "\(base)?token=\(token)")
            for old in stores where old !== store && old.serverAddress.hasPrefix(base) { old.disconnect() }
            stores.removeAll { $0 !== store && $0.serverAddress.hasPrefix(base) }
            save()
        }
        stores.append(store)
        tab = .threads
        select(store)
        Perf.shared.log("PAIRING with \(item("name") ?? host)")
        return true
    }

    /// What the Mac lists this phone as.
    static var deviceName: String {
        #if canImport(UIKit)
        let name = UIDevice.current.name
        let model = UIDevice.current.model
        return name == model ? "\(model) (\(UIDevice.current.systemName) \(UIDevice.current.systemVersion))" : name
        #else
        return "Phone"
        #endif
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
        // A Mac still pairing isn't kept: its code works only once.
        let macs = stores.filter { !$0.pairing }.map { Saved(address: $0.serverAddress, name: $0.serverName) }
        UserDefaults.standard.set(try? JSONEncoder().encode(macs), forKey: "macs")
        UserDefaults.standard.set(stores.firstIndex { $0 === current } ?? 0, forKey: "currentMac")
        Perf.shared.log("SAVED \(macs.count) Macs, showing \(stores.firstIndex { $0 === current } ?? 0)")
    }
}
