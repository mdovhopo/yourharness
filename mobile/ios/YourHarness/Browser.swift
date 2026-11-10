// A thread's browser on the phone (protocol 7): the Mac's own browser, the one Claude drives,
// streamed as pictures of its shown tab. A tap clicks there, one finger scrolls the page, two
// pinch and pan the picture; the keyboard types into the page's field; the bar at the bottom
// goes back, forward, reloads and takes an address. Pages on `localhost` are the Mac's.

import SwiftUI
import UIKit

struct BrowserScreen: View {
    let threadID: String
    /// In a side panel beside the thread (iPad): a header of its own with a close button, in
    /// place of the navigation bar's title and buttons.
    var onClose: (() -> Void)? = nil
    @Environment(Store.self) private var store
    @State private var address = ""
    @FocusState private var addressFocused: Bool
    /// The page's keyboard is up (typing goes to the page's focused field).
    @State private var typing = false
    @State private var dialogText = ""
    /// When this screen last tapped the page: a field focused by it brings the keyboard up.
    @State private var tappedAt = Date.distantPast
    /// The page's place on screen, in points: Phone lays the page out at exactly this size.
    @State private var pageArea = CGSize.zero
    private static let env = ProcessInfo.processInfo.environment
    private static var ranHooks = false

    private var state: BrowserState? { store.browserState }
    private var shown: BrowserTab? { state?.shown }

