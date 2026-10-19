import CoreGraphics
import Foundation
import Testing
@testable import HarnessCore

struct FlowchartParserTests {
    @Test func shapesEdgesAndLabels() throws {
        let chart = try FlowchartParser.parse("""
        flowchart LR
            %% a comment
            A[Start] --> B{Is it ok?}
            B -->|Yes| C([Done])
            B -- No --> D[(Database)]
            D -.-> E((Retry)) ==> A
            F[["Sub (routine)"]] --- G{{hex}}; G --o H>flag]
            I[/in/] --x J[\\out\\]
            K <--> L
        """)
        #expect(chart.direction == .leftRight)
        let shapes = Dictionary(uniqueKeysWithValues: chart.nodes.map { ($0.id, $0.shape) })
        #expect(shapes["A"] == .rectangle)
        #expect(shapes["B"] == .diamond)
        #expect(shapes["C"] == .stadium)
        #expect(shapes["D"] == .cylinder)
        #expect(shapes["E"] == .circle)
        #expect(shapes["F"] == .subroutine)
        #expect(shapes["G"] == .hexagon)
        #expect(shapes["H"] == .flag)
        #expect(shapes["I"] == .parallelogram)
        #expect(shapes["J"] == .parallelogramAlt)
        #expect(chart.nodes.first { $0.id == "F" }?.label == "Sub (routine)")
        #expect(chart.nodes.first { $0.id == "B" }?.label == "Is it ok?")

        func edge(_ from: String, _ to: String) -> Flowchart.Edge? { chart.edges.first { $0.from == from && $0.to == to } }
        #expect(edge("B", "C")?.label == "Yes")
        #expect(edge("B", "D")?.label == "No")
        #expect(edge("D", "E")?.line == .dotted)
        #expect(edge("E", "A")?.line == .thick)
        #expect(edge("F", "G")?.end == Flowchart.Arrow.none)
        #expect(edge("G", "H")?.end == .circle)
        #expect(edge("I", "J")?.end == .cross)
        #expect(edge("K", "L")?.start == .arrow && edge("K", "L")?.end == .arrow)
    }

    @Test func ampersandsChainsAndCompactSyntax() throws {
        let chart = try FlowchartParser.parse("graph TD\nA & B --> C --> D & E\nX-->Y\nmy-node-->other_node")
        #expect(chart.direction == .topBottom)
        let pairs = Set(chart.edges.map { "\($0.from)>\($0.to)" })
        #expect(pairs == ["A>C", "B>C", "C>D", "C>E", "X>Y", "my-node>other_node"])
    }

    @Test func subgraphsAndIgnoredStyling() throws {
        let chart = try FlowchartParser.parse("""
        flowchart TB
            classDef hot fill:#f96
            subgraph api [API layer]
                direction LR
                A[Gateway]:::hot --> B[Service]
                subgraph db [Storage]
                    C[(Postgres)]
                end
            end
            subgraph "Web app"
                W[UI]
            end
            W --> api
            B --> C
            style A fill:#bbf
            click A "https://example.com"
        """)
        #expect(chart.subgraphs.map(\.title) == ["API layer", "Storage", "Web app"])
        #expect(chart.subgraphs[0].nodeIDs == ["A", "B"])
        #expect(chart.subgraphs[1].parent == "api")
        #expect(chart.subgraphs[2].nodeIDs == ["W"])
        // An edge into a subgraph lands on its first node.
        #expect(chart.edges.contains { $0.from == "W" && $0.to == "A" })
        #expect(chart.nodes.first { $0.id == "A" }?.label == "Gateway")
    }

