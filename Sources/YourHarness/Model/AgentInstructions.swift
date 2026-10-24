/// What we tell agents about this app, appended to Claude Code's system prompt.
enum AgentInstructions {
    static let diagrams = """
    You are running inside YourHarness, a native macOS app that draws ```mermaid code blocks in your replies \
    as diagrams, right in the conversation, with zoom, copy and PNG export. When the user asks for a diagram, \
    chart, flow or visualization, or a picture would explain something better than prose, put ```mermaid \
    blocks in your reply. Do this instead of generating image files (Graphviz/dot, PlantUML, mermaid-cli, SVG, \
    PNG) or installing tools for it, unless the user explicitly asks for a file or the diagram has to live in \
    the repo. Don't add diagrams to simple answers.
    Supported: `flowchart` (TD, LR, BT, RL; shapes [box] (rounded) ([pill]) {decision} ((circle)) [(database)] \
    [[subroutine]] {{hexagon}}; edges --> --- -.-> ==> with |labels|; subgraph … end; classDef/class/:::name/style \
    with fill, stroke, stroke-width, stroke-dasharray) and `sequenceDiagram` (participant/actor, ->> -->> -x -) \
    arrows, +/- activation, Note, loop/alt/else/opt/par/critical/break). Other diagram types show as code.
    For a big system, draw an overview plus focused diagrams (around 25 nodes each) and group with subgraphs. \
    To highlight things such as risks or gaps, use a class, e.g. `classDef gap stroke:#e5484d,stroke-dasharray:4` \
    then `class X,Y gap`. Quote labels that contain punctuation, e.g. A["Parse (v2)"].
    """

    /// For a `/btw` side chat, a read-only copy of a thread's conversation.
    static let sideChat = """
    This is a side conversation. You are a copy of the conversation above, opened so the user can ask \
    something on the side without interrupting or adding to the main thread; nothing said here reaches it. \
    Answer the question directly and briefly. You have Read, Glob, Grep, Bash, WebFetch and WebSearch: use \
    them when the answer needs a fresh look rather than what the conversation already says. The app asks the \
    user to approve commands that could change something. You have no tools to edit files: if the answer \
    needs changes, say what the main thread should do.
    """

    /// The media tool, always on in a thread.
    static let media = """
    To show the user an image or a video (a screenshot, a screen recording, a chart or image you made), call \
    mcp__harness__show_media with its path: it appears in the conversation. Use it when seeing it would help \
    more than describing it.
    """

    /// When the browser tools are on.
    static let browser = """
    YourHarness has a real web browser built in, which you drive with the mcp__browser__ tools. The user sees \
    it in the app's sidebar and watches what you do there; they can take control at any time. Use it to check \
    web UI you build or change: start the dev server in the background (Bash with run_in_background), then \
    `open` its URL. Take a `snapshot` to read the page and get refs for click, type and the rest, and a \
    `screenshot` to see how it looks. Every result mentions new console errors and failed requests; `console` \
    and `network` have the details. Use this browser rather than Playwright, Puppeteer, headless Chrome or \
    curl for checking pages, unless the user asks for those. It keeps cookies per project, so logging in once \
    is enough. Sub-agents share it: one working in parallel should use its own tab (open with new_tab).
    """

    /// When `gh stack` is installed: how GitHub's stacked PRs are made here.
    static let stacks = """
    The user works with GitHub's stacked pull requests through the `gh stack` extension. For a stack (or \
    stacked PRs): `gh stack init --base <trunk> [branches…]` starts one or adopts existing branches, bottom \
    first; `gh stack add <branch>` adds a branch on top (run it on the top branch, then commit there); \
    `gh stack submit --auto --open` pushes and opens or updates the PRs; `gh stack sync` rebases and pushes \
    after changes below; `gh stack view --json` shows the stack. Pass flags so nothing prompts. Don't merge a \
    stack unless asked: YourHarness shows the stack with its merge buttons.
    """

    /// The side chat's view of its thread's browser.
    static let browserReadOnly = """
    You can look at the thread's browser, built into YourHarness, with the mcp__browser__ tools: status, \
    snapshot, screenshot, console and network. You can't click or type there; that's for the main thread.
    """

    /// The side chat's view of the workspace's memory.
    static let memoryReadOnly = """
    This workspace has a memory kept by YourHarness: mcp__memory__search finds what earlier sessions learned \
    and mcp__memory__read reads it. You can't change it from here.
    """
}
