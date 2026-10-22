/// A colored span of code. Text between tokens is plain.
public struct SyntaxToken: Equatable, Sendable {
    public enum Kind: Equatable, Sendable, CaseIterable {
        case keyword
        case type
        case string
        case comment
        case number
        /// A function or method name, where it's called or declared. Rust macros too.
        case function
        /// Attributes, decorators, annotations and preprocessor directives.
        case attribute
        /// Keys: in JSON, YAML and TOML, and CSS properties.
        case property
        /// Shell and Ruby variables, YAML anchors.
        case variable
        /// A diff's added line.
        case inserted
        /// A diff's removed line.
        case deleted
        /// A diff's file headers and hunks, a YAML document marker.
        case meta
    }

    public var kind: Kind
    public var range: Range<String.Index>

    public init(kind: Kind, range: Range<String.Index>) {
        self.kind = kind
        self.range = range
    }
}

/// Colors code by language, in one pass over its bytes. Fast enough to run on every frame
/// of a streaming reply, and safe on a prefix: an unterminated string or comment runs to
/// the end of its line, or of the text when it may span lines.
public enum SyntaxHighlighter {
    /// The text's tokens, in order and not overlapping. None for a language it doesn't know.
    public static func tokens(in text: String, language: String) -> [SyntaxToken] {
        guard let syntax = Syntax(language: language), !text.isEmpty else { return [] }
        let spans = text.utf8.withContiguousStorageIfAvailable { spans(in: $0, syntax) }
            ?? ContiguousArray(text.utf8).withUnsafeBufferPointer { spans(in: $0, syntax) }

        // Spans start and end on ASCII bytes, so these are scalar boundaries. Walking the
        // offsets in order keeps this linear for bridged strings too.
        var tokens: [SyntaxToken] = []
        tokens.reserveCapacity(spans.count)
        let utf8 = text.utf8
        var index = utf8.startIndex
        var offset = 0
        for span in spans {
            let start = utf8.index(index, offsetBy: span.start - offset)
            let end = utf8.index(start, offsetBy: span.end - span.start)
            tokens.append(SyntaxToken(kind: span.kind, range: start..<end))
            index = end
            offset = span.end
        }
        return tokens
    }

    /// Whether `language` gets any highlighting.
    public static func highlights(_ language: String) -> Bool {
        Syntax(language: language) != nil
    }

    private static func spans(in bytes: UnsafeBufferPointer<UInt8>, _ syntax: Syntax) -> [Span] {
        var out = Spans(bytes)
        switch syntax {
        case .code(let grammar): out.code(grammar, 0, bytes.count)
        case .markup: out.markup(0, bytes.count)
        case .css(let lineComments): out.css(lineComments: lineComments, 0, bytes.count)
        case .yaml: out.yaml(0, bytes.count)
        case .diff: out.diff(0, bytes.count)
        }
        return out.spans
    }
}

// MARK: - Languages

private enum Syntax {
    case code(Grammar)
    case markup
    case css(lineComments: Bool)
    case yaml
    case diff

    init?(language: String) {
        // Fences can carry more than the name: "ts title=app.ts", "swift {1,3}".
        let name = language.prefix { $0 != " " && $0 != "{" && $0 != "," }.lowercased()
        switch name {
        case "swift": self = .code(.swift)
        case "typescript", "ts", "tsx", "mts", "cts": self = .code(.typescript)
        case "javascript", "js", "jsx", "mjs", "cjs", "node": self = .code(.javascript)
        case "python", "py", "python3", "py3", "pyi": self = .code(.python)
        case "go", "golang": self = .code(.go)
        case "rust", "rs": self = .code(.rust)
        case "java": self = .code(.java)
        case "kotlin", "kt", "kts": self = .code(.kotlin)
        case "c", "h", "cpp", "c++", "cc", "cxx", "hpp", "hh", "objc", "objective-c", "objectivec", "objc++", "m", "mm":
            self = .code(.c)
        case "csharp", "cs", "c#": self = .code(.csharp)
        case "ruby", "rb": self = .code(.ruby)
        case "lua": self = .code(.lua)
        case "json", "jsonc", "json5", "jsonl", "ndjson", "geojson": self = .code(.json)
        case "toml": self = .code(.toml)
        case "ini", "cfg", "conf", "env", "dotenv", "properties", "editorconfig", "gitconfig": self = .code(.ini)
        case "bash", "sh", "zsh", "shell", "console", "shellsession", "fish", "ksh", "terminal": self = .code(.shell)
        case "sql", "postgres", "postgresql", "psql", "mysql", "sqlite", "plsql": self = .code(.sql)
        case "dockerfile", "docker", "containerfile": self = .code(.dockerfile)
        case "html", "htm", "xhtml", "xml", "svg", "plist", "vue", "svelte", "xaml", "rss", "atom": self = .markup
        case "css": self = .css(lineComments: false)
        case "scss", "less": self = .css(lineComments: true)
        case "yaml", "yml": self = .yaml
        case "diff", "patch", "udiff": self = .diff
        default: return nil  // Markdown, plain text and anything unknown stay plain.
        }
    }
}

/// How a C-like, hash-commented or similar language reads.
private struct Grammar: Sendable {
    var keywords: Set<String> = []
    var types: Set<String> = []
    /// Commands and built-in functions, colored as functions without a call.
    var builtins: Set<String> = []
    /// Keywords after which the next name is a function being declared.
    var declarations: Set<String> = []
    var lineComments: [[UInt8]] = []
    var blockComments: [(open: [UInt8], close: [UInt8])] = []
    var nestedComments = false
    /// `#` starts a comment only at the start of a word (shell: `${#array}`, `a#b`).
    var commentsAfterSpace = false
    /// Longest openers first: `"""` before `"`.
    var quotes: [Quote] = []
    /// `String`, `URLSession`: capitalized names are types (constants like `MAX_SIZE` aren't).
    var capitalizedTypes = false
    var caseInsensitive = false
    /// A name right before `(` is a function.
    var functionCalls = true
    /// `@MainActor`, `@decorator`, `@Override`.
    var attributes = false
    /// `#include` (at the start of a line), `#if`/`#available` (anywhere).
    var directives: Directives = .none
    /// `$name` (shell), `@name`/`$name` (Ruby).
    var variableSigils: [UInt8] = []
    /// Bytes that also make up names: `$` in JavaScript, `-` in TOML keys.
    var nameExtras: [UInt8] = []
    var regexLiterals = false
    /// Macros (`println!`), lifetimes (`'a`), attributes (`#[derive]`) and raw strings (`r#"…"#`).
    var rust = false
    /// `#"…"#`.
    var swiftRawStrings = false
    var rubySymbols = false
    /// `--flag`, `-x` after a space.
    var shellFlags = false
    /// A string before `:` is a key (JSON).
    var keysBeforeColon = false
    /// A name before `=` is a key (TOML, INI).
    var keysBeforeEquals = false
    /// `[section]` at the start of a line.
    var sectionHeaders = false