    @Test func stylingIsHonored() throws {
        let chart = try FlowchartParser.parse("""
        flowchart LR
            classDef gap stroke:#e5484d,stroke-dasharray:4,stroke-width:2px
            classDef default fill:#f9f9f9
            classDef ok,good fill:green
            A[SSH open]:::gap --> B[Backups] --> C:::ok
            D --> E
            class D,E gap
            style B fill:#ff996680,stroke:rgb(10, 20, 30)
            subgraph risky [Risky]
                F
            end
            style risky stroke:red,stroke-dasharray: 5 5
        """)
        func node(_ id: String) -> Flowchart.Node { chart.nodes.first { $0.id == id }! }
        #expect(node("A").style.dashed && node("A").style.strokeWidth == 2)
        #expect(node("A").style.stroke == ShapeStyle.parse("stroke:#e5484d").stroke)
        #expect(node("A").style.fill == ShapeStyle.parse("fill:#f9f9f9").fill) // from `default`
        #expect(node("B").style.fill?.alpha == 128.0 / 255)
        #expect(node("B").style.stroke == DiagramColor(red: 10.0 / 255, green: 20.0 / 255, blue: 30.0 / 255))
        #expect(node("C").style.fill == DiagramColor.parse("green"))
        #expect(node("D").style.dashed && node("E").style.dashed)
        #expect(chart.subgraphs[0].style.dashed && chart.subgraphs[0].style.stroke == DiagramColor.parse("red"))
        #expect(DiagramColor.parse("#f96") == DiagramColor(red: 1, green: 0.6, blue: 0.4))
        #expect(DiagramColor.parse("nonsense") == nil)
    }

    @Test func labelsAreCleaned() throws {
        let chart = try FlowchartParser.parse(#"flowchart TD; A["Line one<br/>Line #quot;two#quot;"] --> B("**bold**")"#)
        #expect(chart.nodes[0].label == "Line one\nLine \"two\"")
        #expect(chart.nodes[1].label == "bold")
    }

    @Test func errorsAreReadable() {
        #expect(throws: MermaidError.self) { try FlowchartParser.parse("pie title Pets") }
        #expect(throws: MermaidError.self) { try FlowchartParser.parse("flowchart TD\nsubgraph x\nA-->B") }
        #expect(throws: MermaidError.self) { try FlowchartParser.parse("flowchart TD\nA -->") }
    }
}

struct LayeredLayoutTests {
    private let metrics = TextMetrics.monospaced()

    private func assertNoOverlaps(_ frames: [CGRect], sourceLocation: SourceLocation = #_sourceLocation) {
        for i in frames.indices {
            for j in frames.indices where j > i {
                #expect(!frames[i].insetBy(dx: 1, dy: 1).intersects(frames[j].insetBy(dx: 1, dy: 1)),
                        "nodes \(i) and \(j) overlap", sourceLocation: sourceLocation)
            }
        }
    }

    @Test(arguments: [Flowchart.Direction.topBottom, .bottomTop, .leftRight, .rightLeft])
    func flowsInTheRequestedDirection(_ direction: Flowchart.Direction) throws {
        var chart = try FlowchartParser.parse("flowchart TD\nA --> B --> C\nA --> C")
        chart.direction = direction
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let a = geometry.nodes[0].frame, c = geometry.nodes[2].frame
        switch direction {
        case .topBottom: #expect(a.midY < c.midY)
        case .bottomTop: #expect(a.midY > c.midY)
        case .leftRight: #expect(a.midX < c.midX)
        case .rightLeft: #expect(a.midX > c.midX)
        }
        assertNoOverlaps(geometry.nodes.map(\.frame))
        for node in geometry.nodes {
            #expect(node.frame.minX >= -0.5 && node.frame.minY >= -0.5)
            #expect(node.frame.maxX <= geometry.size.width + 0.5 && node.frame.maxY <= geometry.size.height + 0.5)
        }
    }

