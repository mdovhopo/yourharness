//! Links in Claude's replies that the markdown didn't make: bare `https://…` addresses, `#12`
//! and `owner/repo#12` (the thread's repo's issues and pull requests), and paths of the
//! thread's files (`src/app.rs:12`, `app.rs:12-20`, `app.rs#L5`, an absolute path under its
//! folder). Found from the folder's file list alone: nothing is read from disk while drawing,
//! and a name that could be several files isn't linked rather than linked to the wrong one.

use crate::markdown::{Markdown, MdBlock, Span, Styled};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// The thread's repo, as its `origin` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub host: String,
    /// `owner/name` (GitLab: `group/sub/name`).
    pub path: String,
    pub gitlab: bool,
}

impl Repo {
    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// What a thread's replies link against: its folder, its files and its repo.
#[derive(Debug, Default)]
pub struct Linker {
    /// The folder paths are relative to, without a trailing `/`.
    pub root: String,
    pub repo: Option<Repo>,
    paths: HashSet<String>,
    /// Last path component to every path with it.
    by_name: HashMap<String, Vec<String>>,
    /// Changes when the files or the repo do, so text linked against it is linked again then.
    pub revision: u64,
}

impl Linker {
    /// `files` relative to `root`, as `files::list` gives them.
    pub fn new(root: &str, repo: Option<Repo>, files: &[String]) -> Linker {
        use std::hash::{Hash, Hasher};
        let root = root.trim_end_matches('/').to_string();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        root.hash(&mut hasher);
        repo.as_ref().map(|r| (&r.host, &r.path)).hash(&mut hasher);
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for path in files {
            path.hash(&mut hasher);
            let name = path.rsplit('/').next().unwrap_or(path);
            by_name.entry(name.to_string()).or_default().push(path.clone());
        }
        Linker { root, repo, paths: files.iter().cloned().collect(), by_name, revision: hasher.finish() | 1 }
    }

    /// Links every paragraph, heading, list item, quote and table cell; code blocks stay as written.
    pub fn link_markdown(&self, md: &mut Markdown) {
        for block in &mut md.blocks {
            match block {
                MdBlock::Paragraph(s) | MdBlock::Heading(_, s) | MdBlock::Quote(s) => self.link(s),
                MdBlock::List(items) => items.iter_mut().for_each(|(_, _, s)| self.link(s)),
                MdBlock::Table { header, rows } => header.iter_mut().chain(rows.iter_mut().flatten()).for_each(|s| self.link(s)),
                MdBlock::Code { .. } | MdBlock::Rule => {}
            }
        }
    }

    /// Adds the links found in one piece of text; the ones its author made stay as they are.
    pub fn link(&self, styled: &mut Styled) {
        let text = styled.text.clone();
        for (range, url) in bare_urls(&text) {
            mark(styled, range, url, false);
        }
        if text.contains('#') || text.contains('!') {
            for r in issue_references(&text) {
                if let Some(url) = self.issue_url(&r) {
                    mark(styled, r.range, url, false);
                }
            }
        }
        if !self.paths.is_empty() && (text.contains('/') || text.contains('.')) {
            for (range, path, lines) in self.file_references(&text) {
                let mut url = format!("{}/{}", self.root, path);
                match lines {
                    Some((a, b)) if b > a => url += &format!(":{a}-{b}"),
                    Some((a, _)) => url += &format!(":{a}"),
                    None => {}
                }
                mark(styled, range, url, true);
            }
        }
    }

    /// GitHub sends `/issues/N` on to `/pull/N` when N is a pull request, so one URL serves both.
    fn issue_url(&self, r: &IssueReference) -> Option<String> {
        let repo = match (&r.path, &r.repo_name) {
            (Some(path), _) => match &self.repo {
                Some(repo) if repo.path.eq_ignore_ascii_case(path) => repo.clone(),
                _ => Repo { host: "github.com".into(), path: path.clone(), gitlab: false },
            },
            (None, Some(name)) => self.repo.clone().filter(|repo| repo.name().eq_ignore_ascii_case(name))?,
            (None, None) => self.repo.clone()?,
        };
        let base = format!("https://{}/{}", repo.host, repo.path);
        match (repo.gitlab, r.merge_request) {
            (true, true) => Some(format!("{base}/-/merge_requests/{}", r.number)),
            (true, false) => Some(format!("{base}/-/issues/{}", r.number)),
            (false, false) => Some(format!("{base}/issues/{}", r.number)),
            (false, true) => None,
        }
    }

