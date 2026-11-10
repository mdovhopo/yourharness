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
        /// The Mac's lasting ID: one entry per Mac, whatever its address.
        var id: String?
        /// Every address it may be reached at (Wi‑Fi, Tailscale).
        var hosts: [String]?
    }

    /// This phone's lasting ID, sent when it pairs: pairing again replaces it on the Mac.
    static var phoneID: String {
        if let id = UserDefaults.standard.string(forKey: "phoneID") { return id }
        let id = UUID().uuidString.lowercased()
        UserDefaults.standard.set(id, forKey: "phoneID")
        return id
    }

    private(set) var stores: [Store]
    private(set) var current: Store
    /// The dock's tab (`YH_PHONE_TAB=usage|mac|search` at launch).
    var tab: AppTab
    /// Threads lists every Mac's projects together, not the shown Mac's alone (opt in, saved;
    /// `YH_PHONE_ALL_MACS=0|1` at launch, not saved).
    var showsAllMacs: Bool {
        didSet {
            guard saves, ProcessInfo.processInfo.environment["YH_PHONE_ALL_MACS"] == nil else { return }
            UserDefaults.standard.set(showsAllMacs, forKey: "threads.allMacs")
        }
    }
    /// The list mixes Macs: asked for, and there's more than one.
    var mixesMacs: Bool { showsAllMacs && stores.count > 1 }
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
        let stores = saved.enumerated().map { i, mac in Store(address: mac.address, name: mac.name, macID: mac.id, hosts: mac.hosts ?? [], selects: i == start) }
        self.stores = stores
        current = stores[start]
        tab = env["YH_PHONE_TAB"].flatMap(AppTab.init(rawValue:)) ?? .threads
        showsAllMacs = env["YH_PHONE_ALL_MACS"].map { $0 == "1" } ?? UserDefaults.standard.bool(forKey: "threads.allMacs")
        stores.forEach(wire)
        current.shown = true
        current.becomeCurrent()
    }

    /// The app in the background or back: no Mac streams a thread nobody sees.
    func setAway(_ away: Bool) {
        stores.forEach { $0.setAway(away) }
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

    /// A pairing link from the Mac's QR code (`yourharness://pair?host=…&hosts=…&port=…&code=…&name=…`,
    /// `hosts` every address it may be reached at, tried at once):
    /// connects with the code, and once the Mac answers with this phone's token, keeps the Mac
    /// with that. Pairing again with a Mac already listed replaces it.
    @discardableResult
    func pair(_ link: URL) -> Bool {
        guard link.scheme == "yourharness", link.host() == "pair",
              let items = URLComponents(url: link, resolvingAgainstBaseURL: false)?.queryItems else { return false }
        func item(_ name: String) -> String? { items.first { $0.name == name }?.value }
        guard let host = item("host"), let port = item("port"), let code = item("code") else { return false }
        let base = "ws://\(host):\(port)/"
        let hosts = item("hosts").map { $0.split(separator: ",").map(String.init) } ?? [host]
        let macID = item("id")
        let device = Self.deviceName.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "Phone"
        // The guess a new install starts with goes now; the Mac it paired with before, once this
        // pairing works (a used code leaves the old pairing as it was), and any earlier try.
        let guess = { (s: Store) in s.serverAddress == Store.defaultAddress && !s.everConnected }
        let earlier = { (s: Store) in s.pairing && (s.serverAddress.hasPrefix(base) || (macID != nil && s.macID == macID)) }
        stores.filter { guess($0) || earlier($0) }.forEach { $0.disconnect() }
        stores.removeAll { guess($0) || earlier($0) }
        let store = Store(address: "\(base)?pair=\(code)&device=\(device)&device_id=\(Self.phoneID)", name: item("name"), macID: macID, hosts: hosts)
        wire(store)
        store.onPaired = { [weak self, weak store] token in
            guard let self, let store else { return }
            store.paired(address: "\(base)?token=\(token)")
            // The same Mac paired before (by its ID, else its address) gives way to this.
            let same = { (s: Store) in s !== store && (s.serverAddress.hasPrefix(base) || (store.macID != nil && s.macID == store.macID)) }
            stores.filter(same).forEach { $0.disconnect() }
            stores.removeAll(where: same)
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

    /// A thread opened from the list of every Mac: its Mac is shown, the thread over the list.
    func open(_ thread: String, on store: Store) {
        select(store)
        store.show(thread)
    }

    /// A thread in the list of every Mac, with the Mac it's on.
    struct MixedThread: Identifiable {
        let mac: Store
        let thread: ThreadInfo
        var id: String { "\(mac.id)/\(thread.id)" }
    }

    /// A project as the list of every Mac has it: projects of the same name on several Macs are one.
    struct MixedSection: Identifiable {
        let name: String
        /// Each Mac's project of that name, in the order the Macs are listed.
        let projects: [(mac: Store, project: Project)]
        let live: [MixedThread]
        let settled: [MixedThread]
        var id: String { name }
    }

    /// Every Mac's sections, mixed: in the order the Macs list their projects, each thread with
    /// its Mac; pinned first, then newest made first; settled ones on the shelf, last changed first.
    var mixedSections: [MixedSection] {
        var names: [String] = []
        var groups: [String: [(mac: Store, section: Store.Section)]] = [:]
        for mac in stores {
            for section in mac.sections {
                let name = section.project.name
                if groups[name] == nil { names.append(name) }
                groups[name, default: []].append((mac, section))
            }
        }
        return names.map { name in
            let group = groups[name] ?? []
            let live = group.flatMap { g in g.section.live.map { MixedThread(mac: g.mac, thread: $0) } }
                .sorted { ($0.thread.pinned ? 1 : 0, $0.thread.createdAt) > ($1.thread.pinned ? 1 : 0, $1.thread.createdAt) }
            let settled = group.flatMap { g in g.section.settled.map { MixedThread(mac: g.mac, thread: $0) } }
                .sorted { $0.thread.updatedAt > $1.thread.updatedAt }
            return MixedSection(name: name, projects: group.map { ($0.mac, $0.section.project) }, live: live, settled: settled)
        }
    }

    /// Threads waiting on you on Macs not shown.
    var waitingElsewhere: Int {
        stores.filter { $0 !== current }.reduce(0) { $0 + $1.waitingCount }
    }

    private func wire(_ store: Store) {
        store.onShow = { [weak self] in self?.tab = .threads }
        store.onHello = { [weak self, weak store] in
            guard let self, let store else { return }
            merge(store)
            save()
        }
    }

    /// Two entries that turn out to be the same Mac (its ID, told on connecting) become one:
    /// the one shown, else the one that just connected.
    private func merge(_ store: Store) {
        guard let id = store.macID, !store.pairing else { return }
        let same = stores.filter { $0.macID == id && !$0.pairing }
        guard same.count > 1 else { return }
        let keep = same.contains { $0 === current } ? current : store
        for s in same where s !== keep { s.disconnect() }
        stores.removeAll { $0 !== keep && $0.macID == id && !$0.pairing }
        Perf.shared.log("MERGED \(same.count) entries for \(keep.displayName)")
    }

    private func save() {
        guard saves else { return }
        // A Mac still pairing isn't kept: its code works only once.
        let macs = stores.filter { !$0.pairing }.map { Saved(address: $0.serverAddress, name: $0.serverName, id: $0.macID, hosts: $0.hosts.isEmpty ? nil : $0.hosts) }
        UserDefaults.standard.set(try? JSONEncoder().encode(macs), forKey: "macs")
        UserDefaults.standard.set(stores.firstIndex { $0 === current } ?? 0, forKey: "currentMac")
        Perf.shared.log("SAVED \(macs.count) Macs, showing \(stores.firstIndex { $0 === current } ?? 0)")
    }
}