    var body: some View {
        if let onClose {
            page.safeAreaInset(edge: .top, spacing: 0) { header(onClose) }
        } else {
            page
                .navigationTitle(shown.map { $0.title.isEmpty ? Self.host($0.url) : $0.title } ?? "Browser")
                .navigationSubtitle(shown.map { Self.host($0.url) } ?? "")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) { keyboardButton }
                    ToolbarItem(placement: .topBarTrailing) { menu }
                }
        }
    }

    private var keyboardButton: some View {
        Button(typing ? "Hide Keyboard" : "Type in the Page", systemImage: typing ? "keyboard.chevron.compact.down" : "keyboard") { typing.toggle() }
            .disabled(shown == nil)
    }

    /// The side panel's top: the page's title and address, the keyboard, More, and Close.
    private func header(_ close: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 1) {
                Text(shown.map { $0.title.isEmpty ? Self.host($0.url) : $0.title } ?? "Browser").font(.headline).lineLimit(1)
                if let shown { Text(Self.host(shown.url)).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
            }
            Spacer(minLength: 4)
            keyboardButton
            menu
            Button("Close", systemImage: "xmark", action: close)
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.glass)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private var page: some View {
        ZStack {
            Color(.secondarySystemBackground).ignoresSafeArea()
            if let state, state.tabs.isEmpty {
                ContentUnavailableView {
                    Label("No Pages Open", systemImage: "globe")
                } description: {
                    Text("Claude opens pages here when it tests your app. Type an address below to open one on your Mac.")
                }
            } else if let image = store.browserFrame, let state {
                PageView(image: image, pageSize: CGSize(width: state.width, height: state.height),
                         onTap: { p in
                             addressFocused = false
                             tappedAt = Date()
                             store.browserInput(.tap(x: p.x, y: p.y), in: threadID)
                         },
                         onScroll: { p, d in store.browserInput(.scroll(x: p.x, y: p.y, dx: d.width, dy: d.height), in: threadID) })
                    .ignoresSafeArea(.keyboard)
                    .onGeometryChange(for: CGSize.self, of: { $0.size }) { pageArea = $0 }
            } else {
                ProgressView("Asking your Mac…")
            }
            PageKeyboard(active: $typing, onText: { store.browserInput(.text($0), in: threadID) }, onKey: { store.browserInput(.key($0), in: threadID) })
                .frame(width: 1, height: 1)
                .opacity(0)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { bar }
        .overlay(alignment: .top) {
            if let error = store.actionError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.footnote)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .glassEffect(in: .capsule)
                    .padding(.top, 6)
                    .onTapGesture { store.actionError = nil }
                    .task(id: error) {
                        try? await Task.sleep(for: .seconds(5))
                        if store.actionError == error { store.actionError = nil }
                    }
            }
        }
        .onAppear { store.watchBrowser(threadID) }
        .onDisappear { typing = false; store.unwatchBrowser() }
        // A field took the focus with a tap: the keyboard comes up, as in Safari.
        .onChange(of: state?.editing) { _, editing in
            if editing == true, !addressFocused, Date().timeIntervalSince(tappedAt) < 3 { typing = true }
        }
        .onChange(of: shown?.url) { _, url in if !addressFocused { address = url ?? "" } }
        .alert(state?.dialog.map { $0.kind == "alert" ? "The page says" : "The page asks" } ?? "", isPresented: Binding(get: { state?.dialog != nil }, set: { _ in })) {
            if state?.dialog?.kind == "prompt" { TextField("Answer", text: $dialogText) }
            if state?.dialog?.kind != "alert" {
                Button("Cancel", role: .cancel) { store.browserInput(.dialog(accept: false, text: nil), in: threadID) }
            }
            Button("OK") { store.browserInput(.dialog(accept: true, text: state?.dialog?.kind == "prompt" ? dialogText : nil), in: threadID) }
        } message: {
            Text(state?.dialog?.message ?? "")
        }
        .onChange(of: state?.dialog) { _, d in dialogText = d?.defaultText ?? "" }
        .task(id: store.browserFrame == nil) { await runHooks() }
    }

    /// Back, forward, the address (an address or words to search for), reload.
    private var bar: some View {
        HStack(spacing: 6) {
            Button("Back", systemImage: "chevron.backward") { store.browserInput(.back, in: threadID) }
                .disabled(state?.canBack != true)
            Button("Forward", systemImage: "chevron.forward") { store.browserInput(.forward, in: threadID) }
                .disabled(state?.canForward != true)
            HStack(spacing: 6) {
                if shown?.loading == true { ProgressView().controlSize(.small) }
                TextField("Address or search", text: $address)
                    .focused($addressFocused)
                    .keyboardType(.webSearch)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.go)
                    .onSubmit(go)
                    .onChange(of: addressFocused) { _, focused in
                        if focused { typing = false } else { address = shown?.url ?? address }
                    }
            }
            .padding(.horizontal, 14)
            .frame(height: 44)
            .glassEffect(in: .capsule)
            Button("Reload", systemImage: "arrow.clockwise") { store.browserInput(.reload, in: threadID) }
                .disabled(shown == nil)
        }
        .labelStyle(.iconOnly)
        .buttonStyle(.glass)
        .controlSize(.large)
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
        .onAppear { address = shown?.url ?? "" }
    }

    /// The tabs, a new one, closing this one, and the page's size.
    private var menu: some View {
        Menu("More", systemImage: state.map { $0.tabs.count > 1 ? "\(min($0.tabs.count, 50)).square" : "square.on.square" } ?? "square.on.square") {
            if let state, !state.tabs.isEmpty {
                Section("Tabs") {
                    ForEach(state.tabs) { tab in
                        Button {
                            store.browserInput(.selectTab(tab.id), in: threadID)
                        } label: {
                            if tab.id == state.shown?.id {
                                Label(tab.title.isEmpty ? Self.host(tab.url) : tab.title, systemImage: "checkmark")
                            } else {
                                Text(tab.title.isEmpty ? Self.host(tab.url) : tab.title)
                            }
                        }
                    }
                }
                if let shown {
                    Button("Close Tab", systemImage: "xmark", role: .destructive) { store.browserInput(.closeTab(shown.id), in: threadID) }
                }
                Section("Page Size") {
                    sizeButton("This Phone", "phone", width: pageArea.width.rounded())
                    sizeButton("Tablet", "tablet", width: 834)
                    sizeButton("Laptop", "laptop", width: 1512)
                    sizeButton("Fit the Mac's Panel", "fit", width: nil)
                }
            }
            Button("New Tab", systemImage: "plus") {
                address = ""
                addressFocused = true
            }
        }
    }

    private func sizeButton(_ title: String, _ preset: String, width: Double?) -> some View {
        Button {
            if preset == "phone", pageArea.width > 100 {
                store.browserInput(.size(preset, width: pageArea.width.rounded(), height: pageArea.height.rounded()), in: threadID)
            } else {
                store.browserInput(.size(preset), in: threadID)
            }
        } label: {
            if let width, state?.width == width { Label(title, systemImage: "checkmark") } else { Text(title) }
        }
    }

    private func go() {
        let text = address.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return }
        addressFocused = false
        // A new tab when none is open, or when New Tab cleared the field.
        store.browserInput(shown == nil ? .open(text) : .navigate(text), in: threadID)
    }

    static func host(_ url: String) -> String {
        guard let u = URL(string: url), let host = u.host() else { return url }
        return u.port.map { "\(host):\($0)" } ?? host
    }

    /// Test hooks, doing what the gestures do once the first picture shows:
    /// `YH_PHONE_BROWSER=tap:x,y;type:text;key:Enter;scroll:dy;size:phone;go:url;back`, a second and a half apart.
    private func runHooks() async {
        guard store.browserFrame != nil, !Self.ranHooks, let steps = Self.env["YH_PHONE_BROWSER"] else { return }
        Self.ranHooks = true
        for step in steps.split(separator: ";").map(String.init) {
            try? await Task.sleep(for: .seconds(1.5))
            let parts = step.split(separator: ":", maxSplits: 1).map(String.init)
            let arg = parts.count > 1 ? parts[1] : ""
            Perf.shared.log("PHONE browser \(step)")
            switch parts[0] {
            case "tap":
                let xy = arg.split(separator: ",").compactMap { Double($0) }
                if xy.count == 2 { store.browserInput(.tap(x: xy[0], y: xy[1]), in: threadID) }
            case "type": store.browserInput(.text(arg), in: threadID)
            case "key": store.browserInput(.key(arg), in: threadID)
            case "scroll": store.browserInput(.scroll(x: (state?.width ?? 0) / 2, y: (state?.height ?? 0) / 2, dx: 0, dy: Double(arg) ?? 400), in: threadID)
            case "size" where arg == "phone": store.browserInput(.size(arg, width: pageArea.width.rounded(), height: pageArea.height.rounded()), in: threadID)
            case "size": store.browserInput(.size(arg), in: threadID)
            case "go": store.browserInput(.navigate(arg), in: threadID)
            case "back": store.browserInput(.back, in: threadID)
            case "keyboard": typing = true
            default: break
            }
        }
    }
}

