import Foundation
import Testing
@testable import HarnessCore

struct SyntaxHighlighterTests {
    /// Each token as its kind and text.
    private func spans(_ text: String, _ language: String) -> [Pair] {
        SyntaxHighlighter.tokens(in: text, language: language).map { Pair($0.kind, String(text[$0.range])) }
    }

    private func texts(_ text: String, _ language: String, _ kind: SyntaxToken.Kind) -> [String] {
        spans(text, language).filter { $0.kind == kind }.map(\.text)
    }

    struct Pair: Equatable, CustomStringConvertible {
        var kind: SyntaxToken.Kind
        var text: String
        init(_ kind: SyntaxToken.Kind, _ text: String) {
            self.kind = kind
            self.text = text
        }
        var description: String { "\(kind)(\(text))" }
    }

    @Test func swift() {
        let code = """
        @MainActor
        struct Greeter: View {
            // Says hello.
            let name: String = "Ada \\(first("x")) \\"quoted\\""
            func greet(times: Int) -> Int { print(name); return 0x1F + 2.5e-3 }
        }
        """
        #expect(spans(code, "swift") == [
            Pair(.attribute, "@MainActor"),
            Pair(.keyword, "struct"),
            Pair(.type, "Greeter"),
            Pair(.type, "View"),
            Pair(.comment, "// Says hello."),
            Pair(.keyword, "let"),
            Pair(.type, "String"),
            Pair(.string, #""Ada \(first("x")) \"quoted\"""#),
            Pair(.keyword, "func"),
            Pair(.function, "greet"),
            Pair(.type, "Int"),
            Pair(.type, "Int"),
            Pair(.function, "print"),
            Pair(.keyword, "return"),
            Pair(.number, "0x1F"),
            Pair(.number, "2.5e-3"),
        ])
    }

    @Test func swiftMultilineAndRawStringsAndDirectives() {
        let code = #"""
        #if DEBUG
        let a = """
          line "one"
          """
        let b = #"raw \(no) "quotes""#
        #endif
        let t = x.0 + 1..<5
        """#
        let strings = texts(code, "swift", .string)
        #expect(strings == ["\"\"\"\n  line \"one\"\n  \"\"\"", ##"#"raw \(no) "quotes""#"##])
        #expect(texts(code, "swift", .attribute) == ["#if", "#endif"])
        #expect(texts(code, "swift", .number) == ["1", "5"])
    }

    @Test func nestedBlockComments() {
        #expect(spans("/* a /* b */ c */ d", "swift") == [Pair(.comment, "/* a /* b */ c */")])
        #expect(spans("/* a /* b */ c */ d", "c") == [Pair(.comment, "/* a /* b */")])
    }

    @Test func typescript() {
        let code = """
        import { a } from "./a"
        export async function load<T>(url: string): Promise<T> {
          const re = /ab+c\\/[/]/gi, half = total / 2
          return `${url}?q=${encode("x`y")}`
        }
        """
        #expect(texts(code, "ts", .keyword) == ["import", "from", "export", "async", "function", "const", "return"])
        #expect(texts(code, "typescript", .function).contains("load"))
        #expect(texts(code, "tsx", .type) == ["T", "string", "Promise", "T"])
        #expect(texts(code, "ts", .string) == [#""./a""#, #"/ab+c\/[/]/gi"#, #"`${url}?q=${encode("x`y")}`"#])
        #expect(texts(code, "ts", .number) == ["2"])
    }

    @Test func python() {
        let code = """
        @dataclass
        class Point(Base):
            '''Doc "string"'''
            def norm(self, scale: float = 1.0) -> float:  # length
                return f"{self.x!r}" if self else None
        """
        #expect(texts(code, "python", .attribute) == ["@dataclass"])
        #expect(texts(code, "py", .type) == ["Point", "Base", "float", "float"])
        #expect(texts(code, "py", .function) == ["norm"])
        #expect(texts(code, "py", .string) == [#"'''Doc "string"'''"#, #""{self.x!r}""#])
        #expect(texts(code, "py", .comment) == ["# length"])
        #expect(texts(code, "py", .keyword) == ["class", "def", "self", "return", "if", "self", "else", "None"])
    }

    @Test func goAndRust() {
        let go = "func (s *Server) Run(ctx context.Context) error {\n\treturn fmt.Errorf(`raw \\n`)\n}"
        #expect(texts(go, "go", .keyword) == ["func", "return"])
        #expect(texts(go, "go", .type) == ["error"])
        #expect(texts(go, "go", .function) == ["Run", "Errorf"])
        #expect(texts(go, "go", .string) == ["`raw \\n`"])

        let rust = """
        #[derive(Debug)]
        fn longest<'a>(x: &'a str, c: char) -> &'a str {
            let raw = r#"a "b" c"#;
            println!("{}", '\\n' == c || 'é' == c);
        }
        """
        #expect(texts(rust, "rs", .attribute) == ["#[derive(Debug)]", "'a", "'a", "'a"])
        #expect(texts(rust, "rust", .function) == ["longest", "println!"])
        #expect(texts(rust, "rust", .type) == ["str", "char", "str"])
        #expect(texts(rust, "rust", .string) == [##"r#"a "b" c"#"##, #""{}""#, #"'\n'"#, "'é'"])
    }

    @Test func cFamily() {
        let code = """
        #include <stdio.h>
        #define MAX_SIZE 10
        int main(void) { char c = '\\''; return MAX_SIZE; }
        """
        #expect(texts(code, "c", .attribute) == ["#include", "#define"])
        #expect(texts(code, "cpp", .string) == ["<stdio.h>", #"'\''"#])
        #expect(texts(code, "c", .type) == ["int", "void", "char"])
        #expect(texts(code, "objc", .function) == ["main"])
        #expect(!texts(code, "c", .type).contains("MAX_SIZE"))
    }

    @Test func shell() {
        let code = """
        #!/bin/bash
        set -euo pipefail
        for f in "$HOME/${dir}"/*.txt; do echo "$(basename "$f")" 'no $expansion' # done
        done
        echo a#b ${#items[@]} $1
        """
        #expect(texts(code, "bash", .comment) == ["#!/bin/bash", "# done"])
        #expect(texts(code, "sh", .keyword) == ["set", "for", "in", "do", "done"])
        #expect(texts(code, "zsh", .attribute) == ["-euo"])
        #expect(texts(code, "shell", .string) == [#""$HOME/${dir}""#, #""$(basename "$f")""#, "'no $expansion'"])
        #expect(texts(code, "console", .variable) == ["${#items[@]}", "$1"])
        #expect(texts(code, "bash", .function) == ["echo", "echo"])
    }

    @Test func json() {
        let code = #"{"name": "x", "n": -1.5e3, "ok": true, "none": null, "list": ["a\"b"]}"#
        #expect(spans(code, "json") == [
            Pair(.property, #""name""#), Pair(.string, #""x""#),
            Pair(.property, #""n""#), Pair(.number, "1.5e3"),
            Pair(.property, #""ok""#), Pair(.keyword, "true"),
            Pair(.property, #""none""#), Pair(.keyword, "null"),
            Pair(.property, #""list""#), Pair(.string, #""a\"b""#),
        ])
    }

    @Test func yaml() {
        let code = """
        ---
        name: build # the job
        on: [push, pull_request]
        jobs:
          test:
            runs-on: "ubuntu-latest"
            steps:
              - uses: actions/checkout@v4
              - run: |
                  swift build
                  key: not a key
              - retries: 3
                anchor: &base
                url: http://example.com:80
        """
        #expect(texts(code, "yaml", .meta) == ["---"])
        #expect(texts(code, "yml", .property) == ["name", "on", "jobs", "test", "runs-on", "steps", "uses", "run", "retries", "anchor", "url"])
        #expect(texts(code, "yaml", .comment) == ["# the job"])
        #expect(texts(code, "yaml", .string) == [#""ubuntu-latest""#, "swift build", "key: not a key"])
        #expect(texts(code, "yaml", .number) == ["3"])
        #expect(texts(code, "yaml", .variable) == ["&base"])
    }

    @Test func tomlAndIni() {
        let code = """
        [package]
        name = "app" # comment
        version-code = 3
        [[bin]]
        enabled = true
        """
        #expect(texts(code, "toml", .type) == ["[package]", "[[bin]]"])
        #expect(texts(code, "toml", .property) == ["name", "version-code", "enabled"])
        #expect(texts(code, "toml", .comment) == ["# comment"])
        #expect(texts(code, "toml", .keyword) == ["true"])
        #expect(texts("[core]\n; note\nbare = yes", "ini", .comment) == ["; note"])
    }

    @Test func sqlIsCaseInsensitive() {
        let code = "SELECT id, COUNT(*) FROM users -- all\nwhere name = 'O''Brien' and age > 21::int;"
        #expect(texts(code, "sql", .keyword) == ["SELECT", "FROM", "where", "and"])
        #expect(texts(code, "sql", .function) == ["COUNT"])
        #expect(texts(code, "sql", .comment) == ["-- all"])
        #expect(texts(code, "sql", .string) == ["'O''Brien'"])
        #expect(texts(code, "sql", .type) == ["int"])
        #expect(texts(code, "sql", .number) == ["21"])
    }

    @Test func markupWithScriptAndStyle() {
        let code = """
        <!DOCTYPE html>
        <!-- note -->
        <div class="a" data-x=1 hidden>Hi & bye</div>
        <script>const x = "</div>";</script>
        <style>p { color: #fff; }</style>
        """
        #expect(texts(code, "html", .meta) == ["<!DOCTYPE html>"])
        #expect(texts(code, "html", .comment) == ["<!-- note -->"])
        #expect(texts(code, "xml", .keyword) == ["div", "div", "script", "const", "script", "style", "p", "style"])
        #expect(texts(code, "html", .attribute) == ["class", "data-x", "hidden"])
        #expect(texts(code, "html", .string) == [#""a""#, "1", #""</div>""#])
        #expect(texts(code, "html", .property) == ["color"])
        #expect(texts(code, "html", .number) == ["#fff"])
    }

    @Test func css() {
        let code = """
        @media (max-width: 600px) {
          .card > a:hover, #main { margin: -4px auto; background: url(//cdn/x.png) !important; }
        }
        // scss comment
        $gap: 1.5rem;
        """
        #expect(texts(code, "css", .keyword).starts(with: ["@media"]))
        #expect(texts(code, "scss", .attribute) == [".card", "#main"])
        #expect(texts(code, "scss", .keyword).contains("a:hover"))
        #expect(texts(code, "scss", .property) == ["margin", "background"])
        #expect(texts(code, "scss", .number) == ["600px", "-4px", "1.5rem"])
        #expect(texts(code, "scss", .string) == ["//cdn/x.png"])
        #expect(texts(code, "scss", .comment) == ["// scss comment"])
        #expect(texts(code, "scss", .variable) == ["$gap"])
        #expect(texts(code, "css", .comment).isEmpty)
    }

    @Test func diff() {
        let code = """
        diff --git a/x b/x
        --- a/x
        +++ b/x
        @@ -1,2 +1,2 @@
         same
        -old
        +new
        """
        #expect(spans(code, "diff") == [
            Pair(.meta, "diff --git a/x b/x"),
            Pair(.meta, "--- a/x"),
            Pair(.meta, "+++ b/x"),
            Pair(.meta, "@@ -1,2 +1,2 @@"),
            Pair(.deleted, "-old"),
            Pair(.inserted, "+new"),
        ])
    }

    @Test func otherLanguages() {
        #expect(texts("fun main() { val s = \"$x ${y}\" }", "kotlin", .keyword) == ["fun", "val"])
        #expect(texts("public class A { @Override void f() {} }", "java", .attribute) == ["@Override"])
        #expect(texts("def go(x) = puts \"#{x}\" if @ready && :sym", "ruby", .variable) == ["@ready"])
        #expect(texts("def go(x) = puts \"#{x}\" if @ready && :sym", "rb", .string) == [##""#{x}""##, ":sym"])
        #expect(texts("FROM swift:6 AS build\nRUN swift build -c release # go", "dockerfile", .keyword) == ["FROM", "AS", "RUN"])
        #expect(texts("local x = 1 --[[ block ]] -- line", "lua", .comment) == ["--[[ block ]]", "-- line"])
        #expect(texts("var s = \"x\";", "csharp", .keyword) == ["var"])
    }

    @Test func unknownLanguagesStayPlain() {
        #expect(SyntaxHighlighter.tokens(in: "let x = 1", language: "").isEmpty)
        #expect(SyntaxHighlighter.tokens(in: "# Title\n`code`", language: "markdown").isEmpty)
        #expect(SyntaxHighlighter.tokens(in: "let x = 1", language: "brainfudge").isEmpty)
        #expect(SyntaxHighlighter.highlights("Swift"))
        #expect(SyntaxHighlighter.highlights("ts title=app.ts"))
        #expect(!SyntaxHighlighter.highlights("text"))
    }

    @Test func unterminatedConstructsRunToTheirEnd() {
        // A reply streams in a character at a time, so every prefix must highlight.
        #expect(spans("let s = \"abc", "swift").last == Pair(.string, "\"abc"))
        #expect(spans("let s = \"abc\nlet t", "swift").map(\.text) == ["let", "\"abc", "let"])
        #expect(spans("x /* open\nstill", "go") == [Pair(.comment, "/* open\nstill")])
        #expect(spans("s = '''doc\nmore", "python").last == Pair(.string, "'''doc\nmore"))
        #expect(spans("let a = \"\"\"\nbody \\(x", "swift").last == Pair(.string, "\"\"\"\nbody \\(x"))
        #expect(spans("const t = `a ${b", "js").last == Pair(.string, "`a ${b"))
        #expect(spans("let r = r#\"open", "rust").last == Pair(.string, "r#\"open"))
        #expect(spans("<div class=\"a", "html").last == Pair(.string, "\"a"))
        #expect(spans("<!-- open", "html") == [Pair(.comment, "<!-- open")])
        #expect(spans("a: \"open", "yaml").last == Pair(.string, "\"open"))
        #expect(spans("echo \"$(date", "sh").last == Pair(.string, "\"$(date"))
        #expect(spans("s = \"esc\\", "python").last == Pair(.string, "\"esc\\"))

        // Every prefix of a mixed sample, cut at every byte that's a character boundary.
        let sample = """
        /// Doc
        @attr func f(_ x: Int) -> String { "a\\(x)b" + #"r"# /* c */ + '\\'' }
        """
        for language in ["swift", "ts", "py", "rs", "go", "c", "rb", "sh", "sql", "json", "yaml", "toml", "html", "css", "diff"] {
            for end in sample.indices {
                let prefix = String(sample[..<end])
                for token in SyntaxHighlighter.tokens(in: prefix, language: language) {
                    #expect(!token.range.isEmpty)
                    #expect(token.range.upperBound <= prefix.endIndex)
                }
            }
        }
    }

    @Test func tokensAreOrderedAndDisjoint() {
        let code = String(repeating: "func a() { let s = \"x\\(y)\" // c\n}\n", count: 20)
        let tokens = SyntaxHighlighter.tokens(in: code, language: "swift")
        for (a, b) in zip(tokens, tokens.dropFirst()) {
            #expect(a.range.upperBound <= b.range.lowerBound)
        }
    }

    @Test func nonASCIIStaysWhole() {
        let code = "let café = \"naïve 🎉\" // ünïcode"
        #expect(spans(code, "swift") == [Pair(.keyword, "let"), Pair(.string, "\"naïve 🎉\""), Pair(.comment, "// ünïcode")])
    }

    @Test func bridgedStringsWork() {
        let code = NSString(string: "let x = \"é\" // ok") as String
        #expect(spans(code, "swift") == [Pair(.keyword, "let"), Pair(.string, "\"é\""), Pair(.comment, "// ok")])
    }

    @Test func largeFilesHighlightQuickly() {
        let line = "    let value: [String: Int] = compute(\"key \\(index)\", 42) // note\n"
        let code = String(repeating: line, count: 5_000)
        let clock = ContinuousClock()
        let elapsed = clock.measure {
            #expect(SyntaxHighlighter.tokens(in: code, language: "swift").count > 30_000)
        }
        // Debug builds are several times slower than release; this only catches a blow-up.
        #expect(elapsed < .seconds(2))
    }
}
