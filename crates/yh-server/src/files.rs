//! A thread's folder for the phone, read only (protocol 8): its tree a folder at a time, a file
//! a page of lines at a time with its colours, what changed since the last commit and each
//! change's diff, and the files a reply names. Nothing outside the folder is ever read: every
//! path is resolved, links followed, and must still be inside it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use yh_core::links::Linker;
use yh_core::markdown::{Carry, Span};
use yh_proto::{DirEntry, FilePage, FileRef, PullFile, Row};

/// What's read of a file: its first 8 MB, as the Mac's Files tab does.
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Lines longer than this are cut.
const LINE_LIMIT: usize = 2_000;
/// Lines in a page, unless asked otherwise, and at most.
const PAGE: u64 = 1_000;
const MAX_PAGE: u64 = 5_000;
/// Pictures sent whole up to this size.
const MAX_PICTURE: u64 = 6 * 1024 * 1024;
/// Changed files listed, at most.
const MAX_CHANGES: usize = 1_000;
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tif", "tiff", "ico"];

/// `path` (relative to `root`, or absolute inside it) as a real path inside `root`, links
/// followed; and how it's named relative to the root.
pub fn resolve(root: &str, path: &str) -> Result<(PathBuf, String), String> {
    let root = Path::new(root).canonicalize().map_err(|_| "The thread's folder isn't on the Mac any more.".to_string())?;
    let asked = Path::new(path.trim_start_matches("./"));
    let joined = if asked.is_absolute() { asked.to_path_buf() } else { root.join(asked) };
    let real = joined.canonicalize().map_err(|_| "That file isn't there any more.".to_string())?;
    let relative = real.strip_prefix(&root).map_err(|_| "That's outside the thread's folder.".to_string())?;
    Ok((real.clone(), relative.to_string_lossy().into_owned()))
}

/// A folder's entries, as the Mac's tree shows them: ignored ones marked when `listed` (what
/// git sees, folders with a `/`) is known.
pub fn list_dir(root: &str, path: &str, listed: Option<&HashSet<String>>) -> Result<Vec<DirEntry>, String> {
    let (real, relative) = resolve(root, if path.is_empty() { "." } else { path })?;
    if !real.is_dir() {
        return Err("That's not a folder.".into());
    }
    let prefix = if relative.is_empty() { String::new() } else { format!("{relative}/") };
    Ok(yh_core::files::list_dir(&real)
        .into_iter()
        .map(|(name, folder)| {
            let key = format!("{prefix}{name}{}", if folder { "/" } else { "" });
            let size = (!folder).then(|| std::fs::metadata(real.join(&name)).map(|m| m.len()).ok()).flatten();
            DirEntry { ignored: listed.is_some_and(|l| !l.contains(&key)), name, folder, size }
        })
        .collect())
}

/// A file read and split into lines, kept while it's unchanged.
struct Read {
    modified: f64,
    size: u64,
    kind: &'static str,
    lines: Arc<Vec<String>>,
    carries: Arc<Vec<Carry>>,
    note: Option<String>,
}

/// The last few files read, so paging through one reads it once.
#[derive(Default)]
pub struct Reads(Mutex<Vec<(PathBuf, Arc<Read>)>>);

fn modified_ms(m: &std::fs::Metadata) -> f64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_millis() as f64)
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

impl Reads {
    /// A page of `path`: `count` lines from `from`, coloured; empty and `unchanged` when it
    /// changed no later than `known`.
    pub fn page(&self, root: &str, path: &str, from: Option<u64>, count: Option<u64>, known: Option<f64>) -> Result<FilePage, String> {
        let (real, _) = resolve(root, path)?;
        let meta = std::fs::metadata(&real).map_err(|_| "That file isn't there any more.".to_string())?;
        if meta.is_dir() {
            return Err("That's a folder.".into());
        }
        let modified = modified_ms(&meta);
        if known == Some(modified) {
            return Ok(FilePage { kind: String::new(), size: meta.len(), modified, unchanged: true, ..Default::default() });
        }
        let ext = extension(&real);
        if IMAGES.contains(&ext.as_str()) {
            let data = (meta.len() <= MAX_PICTURE).then(|| std::fs::read(&real).ok().map(|b| yh_core::attachments::base64(&b))).flatten();
            let note = data.is_none().then(|| "This picture is too big to send.".to_string());
            return Ok(FilePage { kind: "image".into(), size: meta.len(), modified, data, note, ..Default::default() });
        }
        let read = self.read(&real, &meta, modified)?;
        let total = read.lines.len() as u64;
        let from = from.unwrap_or(0).min(total);
        let count = count.unwrap_or(PAGE).clamp(1, MAX_PAGE);
        let end = (from + count).min(total);
        let mut spans = Vec::with_capacity((end - from) as usize);
        for i in from..end {
            let line = &read.lines[i as usize];
            spans.push(colours(line, &ext, read.carries.get(i as usize).copied().unwrap_or_default()));
        }
        Ok(FilePage {
            kind: read.kind.into(),
            size: read.size,
            modified,
            total,
            from,
            lines: read.lines[from as usize..end as usize].to_vec(),
            spans,
            note: read.note.clone(),
            data: None,
            unchanged: false,
        })
    }

