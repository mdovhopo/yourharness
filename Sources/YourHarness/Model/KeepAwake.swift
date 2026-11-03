import Foundation
import IOKit.pwr_mgt

/// What Settings says about keeping the Mac awake.
struct KeepAwakeSettings: Codable, Equatable {
    enum Mode: String, Codable, CaseIterable, Identifiable {
        /// While Claude works in any thread, and for `afterWorkMinutes` after.
        case whileWorking
        /// As long as the app is open.
        case always
        var id: String { rawValue }
        var label: String {
            switch self {
            case .whileWorking: "While Claude works"
            case .always: "Always"
            }
        }
    }

    var enabled = true
    var mode = Mode.whileWorking
    /// Keep the display on too; otherwise only the Mac, and the screen may dim and lock.
    var keepsDisplayOn = false
    /// How long to stay awake once the last turn ends, so a pause between turns doesn't
    /// let the Mac sleep; 0 lets go right away.
    var afterWorkMinutes = 0

    init() {}

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try container.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        mode = try container.decodeIfPresent(Mode.self, forKey: .mode) ?? .whileWorking
        keepsDisplayOn = try container.decodeIfPresent(Bool.self, forKey: .keepsDisplayOn) ?? false
        afterWorkMinutes = try container.decodeIfPresent(Int.self, forKey: .afterWorkMinutes) ?? 0
    }
}

/// Keeps the Mac from sleeping on its own, as `caffeinate` does, with the same power
/// assertion and no process of its own. Nothing ticks: it changes only when work starts or
/// stops, when a setting changes, and at the one deadline it has, if any.
@MainActor
@Observable
final class KeepAwake {
    enum Reason: Equatable {
        case working
        /// The last turn ended; awake until then.
        case afterWork(until: Date)
        case always
        /// Asked for from the status button; nil until you stop it.
        case manual(until: Date?)
    }

    /// Why the Mac is being kept awake, nil when it isn't.
    private(set) var reason: Reason?
    private(set) var settings = KeepAwakeSettings()
    /// "Keep awake for an hour" from the status button, over whatever the settings say.
    private(set) var manualUntil: Date?
    private var manualForever = false

    @ObservationIgnored private var working = false
    @ObservationIgnored private var afterWorkUntil: Date?
    @ObservationIgnored private var assertion: IOPMAssertionID?
    @ObservationIgnored private var assertionKeepsDisplay = false
    @ObservationIgnored private var deadline: Task<Void, Never>?

    func configure(_ settings: KeepAwakeSettings) {
        self.settings = settings
        if settings.afterWorkMinutes == 0 { afterWorkUntil = nil }
        update()
    }

    func setWorking(_ working: Bool) {
        guard working != self.working else { return }
        self.working = working
        afterWorkUntil = !working && settings.afterWorkMinutes > 0
            ? Date.now.addingTimeInterval(TimeInterval(settings.afterWorkMinutes * 60)) : nil
        update()
    }

    /// Keeps it awake for this long, or until `stopManual` with nil.
    func keepAwake(for duration: TimeInterval?) {
        manualForever = duration == nil
        manualUntil = duration.map { Date.now.addingTimeInterval($0) }
        update()
    }

    func stopManual() {
        manualForever = false
        manualUntil = nil
        afterWorkUntil = nil
        update()
    }

    var hasManual: Bool { manualForever || manualUntil != nil }

    private func update() {
        let now = Date.now
        if let until = manualUntil, until <= now { manualUntil = nil }
        if let until = afterWorkUntil, until <= now { afterWorkUntil = nil }

        let reason: Reason? =
            if manualForever { .manual(until: nil) }
            else if let manualUntil { .manual(until: manualUntil) }
            else if !settings.enabled { nil }
            else if settings.mode == .always { .always }
            else if working { .working }
            else if let afterWorkUntil { .afterWork(until: afterWorkUntil) }
            else { nil }
        if reason != self.reason { self.reason = reason }
        hold(reason != nil, display: settings.keepsDisplayOn)

        deadline?.cancel()
        deadline = nil
        if let next = [manualUntil, afterWorkUntil].compactMap({ $0 }).min() {
            deadline = Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(next.timeIntervalSinceNow, 0) + 0.5))
                guard !Task.isCancelled else { return }
                self?.update()
            }
        }
    }

    private func hold(_ on: Bool, display: Bool) {
        if let assertion, !on || display != assertionKeepsDisplay {
            IOPMAssertionRelease(assertion)
            self.assertion = nil
        }
        guard on, assertion == nil else { return }
        var id = IOPMAssertionID(0)
        let type = display ? kIOPMAssertPreventUserIdleDisplaySleep : kIOPMAssertPreventUserIdleSystemSleep
        let result = IOPMAssertionCreateWithName(type as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn),
                                                 "YourHarness: keeping the Mac awake while Claude works" as CFString, &id)
        if result == kIOReturnSuccess {
            assertion = id
            assertionKeepsDisplay = display
        }
    }
}