    @Test func edgesStartAndEndOnTheirNodes() throws {
        let chart = try FlowchartParser.parse("flowchart TD\nA[Start] --> B{Check} -->|yes| C((End))\nB --> D[Long label that wraps across more than one line for sure] --> C")
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let frames = Dictionary(uniqueKeysWithValues: geometry.nodes.map { ($0.node.id, $0.frame) })
        for edge in geometry.edges {
            let from = try #require(frames[edge.edge.from]), to = try #require(frames[edge.edge.to])
            let start = try #require(edge.points.first), end = try #require(edge.points.last)
            #expect(from.insetBy(dx: -1, dy: -1).contains(start))
            #expect(to.insetBy(dx: -1, dy: -1).contains(end))
            // On the outline, not the center.
            #expect(abs(start.y - from.midY) > 1 || abs(start.x - from.midX) > 1)
        }
        let labelled = try #require(geometry.edges.first { $0.edge.label == "yes" })
        #expect(labelled.labelFrame != nil)
        #expect(geometry.nodes.first { $0.node.id == "D" }!.label.lines.count > 1)
    }

    @Test func cyclesAndSelfLoopsStillLayOut() throws {
        let chart = try FlowchartParser.parse("flowchart TD\nA --> B --> C --> A\nC --> C\nB --> D")
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        assertNoOverlaps(geometry.nodes.map(\.frame))
        #expect(geometry.edges.allSatisfy { $0.points.count >= 2 })
        let loop = try #require(geometry.edges.first { $0.edge.from == "C" && $0.edge.to == "C" })
        #expect(loop.points.count == 4)
    }

    @Test func layoutSettlesWithoutCrossingsWhenPossible() throws {
        // Two independent chains drawn in an order that invites a crossing.
        let chart = try FlowchartParser.parse("flowchart TD\nA --> D\nB --> C\nA --> E\nB --> F")
        let nodes = chart.nodes.map { _ in LayeredLayout.Node(size: CGSize(width: 40, height: 20)) }
        let index = Dictionary(uniqueKeysWithValues: chart.nodes.enumerated().map { ($1.id, $0) })
        let edges = chart.edges.map { LayeredLayout.Edge(from: index[$0.from]!, to: index[$0.to]!) }
        let result = LayeredLayout.layout(nodes: nodes, edges: edges, direction: .topBottom)
        // No two edges between the same layers cross.
        for (i, a) in edges.enumerated() {
            for b in edges[(i + 1)...] {
                let a1 = result.frames[a.from].midX, a2 = result.frames[a.to].midX
                let b1 = result.frames[b.from].midX, b2 = result.frames[b.to].midX
                #expect((a1 - b1) * (a2 - b2) >= 0, "edges cross")
            }
        }
    }

    @Test func subgraphBoxesWrapTheirMembers() throws {
        let chart = try FlowchartParser.parse("""
        flowchart LR
          subgraph outer [Outer]
            A --> B
            subgraph inner [Inner]
              C
            end
          end
          B --> C --> D
        """)
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let frames = Dictionary(uniqueKeysWithValues: geometry.nodes.map { ($0.node.id, $0.frame) })
        let outer = try #require(geometry.subgraphs.first { $0.subgraph.id == "outer" })
        let inner = try #require(geometry.subgraphs.first { $0.subgraph.id == "inner" })
        #expect(outer.frame.contains(frames["A"]!) && outer.frame.contains(frames["B"]!))
        #expect(outer.frame.contains(inner.frame))
        #expect(inner.frame.contains(frames["C"]!))
        #expect(!outer.frame.contains(frames["D"]!))
        #expect(geometry.subgraphs.first?.subgraph.id == "outer") // drawn first, underneath
    }

    @Test func nodesOutsideASubgraphStayOutOfItsBox() throws {
        let chart = try FlowchartParser.parse("""
        flowchart TD
            W[Write JSON line to stdin] --> M
            subgraph CLI [Claude Code]
                M[Model turn] --> P{Needs permission?}
                P -->|Yes| A[Ask in app]
                P -->|No| R[Run tool]
                A -->|Allow| R
                R --> M
            end
            M -.->|stream-json| E[Event parser]
            E --> V[(Transcript store)]
            V --> UI[[Thread view]]
        """)
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let box = try #require(geometry.subgraphs.first).frame
        for node in geometry.nodes where !["M", "P", "A", "R"].contains(node.node.id) {
            #expect(!box.intersects(node.frame), "\(node.node.id) overlaps the subgraph")
        }
        assertNoOverlaps(geometry.nodes.map(\.frame))
    }

