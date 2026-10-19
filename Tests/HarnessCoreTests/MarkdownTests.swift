import Foundation
import Testing
@testable import HarnessCore

struct MarkdownParserTests {
    @Test func headingsParagraphsAndRules() {
        let blocks = MarkdownParser.parse("# Title ##\nSome *text*\nmore\n\n---\n### Sub")
        #expect(blocks == [
            .heading(level: 1, text: "Title"),
            .paragraph("Some *text*\nmore"),
            .rule,
            .heading(level: 3, text: "Sub"),
        ])
    }

    @Test func listsGroupNestAndContinue() throws {
        let blocks = MarkdownParser.parse("""
        - one
          still one
        - two

        - three
          - nested
        1. first
        2) second
        - [ ] todo
        - [x] done

        After
        """)
        #expect(blocks.count == 2)
        guard case .list(let items) = blocks[0] else { Issue.record("expected a list"); return }
        #expect(items.map(\.text) == ["one\nstill one", "two", "three", "nested", "first", "second", "todo", "done"])
        #expect(items[3].depth == 1)
        #expect(items[4].marker == .number(1))
        #expect(items[5].marker == .number(2))
        #expect(items[6].task == false)
        #expect(items[7].task == true)
        #expect(blocks[1] == .paragraph("After"))
    }

    @Test func tables() throws {
        let blocks = MarkdownParser.parse("""
        Intro
        | Phase | Status | n |
        |---|:---:|--:|
        | 1. Node | Done, `a|b` ok | 3 |
        | 2. DB | Done |
        """)
        #expect(blocks.first == .paragraph("Intro"))
        guard case .table(let table) = try #require(blocks.last) else { Issue.record("expected a table"); return }
        #expect(table.header == ["Phase", "Status", "n"])
        #expect(table.alignments == [.leading, .center, .trailing])
        #expect(table.rows == [["1. Node", "Done, `a|b` ok", "3"], ["2. DB", "Done", ""]])
    }

    @Test func pipesWithoutSeparatorStayText() {
        #expect(MarkdownParser.parse("a | b\nnext") == [.paragraph("a | b\nnext")])
    }

    @Test func codeFencesKeepContentVerbatim() {
        let blocks = MarkdownParser.parse("```swift\nlet a = 1\n# not a heading\n```\nafter")
        #expect(blocks == [.code(language: "swift", text: "let a = 1\n# not a heading"), .paragraph("after")])
    }

    @Test func streamingHoldsBackPendingConstructs() {
        #expect(MarkdownParser.parse("Hello\n| Phase | Status |", streaming: true) == [.paragraph("Hello")])
        #expect(MarkdownParser.parse("Hello\n| Phase | Status |\n", streaming: true) == [.paragraph("Hello")])
        #expect(MarkdownParser.parse("Hello\n#", streaming: true) == [.paragraph("Hello")])
        #expect(MarkdownParser.parse("Hello\n- ", streaming: true) == [.paragraph("Hello")])
        // An open fence is a code block in progress.
        #expect(MarkdownParser.parse("```py\nprint(1)", streaming: true) == [.code(language: "py", text: "print(1)")])
        // Not streaming: nothing is held back.
        #expect(MarkdownParser.parse("Hello\n#") == [.paragraph("Hello\n#")])
    }

    @Test func closesUnterminatedInlineSyntax() {
        #expect(MarkdownInline.closingUnterminated("a **bold") == "a **bold**")
        #expect(MarkdownInline.closingUnterminated("a **") == "a ")
        #expect(MarkdownInline.closingUnterminated("run `ls") == "run `ls`")
        #expect(MarkdownInline.closingUnterminated("see [the docs](https://exa") == "see the docs")
        #expect(MarkdownInline.closingUnterminated("see [the do") == "see the do")
        #expect(MarkdownInline.closingUnterminated("an *em") == "an *em*")
        #expect(MarkdownInline.closingUnterminated("`a**b` done") == "`a**b` done")
        #expect(MarkdownInline.closingUnterminated("done **ok** and [x](y)") == "done **ok** and [x](y)")
    }
}

struct IssueReferenceTests {
    private let docs = GitRemote(host: "github.com", path: "acme/docs", forge: .github)
    private let api = GitRemote(host: "github.com", path: "acme/api", forge: .github)
    private let hooks = GitRemote(host: "gitlab.com", path: "acme/hooks", forge: .gitlab)

    private func pr(_ remote: GitRemote, _ number: Int) -> PullRequest {
        PullRequest(
            remote: remote, number: number, title: "", url: URL(string: "https://\(remote.host)/\(remote.path)/pull/\(number)")!,
            author: nil, isDraft: false, headBranch: "h\(number)", baseBranch: "main", updatedAt: nil,
            review: .none, checks: .none, isMine: false, reviewRequestedFromMe: false
        )
    }

    @Test func findsReferences() {
        let refs = IssueReference.find(in: "PRs #4 and #5, acme/api#9, docs#2, hooks!7. Not color#fff, not a#b, not x!3, not https://x.io/a#12")
        // `x!3` is found; the linker decides it isn't a repo.
        #expect(refs.map(\.number) == [4, 5, 9, 2, 7, 3])
        #expect(refs[2].path == "acme/api")
        #expect(refs[3].repoName == "docs")
        #expect(refs[4].isMergeRequest)
        let text = "PRs #4"
        let first = IssueReference.find(in: text)[0]
        #expect(Array(text)[first.range].map(String.init).joined() == "#4")
    }

    @Test func resolvesUsingOpenPRsAndContext() {
        let linker = IssueLinker(
            repos: [.init(name: "docs", remote: docs), .init(name: "api", remote: api), .init(name: "hooks", remote: hooks)],
            openPullRequests: [pr(docs, 4), pr(api, 4), pr(api, 5)]
        )
        func link(_ text: String) -> String? {
            IssueReference.find(in: text).first.flatMap { linker.url(for: $0, context: text)?.absoluteString }
        }
        // One open PR with that number anywhere.
        #expect(link("see #5") == "https://github.com/acme/api/pull/5")
        // Ambiguous number, settled by the repo named nearby.
        #expect(link("Open docs PRs #4 are stale") == "https://github.com/acme/docs/pull/4")
        // Ambiguous and nothing settles it: no link rather than a wrong one.
        #expect(link("PR #4 is stale") == nil)
        // Closed PRs aren't in the open list; the mentioned repo still gives a URL.
        #expect(link("the api change in #77") == "https://github.com/acme/api/issues/77")
        #expect(link("acme/api#3") == "https://github.com/acme/api/issues/3")
        #expect(link("other/thing#3") == "https://github.com/other/thing/issues/3")
        #expect(link("hooks!7") == "https://gitlab.com/acme/hooks/-/merge_requests/7")
        #expect(link("unknown#3") == nil)
        #expect(link("Done!1") == nil)
    }

    @Test func singleRepoProjectsAlwaysLink() {
        let linker = IssueLinker(repos: [.init(name: "docs", remote: docs)], openPullRequests: [])
        let ref = IssueReference.find(in: "fixed in #12")[0]
        #expect(linker.url(for: ref, context: "fixed in #12")?.absoluteString == "https://github.com/acme/docs/issues/12")
    }
}