/// The picture of the page, fitted to the width: a tap and a one-finger drag go to the page (in
/// its CSS pixels), two fingers pinch and pan the picture.
struct PageView: UIViewRepresentable {
    let image: UIImage
    let pageSize: CGSize
    let onTap: (CGPoint) -> Void
    let onScroll: (CGPoint, CGSize) -> Void

    func makeUIView(context: Context) -> PageScrollView {
        let view = PageScrollView()
        view.onTap = onTap
        view.onScroll = onScroll
        return view
    }

    func updateUIView(_ view: PageScrollView, context: Context) {
        view.onTap = onTap
        view.onScroll = onScroll
        view.show(image, pageSize: pageSize)
    }
}

final class PageScrollView: UIScrollView, UIScrollViewDelegate {
    private let imageView = UIImageView()
    private var pageSize = CGSize.zero
    var onTap: ((CGPoint) -> Void)?
    var onScroll: ((CGPoint, CGSize) -> Void)?
    /// Scrolling not sent yet (CSS pixels), and where it started.
    private var pending = CGSize.zero
    private var origin = CGPoint.zero
    private var lastSent = Date.distantPast
    private var momentum: Task<Void, Never>?

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        minimumZoomScale = 1
        maximumZoomScale = 5
        bouncesZoom = true
        showsVerticalScrollIndicator = false
        showsHorizontalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        // Two fingers move the picture; one is the page's.
        panGestureRecognizer.minimumNumberOfTouches = 2
        imageView.isUserInteractionEnabled = true
        imageView.contentMode = .scaleToFill
        imageView.layer.shadowColor = UIColor.black.cgColor
        imageView.layer.shadowOpacity = 0.12
        imageView.layer.shadowRadius = 6
        addSubview(imageView)
        let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
        imageView.addGestureRecognizer(tap)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        imageView.addGestureRecognizer(pan)
        let double = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
        double.numberOfTouchesRequired = 2
        addGestureRecognizer(double)
        accessibilityLabel = "The page in your Mac's browser"
    }

    required init?(coder: NSCoder) { fatalError() }

    func show(_ image: UIImage, pageSize: CGSize) {
        if imageView.image !== image { imageView.image = image }
        if self.pageSize != pageSize {
            self.pageSize = pageSize
            setNeedsLayout()
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard pageSize.width > 0, bounds.width > 0, zoomScale == 1 else { return center() }
        // Fitted: the whole width, or the whole page when it's taller than the screen is wide.
        let fit = min(bounds.width / pageSize.width, (bounds.height - 8) / pageSize.height)
        let size = CGSize(width: (pageSize.width * fit).rounded(), height: (pageSize.height * fit).rounded())
        if imageView.bounds.size != size {
            imageView.frame = CGRect(origin: .zero, size: size)
            contentSize = size
        }
        center()
    }

    /// The picture at the top, in the middle across when it's narrower than the screen.
    private func center() {
        let f = imageView.frame
        imageView.center = CGPoint(x: max(f.width, bounds.width) / 2, y: f.height / 2)
        imageView.layer.shadowPath = UIBezierPath(rect: imageView.bounds).cgPath
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }
    func scrollViewDidZoom(_ scrollView: UIScrollView) { center() }

    /// A point on the picture in the page's CSS pixels.
    private func page(_ p: CGPoint) -> CGPoint {
        let b = imageView.bounds.size
        guard b.width > 0 else { return .zero }
        return CGPoint(x: (p.x / b.width * pageSize.width).rounded(), y: (p.y / b.height * pageSize.height).rounded())
    }

    private var cssPerPoint: CGFloat { imageView.bounds.width > 0 ? pageSize.width / imageView.bounds.width : 1 }

    @objc private func tapped(_ r: UITapGestureRecognizer) {
        let p = r.location(in: imageView)
        momentum?.cancel()
        ripple(at: p)
        onTap?(page(p))
    }

    @objc private func doubleTapped(_ r: UITapGestureRecognizer) {
        setZoomScale(zoomScale > 1 ? 1 : 2.5, animated: true)
    }

    @objc private func panned(_ r: UIPanGestureRecognizer) {
        switch r.state {
        case .began:
            momentum?.cancel()
            origin = page(r.location(in: imageView))
            pending = .zero
        case .changed:
            let t = r.translation(in: imageView)
            r.setTranslation(.zero, in: imageView)
            pending.width -= t.x * cssPerPoint
            pending.height -= t.y * cssPerPoint
            flush(force: false)
        case .ended, .cancelled:
            flush(force: true)
            let v = r.velocity(in: imageView)
            coast(CGSize(width: -v.x * cssPerPoint, height: -v.y * cssPerPoint))
        default: break
        }
    }

    /// What's scrolled since the last send, at most twenty times a second.
    private func flush(force: Bool) {
        guard pending != .zero, force || Date().timeIntervalSince(lastSent) > 0.05 else { return }
        onScroll?(origin, CGSize(width: pending.width.rounded(), height: pending.height.rounded()))
        pending = .zero
        lastSent = Date()
    }

    /// A flick keeps scrolling the page a little, slowing down.
    private func coast(_ velocity: CGSize) {
        guard hypot(velocity.width, velocity.height) > 300 else { return }
        momentum = Task { @MainActor [weak self] in
            var v = velocity
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(60))
                let step = CGSize(width: v.width * 0.06, height: v.height * 0.06)
                guard let self, !Task.isCancelled, hypot(step.width, step.height) >= 4 else { return }
                onScroll?(origin, CGSize(width: step.width.rounded(), height: step.height.rounded()))
                v.width *= 0.8
                v.height *= 0.8
            }
        }
    }

    /// Where the tap went, for a moment: the page changes only once the Mac's picture comes.
    private func ripple(at p: CGPoint) {
        let dot = UIView(frame: CGRect(x: 0, y: 0, width: 36, height: 36))
        dot.center = p
        dot.layer.cornerRadius = 18
        dot.backgroundColor = tintColor.withAlphaComponent(0.35)
        dot.isUserInteractionEnabled = false
        dot.transform = CGAffineTransform(scaleX: 0.4, y: 0.4)
        imageView.addSubview(dot)
        UIView.animate(withDuration: 0.45, delay: 0, options: .curveEaseOut) {
            dot.transform = .identity
            dot.alpha = 0
        } completion: { _ in dot.removeFromSuperview() }
    }
}