    @Test(arguments: [Flowchart.Direction.topBottom, .leftRight])
    func siblingSubgraphsDontOverlap(_ direction: Flowchart.Direction) throws {
        var chart = try FlowchartParser.parse("""
        flowchart LR
            subgraph GitHub
                D[Dependabot] -->|PRs| R[repos] --> GA[GitHub Actions]
            end
            subgraph VPS
                E[.env secrets] --> DC[docker compose]
                S[sshd :22] --> DC
            end
            I([Internet]) --> S
            L([Admin laptop]) -->|SSH| S
            L -.->|copied by hand| E
            GA -->|pull| DC
        """)
        chart.direction = direction
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let boxes = geometry.subgraphs.map(\.frame)
        #expect(boxes.count == 2)
        #expect(!boxes[0].intersects(boxes[1]), "subgraph boxes overlap")
    }

    @Test func largeGraphsAreFast() throws {
        var source = "flowchart TD\n"
        for i in 0..<120 {
            source += "N\(i)[Node \(i)] --> N\((i * 7 + 3) % 120)\n"
            if i % 3 == 0 { source += "N\(i) -->|step| N\((i + 11) % 120)\n" }
        }
        let chart = try FlowchartParser.parse(source)
        let clock = ContinuousClock()
        let start = clock.now
        let geometry = FlowchartLayout.layout(chart, metrics: metrics)
        let elapsed = clock.now - start
        print("120-node layout: \(elapsed)")
        #expect(elapsed < .milliseconds(500))
        assertNoOverlaps(geometry.nodes.map(\.frame))
    }

    @Test func isotonicRegressionRespectsOrder() {
        #expect(LayeredLayout.isotonic(targets: [1, 3, 2, 4], weights: [1, 1, 1, 1]) == [1, 2.5, 2.5, 4])
        #expect(LayeredLayout.isotonic(targets: [5, 1], weights: [3, 1]) == [4, 4])
    }

    @Test func clippingHitsEachOutline() {
        let frame = CGRect(x: 0, y: 0, width: 100, height: 40)
        let right = CGPoint(x: 200, y: 20)
        #expect(LayeredLayout.clip(frame, .rectangle, toward: right) == CGPoint(x: 100, y: 20))
        #expect(LayeredLayout.clip(frame, .ellipse, toward: right) == CGPoint(x: 100, y: 20))
        let below = CGPoint(x: 50, y: 300)
        #expect(LayeredLayout.clip(frame, .diamond, toward: below) == CGPoint(x: 50, y: 40))
    }
}

struct SequenceDiagramTests {
    private let source = """
    sequenceDiagram
        title Checkout
        autonumber
        actor U as User
        participant W as Web app
        participant A as API
        U->>+W: Click "Pay"
        W->>A: POST /payments
        activate A
        Note right of A: Validates the card
        alt card ok
            A-->>W: 201 Created
        else declined
            A--xW: 402
            W->>W: Show error
        end
        deactivate A
        loop every 2s
            W-)A: poll status
        end
        Note over W,A: Webhook arrives later
        W-->>-U: Done
    """

