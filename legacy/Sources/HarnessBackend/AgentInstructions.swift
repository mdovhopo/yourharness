import Foundation

/// What we tell agents about this app, appended to Claude Code's system prompt. Worded as the
/// frozen app's (`legacy/YourHarness/Model/AgentInstructions.swift`): agents already act on it.
enum AgentInstructions {
    /// The thread tools. `project` names the one project a thread outside Home reaches.
    static func threads(project: String?, updatesTitle: Bool) -> String {
        let reach = project.map {
            "From this thread you reach only the threads of the project \($0), and only Claude Code sessions that ran in "
                + "its folder; for anything wider, the user can ask from a thread in Home."
        } ?? "This thread is in Home, so you reach every project's threads and every Claude Code session."
        return """
        The user's conversations in YourHarness are threads, grouped by project in the sidebar. The mcp__threads__ \
        tools work with them: list_threads, read_thread and search_threads find and read them; rename_thread, \
        settle_thread and pin_thread tidy them; list_claude_code_sessions and import_claude_code_session bring in \
        sessions from Claude Code run in a terminal or an editor. \(reach)
        Reading and searching are fine whenever they help, like picking up what an earlier thread worked out. \
        Changing threads is the user's call: rename, settle, pin or import only what the user asked for, or a list \
        they agreed to. Before changing more than two threads, list exactly what you'll change and wait for a yes. \
        Leave threads that are working or waiting for the user alone unless the user named them. You can't delete \
        threads; if asked, tell the user to do it from the sidebar. Other threads can hold private details or \
        secrets: bring into this conversation only what the task needs. When you're done, say what you changed.
        """ + (updatesTitle ? "\n\n" + threadTitle : "")
    }

    /// With title updates on: when to have this thread's own title rewritten.
    static let threadTitle = """
    Keeping this thread's title right is part of your job; a separate title model writes it, you tell it when to \
    look again. When the user's latest message turns the thread to a different subject than it has been about (a \
    new, unrelated task or question), or a vague start ("fix this", a bare link) has turned out to be about \
    something specific, call mcp__threads__update_title before you answer, with a few words on the new subject as \
    the hint. Not for progress on the same subject (exploring, planning, implementing, reviewing, fixing CI, \
    merging), and once per change at most. Don't use rename_thread on this thread unless the user asks for a \
    particular name, and don't mention the title to the user.
    """
}
