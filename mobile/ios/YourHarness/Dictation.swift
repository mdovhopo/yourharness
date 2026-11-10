// Voice input: what you say, written into the message field as you say it. Apple's speech
// recognition, on the device when it can be; the microphone and speech recognition are asked
// for the first time.

import AVFoundation
import Observation
import Speech

@Observable @MainActor
final class Dictation {
    enum State: Equatable {
        case idle
        case listening
        /// Why it can't listen (no permission, no recognizer for this language).
        case unavailable(String)
    }

    var state: State = .idle
    /// What's been heard so far this time, punctuated.
    var transcript = ""

    @ObservationIgnored private let engine = AVAudioEngine()
    @ObservationIgnored private var request: SFSpeechAudioBufferRecognitionRequest?
    @ObservationIgnored private var task: SFSpeechRecognitionTask?
    @ObservationIgnored private let recognizer = SFSpeechRecognizer(locale: .current) ?? SFSpeechRecognizer(locale: Locale(identifier: "en-US"))

    func toggle() {
        if state == .listening { stop() } else { Task { await start() } }
    }

    func start() async {
        guard await Self.allowed() else {
            state = .unavailable("Allow the microphone and Speech Recognition for YourHarness in Settings.")
            return
        }
        guard let recognizer, recognizer.isAvailable else {
            state = .unavailable("Dictation isn't available right now.")
            return
        }
        transcript = ""
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.record, mode: .measurement, options: .duckOthers)
            try session.setActive(true, options: .notifyOthersOnDeactivation)
        } catch {
            state = .unavailable("The microphone isn't available.")
            return
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        self.request = request
        Self.feed(engine.inputNode, to: request)
        engine.prepare()
        do {
            try engine.start()
        } catch {
            finish()
            state = .unavailable("The microphone couldn't start.")
            return
        }
        state = .listening
        task = Self.recognize(request, with: recognizer) { [weak self] text, done in
            guard let self else { return }
            if let text { self.transcript = text }
            if done { self.finish() }
        }
    }

    // These two hand iOS closures it calls off the main thread: the microphone's on its audio
    // thread, the recognizer's on a queue of its own. Made here, not on the main actor, so Swift's
    // isolation check doesn't stop the app when they're called.
    nonisolated private static func feed(_ input: AVAudioInputNode, to request: SFSpeechAudioBufferRecognitionRequest) {
        nonisolated(unsafe) let request = request
        input.installTap(onBus: 0, bufferSize: 1024, format: input.outputFormat(forBus: 0)) { buffer, _ in request.append(buffer) }
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

    /// Stops listening; the last words still come in.
    func stop() {
        guard state == .listening else { return }
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        request?.endAudio()
        state = .idle
    }

    private func finish() {
        if engine.isRunning {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
        }
        request = nil
        task = nil
        if state == .listening { state = .idle }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// A recording's words, as dictation would write them (test runs: `YH_PHONE_DICTATE_FILE`).
    func transcribe(file: URL) async -> String? {
        guard await Self.speechAllowed(), let recognizer, recognizer.isAvailable else { return nil }
        let request = SFSpeechURLRecognitionRequest(url: file)
        request.addsPunctuation = true
        if recognizer.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        return await withCheckedContinuation { (done: CheckedContinuation<String?, Never>) in
            nonisolated(unsafe) var answered = false
            recognizer.recognitionTask(with: request) { result, error in
                guard !answered else { return }
                if let result, result.isFinal {
                    answered = true
                    done.resume(returning: result.bestTranscription.formattedString)
                } else if error != nil {
                    answered = true
                    done.resume(returning: nil)
                }
            }
        }
    }

    // Not on the main actor: iOS answers these on a queue of its own, and a handler tied to the
    // main actor stops the app there (Swift's isolation check).
    nonisolated private static func allowed() async -> Bool {
        guard await speechAllowed() else { return false }
        return await AVAudioApplication.requestRecordPermission()
    }

    nonisolated private static func speechAllowed() async -> Bool {
        await withCheckedContinuation { done in
            SFSpeechRecognizer.requestAuthorization { done.resume(returning: $0 == .authorized) }
        }
    }
}