    @Test func parsesParticipantsMessagesAndBlocks() throws {
        let diagram = try SequenceParser.parse(source)
        #expect(diagram.title == "Checkout")
        #expect(diagram.autonumber)
        #expect(diagram.participants.map(\.id) == ["U", "W", "A"])
        #expect(diagram.participants.map(\.label) == ["User", "Web app", "API"])
        #expect(diagram.participants[0].isActor)

        guard case .message(let first) = diagram.items[0] else { Issue.record("expected a message"); return }
        #expect(first.from == "U" && first.to == "W" && first.activatesTarget && first.head == .filled)
        #expect(first.text == "Click \"Pay\"")

        let blocks = diagram.items.compactMap { item -> SequenceDiagram.Block? in
            if case .block(let block) = item { return block }
            return nil
        }
        #expect(blocks.map(\.kind) == [.alt, .loop])
        #expect(blocks[0].sections.map(\.label) == ["card ok", "declined"])
        guard case .message(let declined) = blocks[0].sections[1].items[0] else { Issue.record("expected a message"); return }
        #expect(declined.head == .cross && declined.line == .dotted)
        guard case .message(let poll) = blocks[1].sections[0].items[0] else { Issue.record("expected a message"); return }
        #expect(poll.head == .open)

        guard case .message(let last) = try #require(diagram.items.last) else { Issue.record("expected a message"); return }
        #expect(last.deactivatesSource && last.line == .dotted)
    }

    @Test func layoutKeepsReadingOrderAndRoom() throws {
        let diagram = try SequenceParser.parse(source)
        let geometry = SequenceLayout.layout(diagram, metrics: .monospaced())
        let xs = geometry.participants.map(\.centerX)
        #expect(xs == xs.sorted())
        // Messages go down the page in order.
        let ys = geometry.messages.map(\.y)
        #expect(ys == ys.sorted())
        #expect(geometry.messages.map(\.number) == Array(1...geometry.messages.count).map(Optional.some))
        // Every label fits between its endpoints.
        for message in geometry.messages where !message.isSelf {
            #expect(message.labelFrame.width <= abs(message.toX - message.fromX) + 1)
        }
        // Activation bars: U→+W (closed by -U at the end) and activate/deactivate A.
        #expect(geometry.activations.count == 2)
        #expect(geometry.blocks.count == 2)
        #expect(geometry.notes.count == 2)
        #expect(geometry.size.width > 0 && geometry.size.height > geometry.lifelineTop)
        for block in geometry.blocks { #expect(block.frame.minX >= -0.5) }
    }

    /// Real Claude output (Haiku, with our diagram instruction), kept as a fixture.
    @Test func realModelOutputParsesAndLaysOut() throws {
        let url = try #require(Bundle.module.url(forResource: "oauth-sequence", withExtension: "mmd", subdirectory: "Fixtures"))
        let source = try String(contentsOf: url, encoding: .utf8)
        guard case .success(.sequence(let diagram)) = MermaidDiagram.parse(source) else {
            Issue.record("expected a sequence diagram")
            return
        }
        #expect(diagram.participants.map(\.label) == ["User (Browser)", "App Server", "Auth Server"])
        let geometry = SequenceLayout.layout(diagram, metrics: .monospaced())
        #expect(geometry.messages.count == 20)
        #expect(geometry.notes.count == 1)
    }

    @Test func unbalancedBlocksAreErrors() {
        #expect(throws: MermaidError.self) { try SequenceParser.parse("sequenceDiagram\nloop x\nA->>B: hi") }
        #expect(throws: MermaidError.self) { try SequenceParser.parse("sequenceDiagram\nA->>B hi") }
    }

    @Test func dispatchesByHeader() {
        #expect(MermaidDiagram.isSupported("flowchart LR\nA-->B"))
        #expect(MermaidDiagram.isSupported("%% hi\nsequenceDiagram\nA->>B: x"))
        #expect(!MermaidDiagram.isSupported("pie title Pets\n\"Dogs\": 3"))
        if case .failure = MermaidDiagram.parse("---\ntitle: x\n---\ngraph TD\nA-->B") { Issue.record("front matter should be skipped") }
        if case .success = MermaidDiagram.parse("gantt\ntitle x") { Issue.record("gantt isn't drawn") }
    }
}
