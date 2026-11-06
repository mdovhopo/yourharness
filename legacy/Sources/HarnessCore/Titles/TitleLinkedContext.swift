import Foundation

/// Looks up pull requests and issues linked in a message, so "review this" plus a link
/// gets titled after what the link is about. GitHub only for now, through `gh`.
public enum TitleLinkedContext {
    struct Link: Equatable {
        var url: String
        var owner: String
        var repo: String
        var number: Int
    }

    /// The first two distinct GitHub pull request or issue links.
    static func links(in message: String) -> [Link] {
        guard let regex = try? NSRegularExpression(pattern: #"https://github\.com/([\w.-]+)/([\w.-]+)/(?:pull|issues)/(\d+)"#) else { return [] }
        let range = NSRange(message.startIndex..., in: message)
        var links: [Link] = []
        for match in regex.matches(in: message, range: range) {
            guard let url = Range(match.range, in: message), let owner = Range(match.range(at: 1), in: message),
                  let repo = Range(match.range(at: 2), in: message), let number = Range(match.range(at: 3), in: message),
                  let value = Int(message[number]) else { continue }
            let link = Link(url: String(message[url]), owner: String(message[owner]), repo: String(message[repo]), number: value)
            if links.contains(where: { $0.url == link.url }) { continue }
            links.append(link)
            if links.count == 2 { break }
        }
        return links
    }

    /// Each link with its title and the start of its description, or "unavailable".
    public static func resolve(message: String, gh: URL?, environment: [String: String]) async -> String? {
        let links = links(in: message)
        guard !links.isEmpty else { return nil }
        var sections: [String] = []
        for link in links {
            sections.append(await describe(link, gh: gh, environment: environment))
        }
        return sections.joined(separator: "\n\n")
    }

    private static func describe(_ link: Link, gh: URL?, environment: [String: String]) async -> String {
        guard let gh else { return "\(link.url): unavailable" }
        // The issues endpoint answers for pull requests too.
        let output = await CommandRunner.run(
            gh, arguments: ["api", "repos/\(link.owner)/\(link.repo)/issues/\(link.number)"],
            environment: environment, timeout: 3
        )
        guard output.succeeded,
              let object = try? JSONSerialization.jsonObject(with: Data(output.stdout.utf8)) as? [String: Any],
              let title = object["title"] as? String else { return "\(link.url): unavailable" }
        let body = (object["body"] as? String) ?? ""
        let summary: [String: String] = ["title": String(title.prefix(300)), "body": String(body.prefix(1_200))]
        guard let data = try? JSONSerialization.data(withJSONObject: summary, options: [.sortedKeys, .withoutEscapingSlashes]),
              let json = String(data: data, encoding: .utf8) else { return "\(link.url): unavailable" }
        return "\(link.url)\n\(json)"
    }
}
