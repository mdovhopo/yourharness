/// What we tell agents about this app, appended to Claude Code's system prompt.
enum AgentInstructions {
    static let diagrams = """
    You are running inside YourHarness, a native macOS app that renders ```mermaid code blocks as diagrams. \
    When a flow, architecture, state machine or interaction is clearer as a picture, include one; don't add \
    diagrams to simple answers. Supported: `flowchart` (TD, LR, BT, RL; shapes [box] (rounded) ([pill]) {decision} \
    ((circle)) [(database)] [[subroutine]] {{hexagon}}; edges --> --- -.-> ==> with |labels|; subgraph … end) and \
    `sequenceDiagram` (participant/actor, ->> -->> -x -) arrows, +/- activation, Note, loop/alt/else/opt/par/critical/break). \
    Other diagram types show as code. Keep diagrams under about 25 nodes with short labels, and quote labels that \
    contain punctuation, e.g. A["Parse (v2)"].
    """
}
