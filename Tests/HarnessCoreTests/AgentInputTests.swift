import CoreGraphics
import Foundation
import ImageIO
import Testing
import UniformTypeIdentifiers
@testable import HarnessCore

/// A solid-color image, encoded as `type`.
func testImage(width: Int, height: Int, type: UTType = .png, red: Bool = true) -> Data {
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.setFillColor(red ? CGColor(red: 0.9, green: 0.1, blue: 0.1, alpha: 1) : CGColor(red: 0.1, green: 0.2, blue: 0.9, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    let output = NSMutableData()
    let destination = CGImageDestinationCreateWithData(output, type.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, context.makeImage()!, nil)
    CGImageDestinationFinalize(destination)
    return output as Data
}

struct ImageAttachmentTests {
    @Test func imagesThatFitAreSentUntouched() throws {
        let data = testImage(width: 640, height: 480)
        let image = try #require(ImageAttachment.prepare(data))
        #expect(image.data == data)
        #expect(image.mediaType == "image/png")
        #expect(image.width == 640 && image.height == 480)
    }

    @Test func bigImagesAreScaledToTheLimit() throws {
        let image = try #require(ImageAttachment.prepare(testImage(width: 6000, height: 3000)))
        #expect(image.width == ImageAttachment.maxDimension)
        #expect(image.height == 1000)
        #expect(image.data.count <= ImageAttachment.maxBytes)
    }

    @Test func otherFormatsBecomePNG() throws {
        let image = try #require(ImageAttachment.prepare(testImage(width: 300, height: 200, type: .tiff)))
        #expect(image.mediaType == "image/png")
        #expect(image.fileExtension == "png")
        #expect(image.width == 300)
    }

    @Test func notAnImage() {
        #expect(ImageAttachment.prepare(Data("hello".utf8)) == nil)
    }

    @Test func sentAsABase64Block() throws {
        let image = try #require(ImageAttachment.prepare(testImage(width: 10, height: 10)))
        let block = image.messageBlock
        let source = try #require(block["source"] as? [String: String])
        #expect(block["type"] as? String == "image")
        #expect(source["type"] == "base64")
        #expect(source["media_type"] == "image/png")
        #expect(Data(base64Encoded: source["data"] ?? "") == image.data)
    }
}

struct AgentRequestTests {
    /// As the CLI sends it in a `can_use_tool` request.
    private let questionInput = #"{"questions":[{"question":"Which color do you prefer?","header":"Color","options":[{"label":"Red","description":"Warm"},{"label":"Blue"}],"multiSelect":false},{"question":"Which sizes?","header":"Size","options":[{"label":"S"},{"label":"M"},{"label":"L"}],"multiSelect":true}]}"#

    @Test func readsQuestions() throws {
        let questions = try #require(AgentQuestion.parse(inputJSON: questionInput))
        #expect(questions.count == 2)
        #expect(questions[0].header == "Color")
        #expect(questions[0].options.map(\.label) == ["Red", "Blue"])
        #expect(questions[0].options[0].description == "Warm")
        #expect(questions[1].multiSelect)
        #expect(AgentQuestion.parse(inputJSON: "{}") == nil)
    }

    @Test func answersGoBackInTheInput() throws {
        let json = AgentQuestion.answeredInput(inputJSON: questionInput, answers: [
            "Which color do you prefer?": ["Blue"],
            "Which sizes?": ["S", "L", "something custom"],
        ])
        let input = try #require(try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any])
        #expect(input["answers"] as? [String: String] == ["Which color do you prefer?": "Blue", "Which sizes?": "S, L, something custom"])
        #expect((input["questions"] as? [Any])?.count == 2, "the questions stay in the input")
    }

    @Test func readsThePlan() {
        #expect(AgentPlan.text(inputJSON: #"{"plan":"1. Do it","planFilePath":"/tmp/p.md"}"#) == "1. Do it")
        #expect(AgentPlan.text(inputJSON: #"{"plan":""}"#) == nil)
    }

    private let request = PermissionRequest(requestID: "r1", toolName: "ExitPlanMode", description: nil, inputJSON: #"{"plan":"x"}"#)

    private func body(_ decision: PermissionDecision) throws -> [String: Any] {
        let message = ClaudeSession.response(to: request, with: decision)
        let response = try #require(message["response"] as? [String: Any])
        #expect(response["request_id"] as? String == "r1")
        return try #require(response["response"] as? [String: Any])
    }

    @Test func approvingCanSwitchMode() throws {
        let allow = try body(.allow(mode: "auto"))
        #expect(allow["behavior"] as? String == "allow")
        #expect(allow["updatedInput"] as? [String: String] == ["plan": "x"])
        let permissions = try #require(allow["updatedPermissions"] as? [[String: String]])
        #expect(permissions == [["type": "setMode", "mode": "auto", "destination": "session"]])
        #expect(try body(.allow())["updatedPermissions"] == nil)
    }

    @Test func denyingCarriesFeedback() throws {
        let deny = try body(.deny(message: "Cover errors too."))
        #expect(deny["behavior"] as? String == "deny")
        #expect(deny["message"] as? String == "Cover errors too.")
    }
}

/// The real CLI: questions, plan approval and images, end to end. On Haiku.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["YH_INTEGRATION"] == "1"), .serialized)
struct AgentRequestIntegrationTests {
    private func makeSession(permissionMode: String? = nil) async throws -> (ClaudeSession, URL) {
        var environment = await ShellEnvironment.load()
        environment.removeValue(forKey: "CLAUDECODE")
        let executable = try #require(ShellEnvironment.which("claude", in: environment))
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("yh-requests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let session = try ClaudeSession(options: ClaudeLaunchOptions(
            executable: executable, workingDirectory: folder, environment: environment,
            model: "haiku", permissionMode: permissionMode
        ))
        return (session, folder)
    }

    /// Runs one turn, settling each permission request with `decide`; returns the reply text.
    private func turn(_ session: ClaudeSession, decide: (PermissionRequest) -> PermissionDecision) async throws -> (text: String, requests: [String]) {
        var text = ""
        var requests: [String] = []
        for await event in session.events {
            switch event {
            case .textDelta(_, let delta): text += delta
            case .permissionRequest(let request):
                requests.append(request.toolName)
                try await session.respond(to: request, with: decide(request))
            case .turnCompleted: return (text, requests)
            default: break
            }
        }
        return (text, requests)
    }

    @Test func answersAQuestion() async throws {
        let (session, _) = try await makeSession()
        try await session.send(text: "Use the AskUserQuestion tool to ask me which color I prefer, with options Red and Blue. Then reply with just my answer.")
        let result = try await turn(session) { request in
            guard request.toolName == AgentQuestion.toolName, let question = AgentQuestion.parse(inputJSON: request.inputJSON)?.first else { return .allow() }
            return .allow(updatedInputJSON: AgentQuestion.answeredInput(inputJSON: request.inputJSON, answers: [question.question: ["Blue"]]))
        }
        await session.terminate()
        #expect(result.requests.contains(AgentQuestion.toolName))
        #expect(result.text.contains("Blue"))
    }

    @Test func approvesAPlanIntoAutoMode() async throws {
        let (session, folder) = try await makeSession(permissionMode: "plan")
        try await session.send(text: "Plan creating a file hello.txt containing 'hi' in the current folder. One-line plan; present it for approval, then do it.")
        let result = try await turn(session) { request in
            request.toolName == AgentPlan.toolName ? .allow(mode: "auto") : .deny(message: "Only the plan should ask.")
        }
        await session.terminate()
        #expect(result.requests.contains(AgentPlan.toolName))
        #expect(!result.requests.contains("Write"), "after approving into Auto, the edit shouldn't ask")
        #expect((try? String(contentsOf: folder.appendingPathComponent("hello.txt"), encoding: .utf8))?.contains("hi") == true)
    }

    @Test func seesAnImage() async throws {
        let (session, _) = try await makeSession()
        let image = try #require(ImageAttachment.prepare(testImage(width: 400, height: 300, red: false)))
        try await session.send(text: "What is the main color of this image? Answer with one lowercase word.", images: [image])
        let result = try await turn(session) { _ in .allow() }
        await session.terminate()
        #expect(result.text.lowercased().contains("blue"))
    }
}
