//! `yh` on your PATH (docs/cli.md): a link to the copy inside this app's bundle, made from
//! Settings → General. The first of Homebrew's folder, `/usr/local/bin` and `~/.local/bin` that
//! you can write to gets it; a `yh` there that isn't ours is never replaced.

use std::path::{Path, PathBuf};

/// Where a link may go, in the order tried.
fn folders() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), Path::new(&home).join(".local/bin")]
}

/// The `yh` inside this app, when it's a bundle that has one.
pub fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let yh = exe.parent()?.join("yh");
    (exe.ancestors().nth(3).is_some_and(|b| b.extension().is_some_and(|e| e == "app")) && yh.is_file()).then_some(yh)
}

/// A link to some YourHarness's `yh` (this copy's or another's, stable or Next).
fn is_ours(link: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(link).ok()?;
    let s = target.to_string_lossy();
    (s.ends_with(".app/Contents/MacOS/yh") && s.contains("YourHarness")).then_some(target)
}

/// The link on your PATH and the app it leads to, when there is one.
pub fn installed() -> Option<(PathBuf, PathBuf)> {
    folders().into_iter().map(|f| f.join("yh")).find_map(|link| is_ours(&link).map(|t| (link, t)))
}

/// Links this app's `yh` into the first folder that takes it (replacing a link to another
/// YourHarness); says where.
pub fn install() -> Result<PathBuf, String> {
    let yh = bundled().ok_or("This copy of the app has no command line in it.")?;
    if let Some((link, _)) = installed() {
        std::fs::remove_file(&link).map_err(|e| format!("Can't replace {}: {e}", link.display()))?;
    }
    let mut last = String::new();
    for folder in folders() {
        let home_folder = folder.ends_with(".local/bin");
        if home_folder {
            let _ = std::fs::create_dir_all(&folder);
        }
        if !folder.is_dir() {
            continue;
        }
        let link = folder.join("yh");
        if link.symlink_metadata().is_ok() {
            last = format!("{} is another program; yh went elsewhere.", link.display());
            continue;
        }
        match std::os::unix::fs::symlink(&yh, &link) {
            Ok(()) => return Ok(link),
            Err(e) => last = format!("{}: {e}", folder.display()),
        }
    }
    Err(format!("Couldn't link yh anywhere on your PATH. {last}"))
}

pub fn uninstall() -> Result<(), String> {
    match installed() {
        Some((link, _)) => std::fs::remove_file(&link).map_err(|e| format!("Can't remove {}: {e}", link.display())),
        None => Ok(()),
    }
}

/// Whether your login shell finds what's in `folder` (its PATH as the app read it).
pub fn on_path(folder: &Path) -> bool {
    let path = yh_core::shell_env::loaded_path().or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
    path.split(':').any(|p| Path::new(p) == folder)
}