    fn read(&self, real: &Path, meta: &std::fs::Metadata, modified: f64) -> Result<Arc<Read>, String> {
        if let Some((_, r)) = self.0.lock().unwrap().iter().find(|(p, r)| p == real && r.modified == modified && r.size == meta.len()) {
            return Ok(r.clone());
        }
        use std::io::Read as _;
        let file = std::fs::File::open(real).map_err(|e| format!("It couldn't be read: {e}"))?;
        let mut bytes = vec![];
        file.take(MAX_FILE).read_to_end(&mut bytes).map_err(|e| format!("It couldn't be read: {e}"))?;
        let size = meta.len();
        let read = if bytes.iter().take(8000).any(|b| *b == 0) {
            Read { modified, size, kind: "binary", lines: Arc::new(vec![]), carries: Arc::new(vec![]), note: None }
        } else {
            let text = String::from_utf8_lossy(&bytes);
            // A last newline ends the last line; it doesn't start another.
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let mut cut = false;
            let lines: Vec<String> = if text.is_empty() {
                vec![]
            } else {
                text.split('\n')
                    .map(|l| {
                        let l = l.strip_suffix('\r').unwrap_or(l);
                        if l.len() <= LINE_LIMIT {
                            return l.replace('\t', "    ");
                        }
                        cut = true;
                        let mut end = LINE_LIMIT;
                        while !l.is_char_boundary(end) {
                            end -= 1;
                        }
                        l[..end].replace('\t', "    ")
                    })
                    .collect()
            };
            let mut notes = vec![];
            if size > MAX_FILE {
                notes.push(format!("Showing the first {} MB of {} MB.", MAX_FILE >> 20, size >> 20));
            }
            if cut {
                notes.push(format!("Lines over {LINE_LIMIT} characters are cut."));
            }
            let carries = yh_core::markdown::line_carries(&lines, &extension(real));
            Read { modified, size, kind: "text", lines: Arc::new(lines), carries: Arc::new(carries), note: (!notes.is_empty()).then(|| notes.join(" ")) }
        };
        let read = Arc::new(read);
        let mut held = self.0.lock().unwrap();
        held.retain(|(p, _)| p != real);
        held.push((real.to_path_buf(), read.clone()));
        if held.len() > 6 {
            held.remove(0);
        }
        Ok(read)
    }
}

/// A line's colours as the wire has them: UTF-16 ranges, kind 0 keyword, 1 string, 2 comment, 3 number.
fn colours(line: &str, ext: &str, carry: Carry) -> Vec<[u32; 3]> {
    let styled = yh_core::markdown::highlight_line(line, ext, carry).0;
    let ascii = line.is_ascii();
    let utf16 = |byte: usize| if ascii { byte as u32 } else { line[..byte].encode_utf16().count() as u32 };
    styled
        .spans
        .into_iter()
        .filter_map(|(r, span)| {
            let kind = match span {
                Span::Keyword => 0,
                Span::Str => 1,
                Span::Comment => 2,
                Span::Number => 3,
                _ => return None,
            };
            Some([utf16(r.start), utf16(r.end), kind])
        })
        .collect()
}

fn git(root: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("/usr/bin/git").args(args).current_dir(root).output().map_err(|e| format!("git couldn't run: {e}"))
}

/// What to compare with: the last commit, or nothing (an empty tree) before the first.
fn base(root: &str) -> &'static str {
    let has_head = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok_and(|o| o.status.success());
    if has_head { "HEAD" } else { "4b825dc642cb6eb9a060e54bf8d69288fbee4904" }
}

