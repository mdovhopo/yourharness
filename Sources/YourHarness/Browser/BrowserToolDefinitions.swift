import HarnessCore

extension BrowserTools {
    private static let tab: JSONValue = ["type": "integer", "description": "Which tab, from status. Defaults to the current one."]
    private static let ref: JSONValue = ["type": "string", "description": "An element's ref from the latest snapshot, e.g. e12."]
    private static let selector: JSONValue = ["type": "string", "description": "A CSS selector, instead of a ref."]
    private static let label: JSONValue = ["type": "string", "description": "Visible text of a button or link to find, instead of a ref."]
    private static let element: [String: JSONValue] = ["ref": ref, "selector": selector, "label": label, "tab": tab]
    private static let waitUntil: JSONValue = ["type": "string", "enum": ["load", "none"], "description": "Wait for the page to load (default), or return right away."]

    private static func with(_ base: [String: JSONValue], _ extra: [String: JSONValue]) -> [String: JSONValue] {
        base.merging(extra) { _, new in new }
    }

    static let definitions: [MCPTool] = [
        MCPTool(
            name: "status",
            description: "The browser's tabs (URL, title, loading), the viewport, whether a dialog is waiting, and whether the user has taken control.",
            inputSchema: MCPTool.schema([:]), readOnly: true),
        MCPTool(
            name: "open",
            description: "Open a URL in the browser built into the app, which the user watches. Loads in the current tab unless new_tab is true. Use it for the app you're building: start its dev server in the background first, then open http://localhost:<port>.",
            inputSchema: MCPTool.schema([
                "url": ["type": "string", "description": "A URL, a host, or localhost:3000."],
                "new_tab": ["type": "boolean", "description": "Open another tab instead of using the current one."],
                "show": ["type": "boolean", "description": "Show the browser to the user (default true)."],
                "wait_until": waitUntil,
            ], required: ["url"])),
        MCPTool(
            name: "navigate",
            description: "Go to a URL in the current tab, or back, forward, or reload.",
            inputSchema: MCPTool.schema([
                "url": ["type": "string"],
                "action": ["type": "string", "enum": ["back", "forward", "reload"]],
                "wait_until": waitUntil, "tab": tab,
            ])),
        MCPTool(
            name: "snapshot",
            description: "Read the page as an outline: landmarks, headings, text, and interactive elements with refs ([ref=e12]) to use in click, type and the rest. Take one before acting, and again after the page changes. Password values are hidden.",
            inputSchema: MCPTool.schema([
                "selector": ["type": "string", "description": "Only this part of the page, for a big page."],
                "tab": tab,
            ]), readOnly: true),
        MCPTool(
            name: "screenshot",
            description: "See the page: the visible viewport, the whole page, or one element. Use it to check layout, styling and images, which the outline can't show.",
            inputSchema: MCPTool.schema(with(element, ["full_page": ["type": "boolean", "description": "The whole scrollable page."]])), readOnly: true),
        MCPTool(
            name: "click",
            description: "Click an element with the real mouse: by ref, selector or label, or at x,y in CSS pixels. Waits for a navigation it starts.",
            inputSchema: MCPTool.schema(with(element, [
                "x": ["type": "number"], "y": ["type": "number"],
                "button": ["type": "string", "enum": ["left", "right"]],
                "count": ["type": "integer", "description": "2 for a double click."],
                "modifiers": ["type": "array", "items": ["type": "string", "enum": ["Meta", "Shift", "Alt", "Control"]]],
                "force": ["type": "boolean", "description": "Click with a script even if something covers it."],
            ]))),
        MCPTool(
            name: "type",
            description: "Type text into a field (by ref, selector or label), or into the focused element. Replaces what's there unless clear is false.",
            inputSchema: MCPTool.schema(with(element, [
                "text": ["type": "string"],
                "clear": ["type": "boolean", "description": "Replace the current value (default true)."],
                "submit": ["type": "boolean", "description": "Press Enter after."],
            ]), required: ["text"])),
        MCPTool(
            name: "press",
            description: "Press a key, optionally after focusing an element: Enter, Tab, Escape, Backspace, Delete, ArrowUp/Down/Left/Right, Home, End, PageUp, PageDown, a single character, or combinations like Shift+Tab or Meta+A.",
            inputSchema: MCPTool.schema(with(element, [
                "key": ["type": "string"],
                "count": ["type": "integer", "description": "Press it this many times."],
            ]), required: ["key"])),
        MCPTool(
            name: "hover",
            description: "Move the mouse over an element, for hover menus and tooltips.",
            inputSchema: MCPTool.schema(element)),
        MCPTool(
            name: "select",
            description: "Pick options in a native <select> by value or label. For custom dropdowns, click instead.",
            inputSchema: MCPTool.schema(with(element, ["values": ["type": "array", "items": ["type": "string"]]]), required: ["values"])),
        MCPTool(
            name: "scroll",
            description: "Scroll the page or the scrollable area around an element: a direction and amount in pixels, to top or bottom, or just an element into view.",
            inputSchema: MCPTool.schema(with(element, [
                "direction": ["type": "string", "enum": ["up", "down", "left", "right"]],
                "amount": ["type": "number"],
                "to": ["type": "string", "enum": ["top", "bottom"]],
            ]))),
        MCPTool(
            name: "drag",
            description: "Drag from one element to another with the mouse.",
            inputSchema: MCPTool.schema([
                "from": ["type": "object", "description": "{ref} or {selector}", "properties": ["ref": ref, "selector": selector]],
                "to": ["type": "object", "description": "{ref} or {selector}", "properties": ["ref": ref, "selector": selector]],
                "tab": tab,
            ], required: ["from", "to"])),
        MCPTool(
            name: "wait_for",
            description: "Wait until text appears, an element matching a selector is visible, text is gone, or the URL contains something. Fails at the timeout.",
            inputSchema: MCPTool.schema([
                "text": ["type": "string"], "selector": ["type": "string"], "gone": ["type": "string", "description": "Text that should disappear."],
                "url": ["type": "string", "description": "Part of the URL to wait for."],
                "timeout": ["type": "number", "description": "Seconds, default 10, at most 60."],
                "tab": tab,
            ]), readOnly: true),
        MCPTool(
            name: "console",
            description: "Console messages and uncaught errors from the page, new since you last looked unless all is true.",
            inputSchema: MCPTool.schema([
                "level": ["type": "string", "enum": ["all", "warning", "error"]],
                "all": ["type": "boolean"], "limit": ["type": "integer"], "tab": tab,
            ]), readOnly: true),
        MCPTool(
            name: "network",
            description: "Requests the page made: method, URL, status, type and time, new since you last looked unless all is true.",
            inputSchema: MCPTool.schema([
                "failed_only": ["type": "boolean"], "filter": ["type": "string", "description": "Only URLs containing this."],
                "all": ["type": "boolean"], "limit": ["type": "integer"], "tab": tab,
            ]), readOnly: true),
        MCPTool(
            name: "evaluate",
            description: "Run JavaScript in the page and get the result as JSON. An expression, or a function body with return; await works.",
            inputSchema: MCPTool.schema(["expression": ["type": "string"], "tab": tab], required: ["expression"])),
        MCPTool(
            name: "resize",
            description: "Change the viewport: a preset (desktop 1280×800, tablet 820×1180, phone 390×844, fit = the panel's size) or width and height, and the color scheme the page sees.",
            inputSchema: MCPTool.schema([
                "preset": ["type": "string", "enum": ["desktop", "tablet", "phone", "fit"]],
                "width": ["type": "number"], "height": ["type": "number"],
                "color_scheme": ["type": "string", "enum": ["light", "dark", "auto"]],
                "tab": tab,
            ])),
        MCPTool(
            name: "dialog",
            description: "Answer the alert, confirm or prompt the page is waiting on.",
            inputSchema: MCPTool.schema([
                "accept": ["type": "boolean", "description": "OK (default) or Cancel."],
                "text": ["type": "string", "description": "For a prompt."],
                "tab": tab,
            ])),
        MCPTool(
            name: "upload",
            description: "Put files in a file input: give the input (or the button that opens the file chooser) and absolute paths.",
            inputSchema: MCPTool.schema(with(element, ["paths": ["type": "array", "items": ["type": "string"]]]), required: ["paths"])),
        MCPTool(
            name: "close",
            description: "Close a tab.",
            inputSchema: MCPTool.schema(["tab": tab])),
    ]
}
