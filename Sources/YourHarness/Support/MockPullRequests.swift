import Foundation
import HarnessCore

/// Development: `YH_MOCK_PULLS=<pulls.json>` serves pull requests from a file instead of
/// asking `gh`/`glab`, so a scratch workspace can show PRs. Each entry:
/// `{"repo": "github.com/acme/api", "number": 57, "title": "…", "head": "feature/login",
/// "base": "main", "draft": false, "review": "approved", "checks": "passing",
/// "state": "open", "author": "you", "mine": true}`. Off unless set.
enum MockPullRequests {
    static let all: [PullRequest]? = {
        guard let path = ProcessInfo.processInfo.environment["YH_MOCK_PULLS"], !path.isEmpty,
              let data = FileManager.default.contents(atPath: path),
              let items = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else { return nil }
        return items.compactMap { item in
            guard let repo = item["repo"] as? String, let number = item["number"] as? Int,
                  let slash = repo.firstIndex(of: "/") else { return nil }
            let host = String(repo[..<slash]), path = String(repo[repo.index(after: slash)...])
            let isGitLab = host.contains("gitlab")
            let remote = GitRemote(host: host, path: path, forge: isGitLab ? .gitlab : .github)
            return PullRequest(
                remote: remote, number: number, title: item["title"] as? String ?? "",
                url: URL(string: "https://\(repo)/\(isGitLab ? "-/merge_requests" : "pull")/\(number)")!,
                author: item["author"] as? String, isDraft: item["draft"] as? Bool ?? false,
                headBranch: item["head"] as? String ?? "", baseBranch: item["base"] as? String ?? "main",
                updatedAt: .now, review: PullRequest.Review(rawValue: item["review"] as? String ?? "none") ?? .none,
                checks: PullRequest.Checks(rawValue: item["checks"] as? String ?? "none") ?? .none,
                isMine: item["mine"] as? Bool ?? false, reviewRequestedFromMe: item["reviewRequested"] as? Bool ?? false,
                state: PullRequest.State(rawValue: item["state"] as? String ?? "open") ?? .open)
        }
    }()
}