/// What changed since the last commit, staged or not, new files included; and the branch.
pub fn changes(root: &str) -> Result<(Vec<PullFile>, Option<String>), String> {
    let status = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames"])?;
    if !status.status.success() {
        return Err("This thread's folder isn't a git repository.".into());
    }
    let mut files: Vec<PullFile> = vec![];
    for entry in status.stdout.split(|b| *b == 0).filter(|e| e.len() > 3) {
        let (x, y) = (entry[0], entry[1]);
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        let status = match (x, y) {
            (b'?', b'?') | (b'A', _) => "added",
            (b'D', _) | (_, b'D') => "removed",
            _ => "modified",
        };
        files.push(PullFile { path, previous_path: None, status: status.into(), additions: 0, deletions: 0, has_patch: true });
        if files.len() >= MAX_CHANGES {
            break;
        }
    }
    // Lines added and removed: tracked files from `git diff`, new ones counted.
    let numstat = git(root, &["diff", base(root), "--numstat", "-z", "--no-renames"])?;
    let mut counts: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
    for entry in numstat.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        let mut parts = text.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else { continue };
        counts.insert(p.to_string(), (a.parse().ok(), d.parse().ok()));
    }
    for f in &mut files {
        match counts.get(&f.path) {
            Some((Some(a), Some(d))) => (f.additions, f.deletions) = (*a, *d),
            // Binary: no diff to show.
            Some(_) => f.has_patch = false,
            None if f.status == "added" => match text_lines(&Path::new(root).join(&f.path)) {
                Some(n) => f.additions = n,
                None => f.has_patch = false,
            },
            None => {}
        }
    }
    let branch = git(root, &["branch", "--show-current"]).ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|b| !b.is_empty());
    Ok((files, branch))
}