    /// The path, relative to the root, that `candidate` names, if it's one of the root's files.
    pub fn resolve(&self, candidate: &str) -> Option<String> {
        let mut relative = candidate.to_string();
        if let Some(rest) = relative.strip_prefix("~/") {
            relative = format!("{}/{rest}", std::env::var("HOME").unwrap_or_default());
        }
        if relative.starts_with('/') {
            // Outside the folder: never linked, so nothing outside it is ever opened.
            relative = relative.strip_prefix(&format!("{}/", self.root))?.to_string();
        }
        while let Some(rest) = relative.strip_prefix("./") {
            relative = rest.to_string();
        }
        let bare = relative.trim_end_matches('/');
        if bare.is_empty() || bare.starts_with("../") {
            return None;
        }
        if self.paths.contains(bare) {
            return Some(bare.to_string());
        }
        // A name or the end of a path: linked when only one file ends that way.
        let name = bare.rsplit('/').next().unwrap_or(bare);
        let suffix = format!("/{bare}");
        let mut matching = self.by_name.get(name)?.iter().filter(|p| *p == bare || p.ends_with(&suffix));
        let first = matching.next()?;
        matching.next().is_none().then(|| first.clone())
    }

    /// Paths of the root's files in `text`: (where, path relative to the root, lines).
    pub fn file_references(&self, text: &str) -> Vec<(Range<usize>, String, Option<(usize, usize)>)> {
        let b = text.as_bytes();
        let token = |c: u8| c.is_ascii_alphanumeric() || b"_~@+-./".contains(&c);
        let mut found = vec![];
        let mut i = 0;
        while i < b.len() {
            if !token(b[i]) || (i > 0 && token(b[i - 1])) {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i;
            while end < b.len() && token(b[end]) {
                end += 1;
            }
            i = end;
            // Trim what ends a sentence or opens a mention: "see app.rs.", "@app.rs".
            let (mut s, mut e) = (start, end);
            while e > s && b".,;:!?'\"".contains(&b[e - 1]) {
                e -= 1;
            }
            while s < e && b[s] == b'@' {
                s += 1;
            }
            if e - s < 2 {
                continue;
            }
            let candidate = &text[s..e];
            if !(candidate.contains('/') || candidate.contains('.')) || candidate.starts_with("//") {
                continue;
            }
            let Some(path) = self.resolve(candidate) else { continue };
            // A line goes with it only when nothing was trimmed between them.
            let (lines, tail) = if e == end { line_suffix(&text[end..]) } else { (None, 0) };
            found.push((s..if lines.is_some() { end + tail } else { e }, path, lines));
            i = end + tail;
        }
        found
    }
}

/// `:12`, `:12-20`, `:12:5`, `#L12`, `#L12-L20` at the start of `rest`: the lines, and how
/// much of `rest` they take.
fn line_suffix(rest: &str) -> (Option<(usize, usize)>, usize) {
    let digits = |s: &str, max: usize| -> usize { s.bytes().take(max + 1).take_while(u8::is_ascii_digit).count() };
    let number = |s: &str, n: usize| s[..n].parse::<usize>().ok();
    if let Some(after) = rest.strip_prefix(':') {
        let n = digits(after, 7);
        if n == 0 || n > 7 {
            return (None, 0);
        }
        let line = number(after, n).unwrap_or(0);
        let mut used = 1 + n;
        let mut end = line;
        let tail = &after[n..];
        for dash in ["-", "–"] {
            if let Some(more) = tail.strip_prefix(dash) {
                let m = digits(more, 7);
                if (1..=7).contains(&m) {
                    end = number(more, m).filter(|e| *e >= line).unwrap_or(line);
                    used += dash.len() + m;
                }
            }
        }
        if used == 1 + n {
            if let Some(more) = tail.strip_prefix(':') {
                let m = digits(more, 5);
                if (1..=5).contains(&m) {
                    used += 1 + m;
                }
            }
        }
        return (Some((line, end)), used);
    }
    if let Some(after) = rest.strip_prefix("#L") {
        let n = digits(after, 7);
        if n == 0 || n > 7 {
            return (None, 0);
        }
        let line = number(after, n).unwrap_or(0);
        let mut used = 2 + n;
        let mut end = line;
        let tail = &after[n..];
        if let Some(more) = tail.strip_prefix('-') {
            let more_l = more.strip_prefix('L').unwrap_or(more);
            let m = digits(more_l, 7);
            if (1..=7).contains(&m) {
                end = number(more_l, m).filter(|e| *e >= line).unwrap_or(line);
                used += 1 + (more.len() - more_l.len()) + m;
            }
        }
        return (Some((line, end)), used);
    }
    (None, 0)
}

/// `#12`, `!12` (a GitLab merge request), `api#12` or `acme/api#12`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueReference {
    pub range: Range<usize>,
    /// `owner/name` when the text spelled it out.
    pub path: Option<String>,
    /// A bare repo name, like `api` in `api#12`.
    pub repo_name: Option<String>,
    pub number: u64,
    pub merge_request: bool,
}

pub fn issue_references(text: &str) -> Vec<IssueReference> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let prefix_char = |c: u8| c.is_ascii_alphanumeric() || b"_.-/".contains(&c);
    let mut found = vec![];
    for i in 0..b.len() {
        if b[i] != b'#' && b[i] != b'!' {
            continue;
        }
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 || n > 7 || b.get(i + 1 + n).is_some_and(|c| word(*c)) {
            continue;
        }
        let mut start = i;
        while start > 0 && prefix_char(b[start - 1]) {
            start -= 1;
        }
        // What's before must not run on into the reference (`a/#1`, `&#12`, `x#1#2`).
        if start > 0 && b"#&".contains(&b[start - 1]) {
            continue;
        }
        let prefix = &text[start..i];
        let (path, repo_name) = match prefix.matches('/').count() {
            0 if prefix.is_empty() => (None, None),
            0 => (None, Some(prefix.to_string())),
            1 if !prefix.starts_with('/') && !prefix.ends_with('/') => (Some(prefix.to_string()), None),
            _ => continue,
        };
        let Ok(number) = text[i + 1..i + 1 + n].parse::<u64>() else { continue };
        found.push(IssueReference { range: start..i + 1 + n, path, repo_name, number, merge_request: b[i] == b'!' });
    }
    found
}

