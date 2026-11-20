// Voice input: what you say, written into the message field. Two ways, picked in Settings →
// Voice Input (`dictation.engine`):
// - On the iPhone (the default): iOS 26's speech model (`SpeechAnalyzer` with
//   `SpeechTranscriber`), on the device, words written as you say them. Its language's model is
//   downloaded by iOS the first time. Where it isn't available, the older recognizer
//   (`SFSpeechRecognizer`), which also needs Speech Recognition allowed.
// - On the Mac: the recording sent when you stop (16 kHz mono, protocol 9), written out by the
//   voice model picked on the Mac (Settings → Voice there, Whisper), the text coming back.

import AVFoundation
import Observation
import Speech

@Observable @MainActor
final class Dictation {
    enum State: Equatable {
        case idle
        /// iOS fetching the speech model for the language, the first time.
        case preparing
        case listening
        /// Stopped; the Mac writing the recording out.
        case transcribing
        /// Why it can't listen (no permission, no recognizer for this language).
        case unavailable(String)
    }

    enum Engine: String, CaseIterable, Identifiable {
        case phone, mac
        var id: String { rawValue }
    }

    var state: State = .idle
    /// What's been heard so far this time, punctuated.
    var transcript = ""

    /// Why the words didn't come, said as the field's error.
    struct Failed: Error { let message: String }

    /// Asks the Mac to write a recording out (16 kHz mono 16-bit samples); `nil`: on the phone.
    typealias Mac = (Data) async -> Result<String, Dictation.Failed>

    @ObservationIgnored private let engine = AVAudioEngine()
    // iOS 26's analyzer.
    @ObservationIgnored private var input: AsyncStream<AnalyzerInput>.Continuation?
    @ObservationIgnored private var analyzer: SpeechAnalyzer?
    @ObservationIgnored private var results: Task<Void, Never>?
    // The older recognizer.
    @ObservationIgnored private var request: SFSpeechAudioBufferRecognitionRequest?
    @ObservationIgnored private var task: SFSpeechRecognitionTask?
    // The Mac's: the recording so far, and where it goes.
    @ObservationIgnored private var recording: Recording?
    @ObservationIgnored private var mac: Mac?
    /// Counts each start, so a late end of one dictation leaves the next alone.
    @ObservationIgnored private var session = 0

    /// Listening for the Mac: the words come once it stops.
    var toMac: Bool { recording != nil }

    /// The language picked in Settings (`dictation.language`), or the phone's own.
    static var locale: Locale {
        UserDefaults.standard.string(forKey: "dictation.language").flatMap { $0.isEmpty ? nil : Locale(identifier: $0) } ?? .current
    }

    /// The languages iOS's speech model knows, for Settings.
    static func languages() async -> [Locale] {
        guard SpeechTranscriber.isAvailable else { return [] }
        return await SpeechTranscriber.supportedLocales.sorted { $0.identifier < $1.identifier }
    }

    /// Starts or stops; `mac` set: the Mac writes it out.
    func toggle(mac: Mac? = nil) {
        switch state {
        case .listening: stop()
        case .preparing, .transcribing: return
        default: Task { await start(mac: mac) }
        }
    }

    func start(mac: Mac? = nil) async {
        guard await AVAudioApplication.requestRecordPermission() else {
            state = .unavailable("Allow the microphone for YourHarness in Settings.")
            return
        }
        transcript = ""
        session += 1
        let audio = AVAudioSession.sharedInstance()
        do {
            try audio.setCategory(.record, mode: .measurement, options: .duckOthers)
            try audio.setActive(true, options: .notifyOthersOnDeactivation)
        } catch {
            state = .unavailable("The microphone isn't available.")
            return
        }
        do {
            if let mac {
                try startForMac(mac)
            } else if SpeechTranscriber.isAvailable {
                try await startAnalyzer()
            } else {
                try await startRecognizer()
            }
        } catch let why as Unavailable {
            finish()
            state = .unavailable(why.message)
        } catch {
            finish()
            state = .unavailable("Dictation couldn't start: \(error.localizedDescription)")
        }
    }

    /// Stops listening; the last words still come in (from the Mac, all of them).
    func stop() {
        guard state == .listening else { return }
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        if let input {
            input.finish()
            self.input = nil
            let (analyzer, results, session) = (analyzer, results, session)
            Task { [weak self] in
                try? await analyzer?.finalizeAndFinishThroughEndOfInput()
                await results?.value
                if self?.session == session { self?.finish() }
            }
            state = .idle
        } else if let recording, let mac {
            self.recording = nil
            state = .transcribing
            let (audio, session) = (recording.data, session)
            Task { [weak self] in
                let answer = await mac(audio)
                guard let self, self.session == session else { return }
                self.finish()
                switch answer {
                case .success(let text): self.transcript = text.trimmingCharacters(in: .whitespacesAndNewlines)
                case .failure(let why): self.state = .unavailable(why.message)
                }
            }
        } else {
            request?.endAudio()
            state = .idle
        }
    }