/// How many lines a new file has, when it's text and not too big to diff.
fn text_lines(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    Some(bytes.iter().filter(|b| **b == b'\n').count() as u64 + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n")))
}

/// A changed file's hunks against the last commit (a new one's whole), header left out.
pub fn change_patch(root: &str, path: &str) -> Result<String, String> {
    // The file may be gone (removed): checked against the folder by name, not resolved.
    if path.starts_with('/') || path.split('/').any(|p| p == "..") {
        return Err("That's outside the thread's folder.".into());
    }
    let tracked = git(root, &["ls-files", "--error-unmatch", "--", path]).is_ok_and(|o| o.status.success());
    let out = if tracked {
        git(root, &["diff", base(root), "--no-renames", "--no-color", "--", path])?
    } else {
        resolve(root, path)?;
        git(root, &["diff", "--no-index", "--no-color", "--", "/dev/null", path])?
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if text.lines().any(|l| l.starts_with("Binary files ")) {
        return Err("It's not text, so there's no diff to show.".into());
    }
    Ok(text.find("\n@@").map_or_else(|| if text.starts_with("@@") { text.to_string() } else { String::new() }, |at| text[at + 1..].to_string()))
}

/// Each thread folder's file list and `Linker`, kept a while and read again aside once stale.
#[derive(Default)]
pub struct Folders {
    held: Mutex<HashMap<String, Held>>,
}

#[derive(Clone)]
struct Held {
    at: Instant,
    files: Arc<Vec<String>>,
    linker: Arc<Linker>,
    /// What git doesn't ignore, when it's a repo.
    listed: Option<Arc<HashSet<String>>>,
    refreshing: bool,
}

const FRESH: Duration = Duration::from_secs(30);

impl Folders {
    fn get(self: &Arc<Self>, folder: &str) -> Held {
        let found = self.held.lock().unwrap().get(folder).cloned();
        match found {
            Some(h) if h.at.elapsed() < FRESH => h,
            // Stale: used as it is while it's read again aside.
            Some(h) => {
                if !h.refreshing {
                    if let Some(h) = self.held.lock().unwrap().get_mut(folder) {
                        h.refreshing = true;
                    }
                    let (me, folder) = (self.clone(), folder.to_string());
                    std::thread::spawn(move || {
                        let fresh = read_folder(&folder);
                        me.held.lock().unwrap().insert(folder, fresh);
                    });
                }
                h
            }
            None => {
                let fresh = read_folder(folder);
                self.held.lock().unwrap().insert(folder.to_string(), fresh.clone());
                fresh
            }
        }
    }

    /// The folder's files, folders with a `/` (for `@` and Go to File).
    pub fn files(self: &Arc<Self>, folder: &str) -> Arc<Vec<String>> {
        self.get(folder).files
    }

    /// What git sees in the folder, for dimming the rest.
    pub fn listed(self: &Arc<Self>, folder: &str) -> Option<Arc<HashSet<String>>> {
        self.get(folder).listed
    }

    /// The thread's files each reply in `rows` names, as the Mac links them.
    pub fn link_rows(self: &Arc<Self>, folder: &str, rows: &mut [Row]) {
        if !rows.iter().any(|r| r.kind == "assistant") {
            return;
        }
        let linker = self.get(folder).linker;
        for row in rows.iter_mut().filter(|r| r.kind == "assistant") {
            row.files = refs(&linker, &row.text);
        }
    }
}

fn read_folder(folder: &str) -> Held {
    let files = yh_core::files::list(Path::new(folder));
    let in_repo = Path::new(folder).join(".git").exists() || git(folder, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|o| o.status.success());
    let listed = (in_repo && files.len() < 50_000).then(|| Arc::new(files.iter().cloned().collect::<HashSet<String>>()));
    let linker = Arc::new(Linker::new(folder, None, &files));
    Held { at: Instant::now(), files: Arc::new(files), linker, listed, refreshing: false }
}

/// The files `text` names, once each as written.
fn refs(linker: &Linker, text: &str) -> Vec<FileRef> {
    let mut out: Vec<FileRef> = vec![];
    for (range, path, lines) in linker.file_references(text) {
        let written = &text[range];
        if out.iter().any(|r| r.text == written) {
            continue;
        }
        out.push(FileRef { text: written.to_string(), path, line: lines.map(|l| l.0 as u32), end: lines.map(|l| l.1 as u32).filter(|e| Some(*e) != lines.map(|l| l.0 as u32)) });
        if out.len() >= 200 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yh-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn nothing_outside_the_folder() {
        let dir = scratch("outside");
        std::fs::write(dir.join("src/a.rs"), "fn main() {}\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        assert_eq!(resolve(&root, "src/a.rs").unwrap().1, "src/a.rs");
        assert_eq!(resolve(&root, &format!("{root}/src/a.rs")).unwrap().1, "src/a.rs");
        assert!(resolve(&root, "../../etc/passwd").is_err());
        assert!(resolve(&root, "/etc/passwd").is_err());
        std::os::unix::fs::symlink("/etc", dir.join("etc")).unwrap();
        assert!(resolve(&root, "etc/hosts").is_err());
        assert!(change_patch(&root, "../x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pages_lines_with_colours() {
        let dir = scratch("pages");
        let text: String = (0..2500).map(|i| format!("let x{i} = \"é\"; // n\n")).collect();
        std::fs::write(dir.join("src/a.rs"), &text).unwrap();
        let root = dir.to_string_lossy().to_string();
        let reads = Reads::default();
        let page = reads.page(&root, "src/a.rs", Some(2000), None, None).unwrap();
        assert_eq!((page.kind.as_str(), page.total, page.from, page.lines.len()), ("text", 2500, 2000, 500));
        assert_eq!(page.lines[0], "let x2000 = \"é\"; // n");
        // `let` a keyword; the string and comment in UTF-16 units (é is one).
        assert!(page.spans[0].contains(&[0, 3, 0]));
        assert!(page.spans[0].contains(&[12, 15, 1]));
        assert!(page.spans[0].contains(&[17, 21, 2]));
        let again = reads.page(&root, "src/a.rs", Some(0), Some(1), Some(page.modified)).unwrap();
        assert!(again.unchanged && again.lines.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changes_and_their_diffs() {
        let dir = scratch("changes");
        let root = dir.to_string_lossy().to_string();
        let run = |args: &[&str]| assert!(git(&root, args).unwrap().status.success(), "{args:?}");
        run(&["init", "-q"]);
        std::fs::write(dir.join("src/a.rs"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "x\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "first"]);
        std::fs::write(dir.join("src/a.rs"), "one\n2\nthree\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("new.md"), "# hi\nthere").unwrap();
        let (files, _) = changes(&root).unwrap();
        let find = |p: &str| files.iter().find(|f| f.path == p).cloned().unwrap();
        assert_eq!((find("src/a.rs").status.as_str(), find("src/a.rs").additions, find("src/a.rs").deletions), ("modified", 2, 1));
        assert_eq!(find("gone.txt").status, "removed");
        assert_eq!((find("new.md").status.as_str(), find("new.md").additions), ("added", 2));
        let patch = change_patch(&root, "src/a.rs").unwrap();
        assert!(patch.starts_with("@@") && patch.contains("+three"), "{patch}");
        assert!(change_patch(&root, "new.md").unwrap().contains("+# hi"));
        assert!(change_patch(&root, "gone.txt").unwrap().contains("-x"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replies_link_the_files_they_name() {
        let dir = scratch("links");
        std::fs::write(dir.join("src/app.rs"), "").unwrap();
        let folders = Arc::new(Folders::default());
        let mut rows = vec![Row { kind: "assistant".into(), text: "See `src/app.rs:12-20` and app.rs, not other.rs.".into(), ..Default::default() }];
        folders.link_rows(&dir.to_string_lossy(), &mut rows);
        assert_eq!(rows[0].files.len(), 2);
        assert_eq!(rows[0].files[0], FileRef { text: "src/app.rs:12-20".into(), path: "src/app.rs".into(), line: Some(12), end: Some(20) });
        assert_eq!(rows[0].files[1].text, "app.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