/// `http(s)://…` addresses written out, without what ends the sentence.
pub fn bare_urls(text: &str) -> Vec<(Range<usize>, String)> {
    let mut found = vec![];
    let mut from = 0;
    while let Some(at) = text[from..].find("://").map(|n| from + n) {
        from = at + 3;
        let start = if text[..at].ends_with("https") {
            at - 5
        } else if text[..at].ends_with("http") {
            at - 4
        } else {
            continue;
        };
        if start > 0 && text.as_bytes()[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let mut end = at + 3;
        for (n, c) in text[end..].char_indices() {
            if c.is_whitespace() || "<>()[]`\"".contains(c) {
                end = at + 3 + n;
                break;
            }
            end = at + 3 + n + c.len_utf8();
        }
        while end > at + 3 && ".,;:!?'".contains(text.as_bytes()[end - 1] as char) {
            end -= 1;
        }
        if end > at + 3 {
            found.push((start..end, text[start..end].to_string()));
            from = end;
        }
    }
    found
}

/// Links `range` to `url` unless a link is there already. Inside a code span only when
/// `in_code` (paths are named in code spans); a span around it is split around the link.
fn mark(styled: &mut Styled, range: Range<usize>, url: String, in_code: bool) {
    if range.is_empty() || styled.links.iter().any(|(r, _)| r.start < range.end && range.start < r.end) {
        return;
    }
    let overlapping: Vec<usize> =
        styled.spans.iter().enumerate().filter(|(_, (r, _))| r.start < range.end && range.start < r.end).map(|(n, _)| n).collect();
    let span = match overlapping.as_slice() {
        [] => Span::Link,
        [n] => {
            let (outer, kind) = styled.spans[*n].clone();
            if outer.start > range.start || outer.end < range.end || matches!(kind, Span::Link | Span::CodeLink) {
                return;
            }
            if kind == Span::Code && !in_code {
                return;
            }
            styled.spans.remove(*n);
            if outer.start < range.start {
                styled.spans.push((outer.start..range.start, kind));
            }
            if range.end < outer.end {
                styled.spans.push((range.end..outer.end, kind));
            }
            if kind == Span::Code { Span::CodeLink } else { Span::Link }
        }
        _ => return,
    };
    styled.spans.push((range.clone(), span));
    styled.spans.sort_by_key(|(r, _)| r.start);
    styled.links.push((range, url));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;

    fn linker() -> Linker {
        let files: Vec<String> = ["src/main.rs", "src/ui/rows.rs", "README.md", "a/lib.rs", "b/lib.rs"].iter().map(|s| s.to_string()).collect();
        Linker::new("/work/proj", Some(Repo { host: "github.com".into(), path: "acme/api".into(), gitlab: false }), &files)
    }

    fn links(source: &str) -> Vec<(String, String)> {
        let mut md = parse(source);
        linker().link_markdown(&mut md);
        let mut out = vec![];
        for block in &md.blocks {
            if let MdBlock::Paragraph(s) = block {
                for (r, url) in &s.links {
                    out.push((s.text[r.clone()].to_string(), url.clone()));
                }
            }
        }
        out
    }

    #[test]
    fn issues_link_to_the_repo() {
        assert_eq!(links("Fixed in #12."), vec![("#12".into(), "https://github.com/acme/api/issues/12".into())]);
        assert_eq!(links("See other/repo#7"), vec![("other/repo#7".into(), "https://github.com/other/repo/issues/7".into())]);
        assert_eq!(links("api#3"), vec![("api#3".into(), "https://github.com/acme/api/issues/3".into())]);
        assert!(links("Done!1 and web#4 and `#5` and a#b and x&#12;").is_empty());
    }

    #[test]
    fn gitlab_merge_requests() {
        let l = Linker::new("/w", Some(Repo { host: "gitlab.com".into(), path: "g/p".into(), gitlab: true }), &[]);
        let mut s = Styled { text: "see !4 and #5".into(), ..Default::default() };
        l.link(&mut s);
        let urls: Vec<&str> = s.links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(urls, ["https://gitlab.com/g/p/-/merge_requests/4", "https://gitlab.com/g/p/-/issues/5"]);
    }

    #[test]
    fn files_with_lines() {
        let found = links("Look at src/main.rs:12, rows.rs:3-9, `src/ui/rows.rs#L5-L7` and README.md.");
        assert_eq!(
            found,
            vec![
                ("src/main.rs:12".into(), "/work/proj/src/main.rs:12".into()),
                ("rows.rs:3-9".into(), "/work/proj/src/ui/rows.rs:3-9".into()),
                ("src/ui/rows.rs#L5-L7".into(), "/work/proj/src/ui/rows.rs:5-7".into()),
                ("README.md".into(), "/work/proj/README.md".into()),
            ]
        );
        // Ambiguous, outside the folder, or not a file: no link.
        assert!(links("lib.rs, /etc/hosts, other.rs, ../src/main.rs").is_empty());
        assert_eq!(links("/work/proj/a/lib.rs")[0].1, "/work/proj/a/lib.rs");
    }

    #[test]
    fn urls_and_existing_links() {
        let found = links("Open http://localhost:3000/app. or [docs](https://x.dev) #2");
        assert_eq!(found[0], ("docs".into(), "https://x.dev".into()));
        assert!(found.contains(&("http://localhost:3000/app".into(), "http://localhost:3000/app".into())));
        assert!(found.contains(&("#2".into(), "https://github.com/acme/api/issues/2".into())));
    }

    #[test]
    fn spans_stay_sorted_and_apart() {
        let mut md = parse("**see src/main.rs now** and `src/main.rs:4` too");
        linker().link_markdown(&mut md);
        let MdBlock::Paragraph(s) = &md.blocks[0] else { panic!() };
        for w in s.spans.windows(2) {
            assert!(w[0].0.end <= w[1].0.start, "{:?}", s.spans);
        }
        assert!(s.spans.iter().any(|(_, k)| *k == Span::CodeLink));
        assert_eq!(s.links.len(), 2);
    }
}