    private func finish() {
        if engine.isRunning {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
        }
        input?.finish()
        input = nil
        analyzer = nil
        results = nil
        request = nil
        task = nil
        recording = nil
        mac = nil
        if state == .listening || state == .preparing || state == .transcribing { state = .idle }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    private struct Unavailable: Error { let message: String }

    // MARK: iOS 26's speech model

    private func transcriber() async throws -> SpeechTranscriber {
        let asked = Self.locale
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: asked) else {
            let name = Locale.current.localizedString(forIdentifier: asked.identifier) ?? asked.identifier
            throw Unavailable(message: "Dictation on the iPhone doesn't know \(name). Pick another language in Settings, or transcribe on the Mac.")
        }
        let transcriber = SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [.volatileResults], attributeOptions: [])
        // The language's model, fetched by iOS the first time (shared with other apps).
        if let install = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
            state = .preparing
            try await install.downloadAndInstall()
        }
        return transcriber
    }

    private func startAnalyzer() async throws {
        let transcriber = try await transcriber()
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
            throw Unavailable(message: "Dictation isn't available right now.")
        }
        let (stream, input) = AsyncStream.makeStream(of: AnalyzerInput.self)
        self.analyzer = analyzer
        self.input = input
        // Final pieces stay; the latest guess at what follows is replaced as it firms up.
        results = Task { [weak self] in
            var settled = ""
            do {
                for try await result in transcriber.results {
                    let text = String(result.text.characters)
                    if result.isFinal { settled += text }
                    let shown = (result.isFinal ? settled : settled + text).trimmingCharacters(in: .whitespacesAndNewlines)
                    self?.transcript = shown
                }
            } catch {}
        }
        try await analyzer.start(inputSequence: stream)
        try Self.feed(engine.inputNode, as: format) { input.yield(AnalyzerInput(buffer: $0)) }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            throw Unavailable(message: "The microphone couldn't start.")
        }
        state = .listening
    }

    // MARK: The Mac's model

    /// The recording so far, as the Mac wants it; appended to on the audio thread.
    private final class Recording: @unchecked Sendable {
        private let lock = NSLock()
        private var bytes = Data()

        func append(_ buffer: AVAudioPCMBuffer) {
            guard let samples = buffer.int16ChannelData else { return }
            lock.withLock { bytes.append(Data(bytes: samples[0], count: Int(buffer.frameLength) * 2)) }
        }

        var data: Data { lock.withLock { bytes } }
    }

    static let macFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!

    private func startForMac(_ mac: @escaping Mac) throws {
        let recording = Recording()
        self.recording = recording
        self.mac = mac
        try Self.feed(engine.inputNode, as: Self.macFormat) { recording.append($0) }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            throw Unavailable(message: "The microphone couldn't start.")
        }
        state = .listening
    }

    // MARK: Audio

    /// Turns the microphone's buffers into `format`'s; called on the audio thread.
    private final class Converter: @unchecked Sendable {
        let converter: AVAudioConverter?
        let format: AVAudioFormat

        init(from: AVAudioFormat, to format: AVAudioFormat) {
            self.format = format
            converter = from == format ? nil : AVAudioConverter(from: from, to: format)
        }

        func convert(_ buffer: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
            guard let converter else { return buffer }
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * format.sampleRate / buffer.format.sampleRate) + 16
            guard let out = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else { return nil }
            nonisolated(unsafe) var given = false
            var error: NSError?
            converter.convert(to: out, error: &error) { _, status in
                if given {
                    status.pointee = .noDataNow
                    return nil
                }
                given = true
                status.pointee = .haveData
                return buffer
            }
            return error == nil && out.frameLength > 0 ? out : nil
        }

        /// A whole file, converted (test runs).
        static func read(_ url: URL, as format: AVAudioFormat) throws -> [AVAudioPCMBuffer] {
            let file = try AVAudioFile(forReading: url)
            let converter = Converter(from: file.processingFormat, to: format)
            var out: [AVAudioPCMBuffer] = []
            while file.framePosition < file.length {
                guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 16_384) else { break }
                try file.read(into: buffer)
                if buffer.frameLength == 0 { break }
                if let converted = converter.convert(buffer) { out.append(converted) }
            }
            return out
        }
    }

    // The microphone's tap and the recognizer's handler are closures iOS calls off the main
    // thread. Made here, not on the main actor, so Swift's isolation check doesn't stop the app
    // when they're called.
    nonisolated private static func feed(_ input: AVAudioInputNode, as format: AVAudioFormat, to take: @escaping @Sendable (AVAudioPCMBuffer) -> Void) throws {
        let from = input.outputFormat(forBus: 0)
        guard from.sampleRate > 0 else { throw Unavailable(message: "The microphone isn't available.") }
        let converter = Converter(from: from, to: format)
        input.installTap(onBus: 0, bufferSize: 4096, format: from) { buffer, _ in
            if let converted = converter.convert(buffer) { take(converted) }
        }
    }

    // MARK: The older recognizer, where iOS 26's model isn't available

    @ObservationIgnored private lazy var recognizer = SFSpeechRecognizer(locale: Self.locale) ?? SFSpeechRecognizer(locale: Locale(identifier: "en-US"))

    private func startRecognizer() async throws {
        guard await Self.speechAllowed() else {
            throw Unavailable(message: "Allow Speech Recognition for YourHarness in Settings.")
        }
        guard let recognizer, recognizer.isAvailable else {
            throw Unavailable(message: "Dictation isn't available right now.")
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        self.request = request
        nonisolated(unsafe) let feeding = request
        try Self.feed(engine.inputNode, as: engine.inputNode.outputFormat(forBus: 0)) { feeding.append($0) }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            throw Unavailable(message: "The microphone couldn't start.")
        }
        state = .listening
        task = Self.recognize(request, with: recognizer) { [weak self] text, done in
            guard let self else { return }
            if let text { self.transcript = text }
            if done { self.finish() }
        }
    }

    nonisolated private static func recognize(
        _ request: SFSpeechAudioBufferRecognitionRequest,
        with recognizer: SFSpeechRecognizer,
        heard: @escaping @MainActor @Sendable (String?, Bool) -> Void
    ) -> SFSpeechRecognitionTask {
        recognizer.recognitionTask(with: request) { result, error in
            let text = result?.bestTranscription.formattedString
            let done = (result?.isFinal ?? false) || error != nil
            Task { @MainActor in heard(text, done) }
        }
    }

    // Not on the main actor: iOS answers this on a queue of its own, and a handler tied to the
    // main actor stops the app there (Swift's isolation check).
    nonisolated private static func speechAllowed() async -> Bool {
        await withCheckedContinuation { done in
            SFSpeechRecognizer.requestAuthorization { done.resume(returning: $0 == .authorized) }
        }
    }

    // MARK: Test runs

    /// A recording's words, as dictation would write them (`YH_PHONE_DICTATE_FILE`); the
    /// engine that wrote them, or why it couldn't.
    func transcribe(file: URL, mac: Mac? = nil) async -> (String, Result<String, Dictation.Failed>) {
        do {
            if let mac {
                let recording = Recording()
                try Converter.read(file, as: Self.macFormat).forEach(recording.append)
                return ("mac", await mac(recording.data))
            }
            if SpeechTranscriber.isAvailable {
                let transcriber = try await transcriber()
                let analyzer = SpeechAnalyzer(modules: [transcriber])
                guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else { return ("analyzer", .failure(Failed(message: "no audio format"))) }
                let buffers = try Converter.read(file, as: format)
                let (stream, input) = AsyncStream.makeStream(of: AnalyzerInput.self)
                let collected = Task {
                    var text = ""
                    for try await result in transcriber.results where result.isFinal { text += String(result.text.characters) }
                    return text
                }
                try await analyzer.start(inputSequence: stream)
                buffers.forEach { input.yield(AnalyzerInput(buffer: $0)) }
                input.finish()
                try await analyzer.finalizeAndFinishThroughEndOfInput()
                state = .idle
                return ("analyzer", .success(try await collected.value.trimmingCharacters(in: .whitespacesAndNewlines)))
            }
            return ("recognizer", await recognizeFile(file))
        } catch let why as Unavailable {
            return ("analyzer", .failure(Failed(message: why.message)))
        } catch {
            return ("analyzer", .failure(Failed(message: error.localizedDescription)))
        }
    }

    private func recognizeFile(_ file: URL) async -> Result<String, Dictation.Failed> {
        guard await Self.speechAllowed(), let recognizer, recognizer.isAvailable else { return .failure(Failed(message: "Speech Recognition isn't allowed or available.")) }
        let request = SFSpeechURLRecognitionRequest(url: file)
        request.addsPunctuation = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        return await withCheckedContinuation { (done: CheckedContinuation<Result<String, Dictation.Failed>, Never>) in
            nonisolated(unsafe) var answered = false
            recognizer.recognitionTask(with: request) { result, error in
                guard !answered else { return }
                if let result, result.isFinal {
                    answered = true
                    done.resume(returning: .success(result.bestTranscription.formattedString))
                } else if let error {
                    answered = true
                    done.resume(returning: .failure(Failed(message: error.localizedDescription)))
                }
            }
        }
    }
}