    enum Directives { case none, lineStart, anywhere }
}

private struct Quote: Sendable {
    var open: [UInt8]
    var close: [UInt8]
    var escapes = true
    /// Whether it can span lines. A single-line one ends at the line's end if unterminated.
    var multiline = false
    var interpolations: [Interpolation] = []

    init(_ open: String, _ close: String? = nil, escapes: Bool = true, multiline: Bool = false, interpolations: [Interpolation] = []) {
        self.open = Array(open.utf8)
        self.close = Array((close ?? open).utf8)
        self.escapes = escapes
        self.multiline = multiline
        self.interpolations = interpolations
    }
}

/// Code inside a string (`\(…)`, `${…}`): skipped with its brackets balanced, so quotes in it
/// don't end the string.
private struct Interpolation: Sendable {
    var open: [UInt8]
    var nest: UInt8
    var close: UInt8

    static let swift = Interpolation(open: Array(#"\("#.utf8), nest: .openParen, close: .closeParen)
    static let dollarBrace = Interpolation(open: Array("${".utf8), nest: .openBrace, close: .closeBrace)
    static let dollarParen = Interpolation(open: Array("$(".utf8), nest: .openParen, close: .closeParen)
    static let hashBrace = Interpolation(open: Array("#{".utf8), nest: .openBrace, close: .closeBrace)
}

private func words(_ list: String) -> Set<String> {
    Set(list.split(separator: " ").map(String.init))
}

private extension Grammar {
    static let cComments: [[UInt8]] = [Array("//".utf8)]
    static let cBlock: [(open: [UInt8], close: [UInt8])] = [(Array("/*".utf8), Array("*/".utf8))]
    static let hash: [[UInt8]] = [Array("#".utf8)]

    static let swift = Grammar(
        keywords: words("""
        associatedtype class deinit enum extension fileprivate func import init inout internal let open operator \
        private precedencegroup protocol public rethrows static struct subscript typealias var break case catch \
        continue default defer do else fallthrough for guard if in repeat return throw switch where while as Any \
        false is nil self Self super throws true try await async actor some any nonisolated isolated consuming \
        borrowing mutating nonmutating override final required convenience lazy weak unowned dynamic indirect \
        optional willSet didSet macro package sending each discard
        """),
        declarations: ["func"],
        lineComments: cComments, blockComments: cBlock, nestedComments: true,
        quotes: [
            Quote(#"""""#, multiline: true, interpolations: [.swift]),
            Quote(#"""#, interpolations: [.swift]),
        ],
        capitalizedTypes: true, attributes: true, directives: .anywhere, swiftRawStrings: true
    )

    static let javascript = Grammar(
        keywords: words("""
        break case catch class const continue debugger default delete do else export extends finally for function \
        if import in instanceof new return super switch this throw try typeof var void while with yield let static \
        async await of true false null undefined NaN Infinity from as
        """),
        declarations: ["function"],
        lineComments: cComments, blockComments: cBlock,
        quotes: [
            Quote("`", multiline: true, interpolations: [.dollarBrace]),
            Quote("\""),
            Quote("'"),
        ],
        capitalizedTypes: true, attributes: true, nameExtras: [.dollar], regexLiterals: true
    )

    static let typescript: Grammar = {
        var grammar = javascript
        grammar.keywords.formUnion(words("""
        interface type enum implements namespace module declare abstract private protected public readonly keyof \
        infer is satisfies override
        """))
        grammar.types = words("string number boolean any unknown never object symbol bigint")
        return grammar
    }()

    static let python = Grammar(
        keywords: words("""
        False None True and as assert async await break class continue def del elif else except finally for from \
        global if import in is lambda nonlocal not or pass raise return try while with yield match case self cls
        """),
        types: words("int float str bool list dict set tuple bytes bytearray object type complex frozenset"),
        declarations: ["def"],
        lineComments: hash,
        quotes: [
            Quote(#"""""#, multiline: true),
            Quote("'''", multiline: true),
            Quote("\""),
            Quote("'"),
        ],
        capitalizedTypes: true, attributes: true
    )

    static let go = Grammar(
        keywords: words("""
        break case chan const continue default defer else fallthrough for func go goto if import interface map \
        package range return select struct switch type var true false nil iota
        """),
        types: words("""
        bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 \
        uint16 uint32 uint64 uintptr any comparable
        """),
        declarations: ["func"],
        lineComments: cComments, blockComments: cBlock,
        quotes: [Quote("`", escapes: false, multiline: true), Quote("\""), Quote("'")]
    )

    static let rust = Grammar(
        keywords: words("""
        as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod \
        move mut pub ref return self Self static struct super trait true type unsafe use where while union
        """),
        types: words("i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str"),
        declarations: ["fn"],
        lineComments: cComments, blockComments: cBlock, nestedComments: true,
        quotes: [Quote("\"", multiline: true)],
        capitalizedTypes: true, rust: true
    )

    static let java = Grammar(
        keywords: words("""
        abstract assert break case catch class const continue default do else enum extends final finally for goto \
        if implements import instanceof interface native new package private protected public return static \
        strictfp super switch synchronized this throw throws transient try void volatile while var record sealed \
        permits yield true false null
        """),
        types: words("boolean byte char double float int long short"),
        lineComments: cComments, blockComments: cBlock,
        quotes: [Quote(#"""""#, multiline: true), Quote("\""), Quote("'")],
        capitalizedTypes: true, attributes: true
    )

    static let kotlin = Grammar(
        keywords: words("""
        as break class continue do else false for fun if in interface is null object package return super this \
        throw true try typealias val var when while by catch constructor finally import init where abstract actual \
        annotation companion const crossinline data enum expect external final infix inline inner internal lateinit \
        noinline open operator out override private protected public reified sealed suspend tailrec vararg
        """),
        declarations: ["fun"],
        lineComments: cComments, blockComments: cBlock, nestedComments: true,
        quotes: [
            Quote(#"""""#, escapes: false, multiline: true, interpolations: [.dollarBrace]),
            Quote("\"", interpolations: [.dollarBrace]),
            Quote("'"),
        ],
        capitalizedTypes: true, attributes: true
    )

    /// C, C++ and Objective-C, which share most of their words.
    static let c = Grammar(
        keywords: words("""
        auto break case const continue default do else enum extern for goto if inline register restrict return \
        sizeof static struct switch typedef union volatile while NULL true false alignas alignof asm catch class \
        constexpr consteval constinit const_cast co_await co_return co_yield decltype delete dynamic_cast explicit \
        export friend mutable namespace new noexcept nullptr operator private protected public reinterpret_cast \
        static_assert static_cast template this thread_local throw try typeid typename using virtual override \
        final concept requires self super nil Nil YES NO
        """),
        types: words("""
        bool char double float int long short signed unsigned void wchar_t char8_t char16_t char32_t size_t \
        ssize_t ptrdiff_t int8_t int16_t int32_t int64_t uint8_t uint16_t uint32_t uint64_t intptr_t uintptr_t id \
        instancetype SEL IMP
        """),
        lineComments: cComments, blockComments: cBlock,
        quotes: [Quote("\""), Quote("'")],
        capitalizedTypes: true, attributes: true, directives: .lineStart
    )

    static let csharp = Grammar(
        keywords: words("""
        abstract as base break case catch checked class const continue default delegate do else enum event \
        explicit extern false finally fixed for foreach goto if implicit in interface internal is lock namespace \
        new null operator out override params private protected public readonly ref return sealed sizeof \
        stackalloc static struct switch this throw true try typeof unchecked unsafe using virtual volatile while \
        var async await record yield when
        """),
        types: words("bool byte char decimal double float int long object sbyte short string uint ulong ushort void dynamic"),
        lineComments: cComments, blockComments: cBlock,
        quotes: [Quote(#"""""#, multiline: true), Quote("\""), Quote("'")],
        capitalizedTypes: true, directives: .lineStart
    )

    static let ruby = Grammar(
        keywords: words("""
        BEGIN END alias and begin break case class def defined do else elsif end ensure false for if in module next \
        nil not or redo rescue retry return self super then true undef unless until when while yield require \
        require_relative include extend attr_accessor attr_reader attr_writer private protected public raise lambda \
        proc loop
        """),
        declarations: ["def"],
        lineComments: hash,
        quotes: [
            Quote("\"", multiline: true, interpolations: [.hashBrace]),
            Quote("'", multiline: true),
            Quote("`", multiline: true, interpolations: [.hashBrace]),
        ],
        capitalizedTypes: true, variableSigils: [.at, .dollar], rubySymbols: true
    )

    static let lua = Grammar(
        keywords: words("""
        and break do else elseif end false for function goto if in local nil not or repeat return then true until \
        while self
        """),
        declarations: ["function"],
        lineComments: [Array("--".utf8)],
        blockComments: [(Array("--[[".utf8), Array("]]".utf8))],
        quotes: [Quote("[[", "]]", escapes: false, multiline: true), Quote("\""), Quote("'")]
    )

    static let json = Grammar(
        keywords: ["true", "false", "null"],
        lineComments: cComments, blockComments: cBlock,
        quotes: [Quote("\""), Quote("'")],
        functionCalls: false, keysBeforeColon: true
    )

    static let toml = Grammar(
        keywords: ["true", "false", "inf", "nan"],
        lineComments: hash,
        quotes: [
            Quote(#"""""#, multiline: true),
            Quote("'''", escapes: false, multiline: true),
            Quote("\""),
            Quote("'", escapes: false),
        ],
        functionCalls: false, nameExtras: [.dash], keysBeforeEquals: true, sectionHeaders: true
    )

    static let ini = Grammar(
        keywords: ["true", "false", "yes", "no", "on", "off"],
        lineComments: [Array("#".utf8), Array(";".utf8)], commentsAfterSpace: true,
        quotes: [Quote("\""), Quote("'", escapes: false)],
        functionCalls: false, nameExtras: [.dash], keysBeforeEquals: true, sectionHeaders: true
    )

    static let shell = Grammar(
        keywords: words("""
        if then else elif fi case esac for select while until do done in function time coproc return exit break \
        continue local export readonly declare typeset unset shift source alias eval exec trap set
        """),
        builtins: words("echo printf cd pwd read test pushd popd exit sudo"),
        lineComments: hash, commentsAfterSpace: true,
        quotes: [
            Quote("\"", multiline: true, interpolations: [.dollarParen, .dollarBrace]),
            Quote("'", escapes: false, multiline: true),
        ],
        variableSigils: [.dollar], shellFlags: true
    )

    static let sql = Grammar(
        keywords: words("""
        select from where and or not insert into values update set delete create table drop alter add column index \
        view primary key foreign references join inner left right outer full cross on as group by order having \
        limit offset distinct union all exists in is null like ilike between case when then else end begin commit \
        rollback transaction if returning with default unique check constraint cascade asc desc true false grant \
        revoke database schema trigger function procedure replace temporary temp explain analyze using natural \
        over partition window lateral recursive materialized do nothing conflict
        """),
        types: words("""
        int integer bigint smallint tinyint serial bigserial decimal numeric real float double precision char \
        varchar text boolean bool date time timestamp timestamptz interval uuid json jsonb blob bytea money
        """),
        lineComments: [Array("--".utf8)], blockComments: cBlock,
        quotes: [Quote("'", escapes: false, multiline: true), Quote("\"", escapes: false), Quote("`", escapes: false)],
        caseInsensitive: true
    )

    static let dockerfile = Grammar(
        keywords: words("""
        FROM RUN CMD LABEL MAINTAINER EXPOSE ENV ADD COPY ENTRYPOINT VOLUME USER WORKDIR ARG ONBUILD STOPSIGNAL \
        HEALTHCHECK SHELL AS
        """),
        lineComments: hash, commentsAfterSpace: true,
        quotes: [Quote("\""), Quote("'", escapes: false)],
        functionCalls: false, variableSigils: [.dollar], shellFlags: true
    )
}

// MARK: - Scanning

private enum Delimiter {
    static let blockOpen = Array("/*".utf8)
    static let blockClose = Array("*/".utf8)
    static let tripleQuote = Array(#"""""#.utf8)
    static let commentOpen = Array("<!--".utf8)
    static let commentClose = Array("-->".utf8)
    static let cdataOpen = Array("<![CDATA[".utf8)
    static let cdataClose = Array("]]>".utf8)
    static let scriptClose = Array("</script".utf8)
    static let styleClose = Array("</style".utf8)
    static let documentStart = Array("---".utf8)
    static let documentEnd = Array("...".utf8)
    static let newFile = Array("+++".utf8)
    static let hunk = Array("@@".utf8)
    static let diffHeader = Array("diff ".utf8)
    static let indexHeader = Array("index ".utf8)
}

private extension Quote {
    static let double = Quote("\"")
    static let single = Quote("'")
    /// YAML's single quotes escape by doubling, which closing and reopening already handles.
    static let yamlSingle = Quote("'", escapes: false)
}

private struct Span {
    var kind: SyntaxToken.Kind
    var start: Int
    var end: Int
}

private extension UInt8 {
    static let newline = UInt8(ascii: "\n")
    static let ret = UInt8(ascii: "\r")
    static let space = UInt8(ascii: " ")
    static let tab = UInt8(ascii: "\t")
    static let backslash = UInt8(ascii: "\\")
    static let slash = UInt8(ascii: "/")
    static let star = UInt8(ascii: "*")
    static let hash = UInt8(ascii: "#")
    static let at = UInt8(ascii: "@")
    static let dollar = UInt8(ascii: "$")
    static let dash = UInt8(ascii: "-")
    static let plus = UInt8(ascii: "+")
    static let dot = UInt8(ascii: ".")
    static let comma = UInt8(ascii: ",")
    static let colon = UInt8(ascii: ":")
    static let semicolon = UInt8(ascii: ";")
    static let equals = UInt8(ascii: "=")
    static let bang = UInt8(ascii: "!")
    static let question = UInt8(ascii: "?")
    static let ampersand = UInt8(ascii: "&")
    static let pipe = UInt8(ascii: "|")
    static let less = UInt8(ascii: "<")
    static let greater = UInt8(ascii: ">")
    static let quote = UInt8(ascii: "\"")
    static let apostrophe = UInt8(ascii: "'")
    static let backtick = UInt8(ascii: "`")
    static let openParen = UInt8(ascii: "(")
    static let closeParen = UInt8(ascii: ")")
    static let openBracket = UInt8(ascii: "[")
    static let closeBracket = UInt8(ascii: "]")
    static let openBrace = UInt8(ascii: "{")
    static let closeBrace = UInt8(ascii: "}")
    static let underscore = UInt8(ascii: "_")
    static let percent = UInt8(ascii: "%")
    static let tilde = UInt8(ascii: "~")

    var isDigit: Bool { self &- 48 < 10 }
    var isLetter: Bool { (self | 0x20) &- 97 < 26 }
    var isUppercase: Bool { self &- 65 < 26 }
    var isLowercase: Bool { self &- 97 < 26 }
    var isHexDigit: Bool { isDigit || (self | 0x20) &- 97 < 6 }
    /// Non-ASCII bytes count as letters, so names in any script stay whole.
    var isNameStart: Bool { isLetter || self == .underscore || self >= 0x80 }
    var isNameByte: Bool { isNameStart || isDigit }
    var isBlank: Bool { self == .space || self == .tab || self == .ret }
    var isWhitespace: Bool { isBlank || self == .newline }
}

private struct Spans {
    let s: UnsafeBufferPointer<UInt8>
    var spans: [Span] = []

    static let valueKeywords: Set<String> = ["this", "self", "super", "true", "false", "null", "nil", "undefined"]
    static let yamlLiterals = ["true", "false", "null", "yes", "no", "on", "off"]

    init(_ bytes: UnsafeBufferPointer<UInt8>) {
        s = bytes
        spans.reserveCapacity(bytes.count / 8)
    }

    mutating func emit(_ kind: SyntaxToken.Kind, _ start: Int, _ end: Int) {
        guard end > start else { return }
        if let last = spans.last, last.kind == kind, last.end == start {
            spans[spans.count - 1].end = end
        } else {
            spans.append(Span(kind: kind, start: start, end: end))
        }
    }

    // MARK: Helpers

    func has(_ prefix: [UInt8], at i: Int, _ hi: Int) -> Bool {
        guard i + prefix.count <= hi else { return false }
        for k in 0..<prefix.count where s[i + k] != prefix[k] { return false }
        return true
    }

    func endOfLine(_ i: Int, _ hi: Int) -> Int {
        var j = i
        while j < hi, s[j] != .newline { j += 1 }
        return j
    }

    func skipBlanks(_ i: Int, _ hi: Int) -> Int {
        var j = i
        while j < hi, s[j].isBlank { j += 1 }
        return j
    }

    func name(_ i: Int, _ hi: Int, extras: [UInt8] = []) -> Int {
        var j = i
        while j < hi, s[j].isNameByte || extras.contains(s[j]) { j += 1 }
        return j
    }

    func word(_ start: Int, _ end: Int) -> String {
        String(decoding: UnsafeBufferPointer(rebasing: s[start..<end]), as: UTF8.self)
    }

    /// Whether `start..<end` is ASCII bytes equal to `lowercase`, ignoring case.
    func equalsIgnoringCase(_ start: Int, _ end: Int, _ lowercase: String) -> Bool {
        guard end - start == lowercase.utf8.count else { return false }
        var i = start
        for byte in lowercase.utf8 {
            if s[i] | 0x20 != byte { return false }
            i += 1
        }
        return true
    }

    /// A number from `i`: digits, a fraction, an exponent, a base prefix and a suffix (`1_000`,
    /// `0xFF`, `2.5e-3`, `10px`, `42u8`).
    func number(_ i: Int, _ hi: Int) -> Int {
        var j = i
        if s[j] == UInt8(ascii: "0"), j + 1 < hi, "xXbBoO".utf8.contains(s[j + 1]) {
            j += 2
            while j < hi, s[j].isHexDigit || s[j] == .underscore { j += 1 }
        } else {
            while j < hi, s[j].isDigit || s[j] == .underscore { j += 1 }
            if j + 1 < hi, s[j] == .dot, s[j + 1].isDigit {
                j += 1
                while j < hi, s[j].isDigit || s[j] == .underscore { j += 1 }
            }
            if j + 1 < hi, s[j] | 0x20 == UInt8(ascii: "e") {
                if s[j + 1].isDigit {
                    j += 1
                } else if j + 2 < hi, s[j + 1] == .plus || s[j + 1] == .dash, s[j + 2].isDigit {
                    j += 2
                }
            }
        }
        while j < hi, s[j].isNameByte { j += 1 }
        return j
    }

    /// The end of a string whose opener ends at `i`.
    func stringEnd(_ quote: Quote, from i: Int, _ hi: Int) -> Int {
        var j = i
        let first = quote.close[0]
        while j < hi {
            let c = s[j]
            if !quote.interpolations.isEmpty, let interpolation = quote.interpolations.first(where: { has($0.open, at: j, hi) }) {
                j = interpolationEnd(interpolation, from: j + interpolation.open.count, hi, multiline: quote.multiline)
                continue
            }
            if c == .backslash, quote.escapes {
                j += 2
                continue
            }
            if c == first, has(quote.close, at: j, hi) { return j + quote.close.count }
            if c == .newline, !quote.multiline { return j }
            j += 1
        }
        return hi
    }

    /// Skips the code in an interpolation, with its brackets balanced and its own strings.
    func interpolationEnd(_ interpolation: Interpolation, from i: Int, _ hi: Int, multiline: Bool) -> Int {
        var j = i
        var depth = 1
        while j < hi {
            let c = s[j]
            if c == interpolation.nest {
                depth += 1
            } else if c == interpolation.close {
                depth -= 1
                if depth == 0 { return j + 1 }
            } else if c == .quote || c == .apostrophe || c == .backtick {
                j += 1
                while j < hi, s[j] != c, s[j] != .newline {
                    j += s[j] == .backslash ? 2 : 1
                }
            } else if c == .newline, !multiline {
                return j
            }
            j += 1
        }
        return hi
    }

    /// The end of a raw string (`#"…"#`, `r##"…"##`) whose quote is at `i`.
    func rawStringEnd(quoteAt i: Int, hashes: Int, _ hi: Int) -> Int {
        let triple = has(Delimiter.tripleQuote, at: i, hi)
        let close = Array(repeating: UInt8.quote, count: triple ? 3 : 1) + Array(repeating: UInt8.hash, count: hashes)
        var j = i + (triple ? 3 : 1)
        while j < hi {
            if s[j] == .quote, has(close, at: j, hi) { return j + close.count }
            j += 1
        }
        return hi
    }

    func blockCommentEnd(open: [UInt8], close: [UInt8], from i: Int, nested: Bool, _ hi: Int) -> Int {
        var j = i + open.count
        var depth = 1
        while j < hi {
            if s[j] == close[0], has(close, at: j, hi) {
                depth -= 1
                j += close.count
                if depth == 0 || !nested { return j }
            } else if nested, s[j] == open[0], has(open, at: j, hi) {
                depth += 1
                j += open.count
            } else {
                j += 1
            }
        }
        return hi
    }

    // MARK: Code

    mutating func code(_ g: Grammar, _ lo: Int, _ hi: Int) {
        var i = lo
        var lineStart = true
        /// After `func`, `def`, `fn`: the next name is a function.
        var expectsName = false
        /// The last token could end an expression, so a `/` divides rather than starts a regex.
        var afterValue = false

        while i < hi {
            let b = s[i]
            if b == .newline {
                lineStart = true
                i += 1
                continue
            }
            if b.isBlank {
                i += 1
                continue
            }
            let atLineStart = lineStart
            lineStart = false

            // Comments.
            if let end = comment(g, at: i, lo, hi) {
                emit(.comment, i, end)
                i = end
                continue
            }

            // Strings.
            if let quote = g.quotes.first(where: { $0.open[0] == b && has($0.open, at: i, hi) }) {
                let end = stringEnd(quote, from: i + quote.open.count, hi)
                emit(isKey(g, after: end, hi) ? .property : .string, i, end)
                i = end
                afterValue = true
                expectsName = false
                continue
            }

            // Numbers, but not the digits in `x.0` or `v1`.
            let previous = i > lo ? s[i - 1] : .space
            if b.isDigit || (b == .dot && i + 1 < hi && s[i + 1].isDigit) {
                if previous.isNameByte || previous == .dot {
                    i = name(i + 1, hi)
                    continue
                }
                let end = number(b == .dot ? i + 1 : i, hi)
                emit(.number, i, end)
                i = end
                afterValue = true
                expectsName = false
                continue
            }

            // Names.
            if b.isNameStart || (g.nameExtras.contains(b) && b != .dash) {
                let end = name(i + 1, hi, extras: g.nameExtras)
                i = classify(g, i, end, hi, expectsName: &expectsName, afterValue: &afterValue)
                continue
            }

            if let end = special(g, b, i, hi, atLineStart: atLineStart, afterValue: afterValue) {
                i = end
                afterValue = true
                expectsName = false
                continue
            }

            afterValue = b == .closeParen || b == .closeBracket
            expectsName = false
            i += 1
        }
    }

    func comment(_ g: Grammar, at i: Int, _ lo: Int, _ hi: Int) -> Int? {
        let b = s[i]
        for block in g.blockComments where block.open[0] == b && has(block.open, at: i, hi) {
            return blockCommentEnd(open: block.open, close: block.close, from: i, nested: g.nestedComments, hi)
        }
        for line in g.lineComments where line[0] == b && has(line, at: i, hi) {
            if g.commentsAfterSpace, i > lo, !s[i - 1].isWhitespace, s[i - 1] != .semicolon { return nil }
            return endOfLine(i, hi)
        }
        return nil
    }

    /// Whether a key-like token ending at `end` is followed by its `:` or `=`.
    func isKey(_ g: Grammar, after end: Int, _ hi: Int) -> Bool {
        guard g.keysBeforeColon || g.keysBeforeEquals else { return false }
        let j = skipBlanks(end, hi)
        guard j < hi else { return false }
        if g.keysBeforeColon { return s[j] == .colon }
        return s[j] == .equals && (j + 1 == hi || s[j + 1] != .equals)
    }

    /// Colors the name at `start..<end` and returns where scanning continues.
    mutating func classify(_ g: Grammar, _ start: Int, _ end: Int, _ hi: Int, expectsName: inout Bool, afterValue: inout Bool) -> Int {
        let declaring = expectsName
        expectsName = false
        afterValue = true

        if g.keysBeforeEquals, isKey(g, after: end, hi) {
            emit(.property, start, end)
            return end
        }

        let word = word(start, end)

        if g.rust, word == "r" || word == "br", end < hi, s[end] == .hash || s[end] == .quote {
            var q = end
            while q < hi, s[q] == .hash { q += 1 }
            if q < hi, s[q] == .quote {
                let close = rawStringEnd(quoteAt: q, hashes: q - end, hi)
                emit(.string, start, close)
                return close
            }
        }

        if g.keywords.contains(g.caseInsensitive ? word.lowercased() : word) {
            emit(.keyword, start, end)
            expectsName = g.declarations.contains(word)
            afterValue = Self.valueKeywords.contains(word)
            return end
        }
        if declaring {
            emit(.function, start, end)
            return end
        }
        if g.types.contains(g.caseInsensitive ? word.lowercased() : word) {
            emit(.type, start, end)
            return end
        }
        if g.rust, end + 1 < hi, s[end] == .bang, s[end + 1] != .equals {
            emit(.function, start, end + 1)
            return end + 1
        }
        if g.capitalizedTypes, s[start].isUppercase, isTypeName(start, end) {
            emit(.type, start, end)
            return end
        }
        if g.functionCalls, end < hi, s[end] == .openParen {
            emit(.function, start, end)
            return end
        }
        if g.builtins.contains(word) {
            emit(.function, start, end)
        }
        return end
    }

    /// `String`, `URL`, `HTTPClient`, but not a constant like `MAX_SIZE`.
    func isTypeName(_ start: Int, _ end: Int) -> Bool {
        var hasLowercase = false
        var hasUnderscore = false
        for k in start..<end {
            if s[k].isLowercase { hasLowercase = true; break }
            if s[k] == .underscore { hasUnderscore = true }
        }
        return hasLowercase || !hasUnderscore
    }

    /// Sigils, directives and other language-specific openers. Returns the end of what it
    /// colored, or nil for ordinary punctuation.
    mutating func special(_ g: Grammar, _ b: UInt8, _ i: Int, _ hi: Int, atLineStart: Bool, afterValue: Bool) -> Int? {
        let next: UInt8 = i + 1 < hi ? s[i + 1] : .newline

        if b == .at, g.attributes, next.isNameStart {
            var end = name(i + 1, hi)
            while end + 1 < hi, s[end] == .dot, s[end + 1].isNameStart { end = name(end + 1, hi) }
            emit(.attribute, i, end)
            return end
        }

        if b == .hash {
            if g.swiftRawStrings {
                var q = i
                while q < hi, s[q] == .hash { q += 1 }
                if q < hi, s[q] == .quote {
                    let end = rawStringEnd(quoteAt: q, hashes: q - i, hi)
                    emit(.string, i, end)
                    return end
                }
            }
            if g.rust, next == .openBracket || (next == .bang && i + 2 < hi && s[i + 2] == .openBracket) {
                var j = i + 1
                var depth = 0
                while j < hi {
                    if s[j] == .openBracket { depth += 1 }
                    if s[j] == .closeBracket {
                        depth -= 1
                        if depth == 0 { j += 1; break }
                    }
                    j += 1
                }
                let end = min(j, hi)
                emit(.attribute, i, end)
                return end
            }
            if g.directives == .anywhere || (g.directives == .lineStart && atLineStart) {
                let start = skipBlanks(i + 1, hi)
                guard start < hi, s[start].isNameStart else { return nil }
                let end = name(start, hi)
                emit(.attribute, i, end)
                guard g.directives == .lineStart, equalsIgnoringCase(start, end, "include") || equalsIgnoringCase(start, end, "import") else { return end }
                let open = skipBlanks(end, hi)
                if open < hi, s[open] == .less {
                    let lineEnd = endOfLine(open, hi)
                    var close = open + 1
                    while close < lineEnd, s[close] != .greater { close += 1 }
                    let stop = min(close + 1, lineEnd)
                    emit(.string, open, stop)
                    return stop
                }
                return end
            }
        }

        if g.variableSigils.contains(b) {
            var j = i + 1
            while j < hi, s[j] == b { j += 1 }  // `@@class_var`
            guard j < hi else { return nil }
            let c = s[j]
            if b == .dollar, c == .openBrace {
                var end = j + 1
                while end < hi, s[end] != .closeBrace, s[end] != .newline { end += 1 }
                end = min(end + 1, hi)
                emit(.variable, i, end)
                return end
            }
            if c.isNameByte {
                let end = name(j, hi)
                emit(.variable, i, end)
                return end
            }
            if b == .dollar, "@?#$!*-".utf8.contains(c) {
                emit(.variable, i, j + 1)
                return j + 1
            }
            return nil
        }

        if b == .apostrophe, g.rust {
            // A char ('a', '\n', 'é') or a lifetime ('a, 'static).
            if next == .backslash {
                var j = i + 2
                let limit = min(hi, i + 12)
                while j < limit, s[j] != .apostrophe, s[j] != .newline { j += 1 }
                let end = j < limit && s[j] == .apostrophe ? j + 1 : j
                emit(.string, i, end)
                return end
            }
            var j = i + 1
            while j < hi, j < i + 5, s[j] >= 0x80 { j += 1 }
            if j == i + 1, j < hi, s[j] != .newline, s[j] != .apostrophe { j += 1 }
            if j < hi, s[j] == .apostrophe {
                emit(.string, i, j + 1)
                return j + 1
            }
            if next.isNameStart {
                let end = name(i + 1, hi)
                emit(.attribute, i, end)
                return end
            }
            return nil
        }

        if b == .colon, g.rubySymbols, next.isNameStart, i == 0 || (s[i - 1] != .colon && !s[i - 1].isNameByte) {
            let end = name(i + 1, hi)
            emit(.string, i, end)
            return end
        }

        if b == .dash, g.shellFlags, i == 0 || s[i - 1].isWhitespace, next.isLetter || next == .dash {
            let end = name(i + 1, hi, extras: [.dash])
            emit(.attribute, i, end)
            return end
        }

        if b == .openBracket, g.sectionHeaders, atLineStart {
            let lineEnd = endOfLine(i, hi)
            var j = i + 1
            while j < lineEnd, s[j] != .closeBracket { j += 1 }
            while j < lineEnd, s[j] == .closeBracket { j += 1 }
            emit(.type, i, j)
            return j
        }

        if b == .slash, g.regexLiterals, !afterValue {
            // A regex runs to an unescaped `/` outside a class, on the same line.
            var j = i + 1
            var inClass = false
            while j < hi, s[j] != .newline {
                let c = s[j]
                if c == .backslash {
                    j += 2
                    continue
                }
                if c == .openBracket { inClass = true }
                if c == .closeBracket { inClass = false }
                if c == .slash, !inClass, j > i + 1 {
                    let end = name(j + 1, hi)
                    emit(.string, i, end)
                    return end
                }
                j += 1
            }
            return nil
        }

        return nil
    }

    // MARK: Markup

    mutating func markup(_ lo: Int, _ hi: Int) {
        var i = lo
        while i < hi {
            guard s[i] == .less else {
                i += 1
                continue
            }
            if has(Delimiter.commentOpen, at: i, hi) {
                let end = blockCommentEnd(open: Delimiter.commentOpen, close: Delimiter.commentClose, from: i, nested: false, hi)
                emit(.comment, i, end)
                i = end
                continue
            }
            if has(Delimiter.cdataOpen, at: i, hi) {
                let end = blockCommentEnd(open: Delimiter.cdataOpen, close: Delimiter.cdataClose, from: i, nested: false, hi)
                emit(.string, i, end)
                i = end
                continue
            }
            let next: UInt8 = i + 1 < hi ? s[i + 1] : .newline
            if next == .bang || next == .question {
                var end = i + 2
                while end < hi, s[end] != .greater { end += 1 }
                end = min(end + 1, hi)
                emit(.meta, i, end)
                i = end
                continue
            }
            let closing = next == .slash
            let nameStart = closing ? i + 2 : i + 1
            guard nameStart < hi, s[nameStart].isNameStart else {
                i += 1
                continue
            }
            let nameEnd = name(nameStart, hi, extras: [.colon, .dash, .dot])
            emit(.keyword, nameStart, nameEnd)

            // Attributes, up to the tag's end.
            var j = nameEnd
            var selfClosing = false
            while j < hi, s[j] != .greater {
                let c = s[j]
                if c == .quote || c == .apostrophe {
                    var end = j + 1
                    while end < hi, s[end] != c { end += 1 }
                    end = min(end + 1, hi)
                    emit(.string, j, end)
                    j = end
                } else if c == .equals {
                    j += 1
                    if j < hi, !s[j].isWhitespace, s[j] != .quote, s[j] != .apostrophe, s[j] != .greater {
                        var end = j
                        while end < hi, !s[end].isWhitespace, s[end] != .greater { end += 1 }
                        emit(.string, j, end)
                        j = end
                    }
                } else if c.isNameStart || c == .colon || c == .at {
                    let end = name(j + 1, hi, extras: [.colon, .dash, .dot])
                    emit(.attribute, j, end)
                    j = end
                } else {
                    selfClosing = c == .slash
                    j += 1
                }
            }
            i = min(j + 1, hi)

            // Scripts and styles in their own language.
            guard !closing, !selfClosing else { continue }
            let isScript = equalsIgnoringCase(nameStart, nameEnd, "script")
            guard isScript || equalsIgnoringCase(nameStart, nameEnd, "style") else { continue }
            let closeTag = isScript ? Delimiter.scriptClose : Delimiter.styleClose
            var close = i
            while close < hi, !(s[close] == .less && matchesIgnoringCase(closeTag, at: close, hi)) { close += 1 }
            if isScript {
                code(.javascript, i, close)
            } else {
                css(lineComments: false, i, close)
            }
            i = close
        }
    }

    func matchesIgnoringCase(_ lowercase: [UInt8], at i: Int, _ hi: Int) -> Bool {
        guard i + lowercase.count <= hi else { return false }
        for k in 0..<lowercase.count where s[i + k] | 0x20 != lowercase[k] | 0x20 { return false }
        return true
    }

    // MARK: CSS

    mutating func css(lineComments: Bool, _ lo: Int, _ hi: Int) {
        var i = lo
        var depth = 0
        var statementStart = true
        var declaration = false
        var inValue = false
        
        while i < hi {
            let b = s[i]
            if b.isWhitespace {
                i += 1
                continue
            }
            let next: UInt8 = i + 1 < hi ? s[i + 1] : .newline

            if b == .slash, next == .star {
                let end = blockCommentEnd(open: Delimiter.blockOpen, close: Delimiter.blockClose, from: i, nested: false, hi)
                emit(.comment, i, end)
                i = end
                continue
            }
            if b == .slash, next == .slash, lineComments, i == lo || s[i - 1] != .colon {
                let end = endOfLine(i, hi)
                emit(.comment, i, end)
                i = end
                continue
            }

            if statementStart {
                statementStart = false
                inValue = false
                // A declaration ends at `;` or `}`; a selector at `{`.
                var j = i
                while j < hi, s[j] != .semicolon, s[j] != .openBrace, s[j] != .closeBrace { j += 1 }
                declaration = (depth > 0 || b == .dollar) && (j == hi || s[j] != .openBrace)
            }

            switch b {
            case .openBrace:
                depth += 1
                statementStart = true
                i += 1
            case .closeBrace:
                depth = max(0, depth - 1)
                statementStart = true
                i += 1
            case .semicolon:
                statementStart = true
                i += 1
            case .quote, .apostrophe:
                let end = stringEnd(b == .quote ? Quote.double : Quote.single, from: i + 1, hi)
                emit(.string, i, end)
                i = end
            case .at where next.isNameStart:
                let end = name(i + 1, hi, extras: [.dash])
                emit(.keyword, i, end)
                i = end
            case .dollar where next.isNameStart:
                let end = name(i + 1, hi, extras: [.dash])
                emit(.variable, i, end)
                i = end
            case .bang where next.isLetter:
                let end = name(i + 1, hi)
                emit(.keyword, i, end)
                i = end
            case .colon where declaration:
                inValue = true
                i += 1
            case .colon where next.isNameStart || next == .colon:
                // Pseudo-classes and elements.
                let start = next == .colon ? i + 2 : i + 1
                let end = name(start, hi, extras: [.dash])
                emit(.keyword, i, end)
                i = end
            case .hash where next.isNameByte || next == .dash:
                let end = name(i + 1, hi, extras: [.dash])
                emit(inValue ? .number : .attribute, i, end)
                i = end
            case .dot where !declaration && (next.isNameStart || next == .dash):
                let end = name(i + 1, hi, extras: [.dash])
                emit(.attribute, i, end)
                i = end
            default:
                let previous = i > lo ? s[i - 1] : .space
                let startsNumber = b.isDigit || ((b == .dot || b == .dash || b == .plus) && next.isDigit)
                if startsNumber, !previous.isNameByte, previous != .dash {
                    var end = number(b.isDigit ? i : i + 1, hi)
                    if end < hi, s[end] == .percent { end += 1 }
                    emit(.number, i, end)
                    i = end
                } else if b.isNameStart || b == .dash {
                    let end = name(i + 1, hi, extras: [.dash])
                    if declaration, !inValue {
                        emit(.property, i, end)
                    } else if inValue {
                        if end < hi, s[end] == .openParen {
                            emit(.function, i, end)
                            if equalsIgnoringCase(i, end, "url") {
                                // A URL's `//` isn't a comment.
                                var stop = end
                                while stop < hi, s[stop] != .closeParen, s[stop] != .newline { stop += 1 }
                                emit(.string, end + 1, stop)
                                i = stop
                                continue
                            }
                        }
                    } else if !declaration {
                        emit(.keyword, i, end)
                    }
                    i = end
                } else {
                    i += 1
                }
            }
        }
    }

    // MARK: YAML

    mutating func yaml(_ lo: Int, _ hi: Int) {
        var i = lo
        /// Inside a `|` or `>` block: the indentation of the line that opened it.
        var blockIndent: Int?

        while i < hi {
            let lineEnd = endOfLine(i, hi)
            var j = i
            while j < lineEnd, s[j] == .space { j += 1 }
            let indent = j - i
            if let parent = blockIndent {
                if j == lineEnd || s[j] == .ret || indent > parent {
                    emit(.string, j, lineEnd)
                    i = lineEnd + 1
                    continue
                }
                blockIndent = nil
            }

            if indent == 0, has(Delimiter.documentStart, at: j, lineEnd) || has(Delimiter.documentEnd, at: j, lineEnd),
               j + 3 == lineEnd || s[j + 3].isWhitespace {
                emit(.meta, j, j + 3)
                j += 3
            }
            // List markers.
            while j < lineEnd, s[j] == .dash, j + 1 == lineEnd || s[j + 1].isWhitespace {
                j = skipBlanks(j + 1, lineEnd)
            }
            if let colon = yamlKeyEnd(j, lineEnd) {
                emit(.property, j, colon)
                j = colon + 1
            }
            if yamlValue(j, lineEnd, lineStart: i) {
                blockIndent = indent
            }
            i = lineEnd + 1
        }
    }

    /// Where the `:` after a key starting at `i` is, if there's a key.
    func yamlKeyEnd(_ i: Int, _ lineEnd: Int) -> Int? {
        guard i < lineEnd else { return nil }
        var j = i
        let first = s[i]
        if first == .quote || first == .apostrophe {
            j = i + 1
            while j < lineEnd, s[j] != first { j += s[j] == .backslash && first == .quote ? 2 : 1 }
            j = skipBlanks(min(j + 1, lineEnd), lineEnd)
            return j < lineEnd && s[j] == .colon && (j + 1 == lineEnd || s[j + 1].isWhitespace) ? j : nil
        }
        guard !"#&*!|>%@`[{,?-".utf8.contains(first) || (first == .dash && i + 1 < lineEnd && !s[i + 1].isWhitespace) else { return nil }
        while j < lineEnd {
            let c = s[j]
            if c == .colon, j + 1 == lineEnd || s[j + 1].isWhitespace { return j > i ? j : nil }
            if c == .hash, j > i, s[j - 1].isBlank { return nil }
            j += 1
        }
        return nil
    }

    /// Colors a value from `i` to the line's end. Returns whether it opens a block scalar.
    mutating func yamlValue(_ i: Int, _ lineEnd: Int, lineStart: Int) -> Bool {
        var j = i
        var flow = 0
        var block = false
        while j < lineEnd {
            let c = s[j]
            if c.isWhitespace {
                j += 1
                continue
            }
            if c == .hash, j == lineStart || s[j - 1].isBlank {
                emit(.comment, j, lineEnd)
                return block
            }
            block = false
            switch c {
            case .quote, .apostrophe:
                let end = stringEnd(c == .quote ? Quote.double : Quote.yamlSingle, from: j + 1, lineEnd)
                emit(.string, j, end)
                j = end
            case .ampersand, .star:
                var end = j + 1
                while end < lineEnd, !s[end].isWhitespace, s[end] != .comma, s[end] != .closeBracket, s[end] != .closeBrace { end += 1 }
                emit(.variable, j, end)
                j = end
            case .bang:
                var end = j + 1
                while end < lineEnd, !s[end].isWhitespace { end += 1 }
                emit(.type, j, end)
                j = end
            case .pipe, .greater:
                block = true
                j += 1
                while j < lineEnd, s[j] == .dash || s[j] == .plus || s[j].isDigit { j += 1 }
            case .openBracket, .openBrace:
                flow += 1
                j += 1
            case .closeBracket, .closeBrace:
                flow = max(0, flow - 1)
                j += 1
            case .comma, .colon:
                j += 1
            default:
                // A plain scalar, up to a comment, a `: ` or (in a flow collection) a separator.
                var end = j
                while end < lineEnd {
                    let d = s[end]
                    if d == .hash, s[end - 1].isBlank { break }
                    if d == .colon, end + 1 == lineEnd || s[end + 1].isWhitespace { break }
                    if flow > 0, d == .comma || d == .closeBracket || d == .closeBrace { break }
                    end += 1
                }
                var trimmed = end
                while trimmed > j, s[trimmed - 1].isWhitespace { trimmed -= 1 }
                if isYAMLLiteral(j, trimmed) {
                    emit(.keyword, j, trimmed)
                } else if isYAMLNumber(j, trimmed) {
                    emit(.number, j, trimmed)
                }
                j = end
            }
        }
        return block
    }

    func isYAMLLiteral(_ start: Int, _ end: Int) -> Bool {
        if end - start == 1 { return s[start] == .tilde }
        return Self.yamlLiterals.contains { equalsIgnoringCase(start, end, $0) }
    }

    func isYAMLNumber(_ start: Int, _ end: Int) -> Bool {
        guard start < end else { return false }
        var j = start
        if s[j] == .dash || s[j] == .plus { j += 1 }
        if j + 1 < end, s[j] == .dot, equalsIgnoringCase(j + 1, end, "inf") || equalsIgnoringCase(j + 1, end, "nan") { return true }
        guard j < end, s[j].isDigit || (s[j] == .dot && j + 1 < end && s[j + 1].isDigit) else { return false }
        if s[j] == UInt8(ascii: "0"), j + 2 < end, s[j + 1] | 0x20 == UInt8(ascii: "x") {
            return (j + 2..<end).allSatisfy { s[$0].isHexDigit }
        }
        var dots = 0
        while j < end, s[j].isDigit || s[j] == .underscore || s[j] == .dot {
            if s[j] == .dot { dots += 1 }
            j += 1
        }
        if j < end, s[j] | 0x20 == UInt8(ascii: "e") {
            j += 1
            if j < end, s[j] == .plus || s[j] == .dash { j += 1 }
            guard j < end, s[j].isDigit else { return false }
            while j < end, s[j].isDigit { j += 1 }
        }
        return j == end && dots <= 1
    }

    // MARK: Diff

    mutating func diff(_ lo: Int, _ hi: Int) {
        var i = lo
        while i < hi {
            let lineEnd = endOfLine(i, hi)
            let kind: SyntaxToken.Kind? = switch s[i] {
            case _ where has(Delimiter.newFile, at: i, lineEnd) || has(Delimiter.documentStart, at: i, lineEnd): .meta
            case .plus: .inserted
            case .dash: .deleted
            case _ where has(Delimiter.hunk, at: i, lineEnd): .meta
            case _ where has(Delimiter.diffHeader, at: i, lineEnd) || has(Delimiter.indexHeader, at: i, lineEnd): .meta
            default: nil
            }
            if let kind, lineEnd > i {
                emit(kind, i, lineEnd)
            }
            i = lineEnd + 1
        }
    }
}