/// The keyboard for the page: whatever's typed goes to the page's focused field on the Mac,
/// Return and Delete as keys.
struct PageKeyboard: UIViewRepresentable {
    @Binding var active: Bool
    let onText: (String) -> Void
    let onKey: (String) -> Void

    func makeUIView(context: Context) -> KeyInputView {
        let view = KeyInputView()
        view.onEnd = { active = false }
        return view
    }

    func updateUIView(_ view: KeyInputView, context: Context) {
        view.onText = onText
        view.onKey = onKey
        view.onEnd = { active = false }
        DispatchQueue.main.async {
            if active, !view.isFirstResponder { view.becomeFirstResponder() }
            if !active, view.isFirstResponder { _ = view.resignFirstResponder() }
        }
    }
}

final class KeyInputView: UIView, UIKeyInput {
    var onText: ((String) -> Void)?
    var onKey: ((String) -> Void)?
    var onEnd: (() -> Void)?
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var keyboardType: UIKeyboardType = .default
    var returnKeyType: UIReturnKeyType = .default

    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { true }

    func insertText(_ text: String) {
        if text == "\n" { onKey?("Enter") } else if text == "\t" { onKey?("Tab") } else { onText?(text) }
    }

    func deleteBackward() { onKey?("Backspace") }

    override func resignFirstResponder() -> Bool {
        let done = super.resignFirstResponder()
        if done { onEnd?() }
        return done
    }

    override var keyCommands: [UIKeyCommand]? {
        [UIKeyCommand.inputUpArrow, UIKeyCommand.inputDownArrow, UIKeyCommand.inputLeftArrow, UIKeyCommand.inputRightArrow, UIKeyCommand.inputEscape].map {
            let c = UIKeyCommand(input: $0, modifierFlags: [], action: #selector(arrow(_:)))
            c.wantsPriorityOverSystemBehavior = true
            return c
        }
    }

    @objc private func arrow(_ command: UIKeyCommand) {
        let names = [UIKeyCommand.inputUpArrow: "ArrowUp", UIKeyCommand.inputDownArrow: "ArrowDown", UIKeyCommand.inputLeftArrow: "ArrowLeft", UIKeyCommand.inputRightArrow: "ArrowRight", UIKeyCommand.inputEscape: "Escape"]
        if let input = command.input, let name = names[input] { onKey?(name) }
    }
}

/// A web address on the Mac itself (`localhost` and friends): opened in the thread's browser there.
enum LocalLink {
    static func isLocal(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https", let host = url.host()?.lowercased() else { return false }
        return ["localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]"].contains(host) || host.hasSuffix(".localhost")
    }
}
