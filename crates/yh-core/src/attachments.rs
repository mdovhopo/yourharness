//! Images you send: scaled to what the API takes (2000 px on the long side, under 5 MB) and
//! kept in the data folder's `attachments/`, as the Swift app does; sent as base64 image blocks.
//! Any other file or folder goes by its path (`FileAttachment`, `files_note`).

use std::io::Cursor;
use std::path::{Path, PathBuf};

const MAX_SIDE: u32 = 2000;
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// Image bytes in any common format as PNG (or JPEG when PNG would be too big), scaled down to
/// the API's limits. `None` if it isn't an image.
pub fn prepare(bytes: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    let mut img = image::load_from_memory(bytes).ok()?;
    if img.width().max(img.height()) > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3);
    }
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    if png.len() <= MAX_BYTES {
        return Some((png, "png"));
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut Cursor::new(&mut jpeg), 85).encode_image(&img.to_rgb8()).ok()?;
    Some((jpeg, "jpeg"))
}

/// Prepares and saves an image for a thread; returns its name in `attachments/`.
pub fn save(data_dir: &Path, thread: &str, bytes: &[u8]) -> Option<String> {
    let (data, ext) = prepare(bytes)?;
    let dir = data_dir.join("attachments");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("{thread}-{}.{ext}", crate::library::new_uuid());
    std::fs::write(dir.join(&name), data).ok()?;
    Some(name)
}

pub fn path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join("attachments").join(name)
}

/// What the API calls the image's type, from its name.
pub fn media_type(name: &str) -> &'static str {
    if name.ends_with(".jpeg") || name.ends_with(".jpg") {
        "image/jpeg"
    } else if name.ends_with(".gif") {
        "image/gif"
    } else if name.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    }
}

pub fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "tiff" | "tif" | "bmp")
    )
}

/// A file or folder sent with a message (not an image Claude sees inline): Claude is told
/// where it is. Files are copied into `attachments/` so the thread keeps them; folders and
/// files over `MAX_COPY_BYTES` are referred to where they are, as the Swift app does.
#[derive(Clone, Debug, PartialEq)]
pub struct FileAttachment {
    /// Where it was picked from.
    pub path: PathBuf,
    pub is_dir: bool,
    /// Bytes; None for a folder.
    pub size: Option<u64>,
}

/// Bigger files, and folders, are referred to where they are rather than copied.
pub const MAX_COPY_BYTES: u64 = 200_000_000;

/// The tag around what Claude is told about attached files, so a session's history gives back
/// the message as typed.
pub const FILES_TAG: &str = "attached-files";

impl FileAttachment {
    pub fn from_path(path: &Path) -> Option<FileAttachment> {
        let meta = std::fs::metadata(path).ok()?;
        let is_dir = meta.is_dir();
        Some(FileAttachment { path: path.to_path_buf(), is_dir, size: (!is_dir).then_some(meta.len()) })
    }

    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    pub fn is_copied(&self) -> bool {
        !self.is_dir && self.size.unwrap_or(0) <= MAX_COPY_BYTES
    }

    /// Its name in the message's `attachments`, as the Swift app keeps it: a copy's path inside
    /// `attachments/` (`<thread>-files/<uuid>/<name>`, deleted with the thread), else the
    /// absolute path (a folder's ending in `/`).
    pub fn stored_name(&self, thread: &str) -> String {
        if self.is_copied() {
            format!("{thread}-files/{}/{}", crate::library::new_uuid(), self.name())
        } else {
            format!("{}{}", self.path.to_string_lossy(), if self.is_dir { "/" } else { "" })
        }
    }

    /// Done before the message goes to Claude; the path Claude is told. One that can't be copied
    /// is read where it is.
    pub fn copy_in(&self, data_dir: &Path, name: &str) -> String {
        if name.starts_with('/') {
            return self.path.to_string_lossy().into_owned();
        }
        let destination = path(data_dir, name);
        let copied = destination.parent().is_some_and(|p| std::fs::create_dir_all(p).is_ok())
            && (destination.exists() || std::fs::copy(&self.path, &destination).is_ok());
        if copied {
            destination.to_string_lossy().into_owned()
        } else {
            self.path.to_string_lossy().into_owned()
        }
    }
}

/// What follows your message for Claude: where each attached file is (`(path, is folder)`).
/// Empty without any.
pub fn files_note(paths: &[(String, bool)]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = paths.iter().map(|(p, dir)| format!("- {p}{}", if *dir { "/ (folder)" } else { "" })).collect();
    format!(
        "<{FILES_TAG}>\nAttached (open them with Read, which takes text, PDFs and images; use a command only for other formats):\n{}\n</{FILES_TAG}>",
        lines.join("\n")
    )
}

/// An attachment's name in a message is an image Claude saw inline (not a file sent by path).
pub fn is_image_name(name: &str) -> bool {
    !name.contains('/') && is_image_path(Path::new(name))
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn claude_is_told_where_each_file_is() {
        let note = files_note(&[("/tmp/a/report.pdf".into(), false), ("/tmp/project".into(), true)]);
        assert_eq!(
            note,
            "<attached-files>\nAttached (open them with Read, which takes text, PDFs and images; use a command only for other formats):\n- /tmp/a/report.pdf\n- /tmp/project/ (folder)\n</attached-files>"
        );
        assert!(files_note(&[]).is_empty());
    }

    #[test]
    fn folders_and_big_files_stay_where_they_are() {
        let file = |is_dir, size| FileAttachment { path: "/tmp/x".into(), is_dir, size };
        assert!(file(false, Some(10)).is_copied());
        assert!(!file(true, None).is_copied());
        assert!(!file(false, Some(MAX_COPY_BYTES + 1)).is_copied());
        assert_eq!(file(true, None).stored_name("t"), "/tmp/x/");
        assert!(file(false, Some(10)).stored_name("t").starts_with("t-files/"));
        assert!(file(false, Some(10)).stored_name("t").ends_with("/x"));
        assert!(is_image_name("t-1.png") && !is_image_name("t-files/u/a.png") && !is_image_name("/a/b.png"));
    }

    #[test]
    fn files_are_copied_into_the_thread() {
        let dir = std::env::temp_dir().join(format!("yh-attach-{}", crate::library::new_uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("log.txt");
        std::fs::write(&source, "hello").unwrap();
        let file = FileAttachment::from_path(&source).unwrap();
        assert_eq!(file.size, Some(5));
        let name = file.stored_name("thread");
        let told = file.copy_in(&dir.join("data"), &name);
        assert_eq!(std::fs::read_to_string(&told).unwrap(), "hello");
        assert!(told.contains("/attachments/thread-files/"));
        let folder = FileAttachment::from_path(&dir).unwrap();
        assert!(folder.is_dir);
        assert_eq!(folder.copy_in(&dir, &folder.stored_name("thread")), dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn big_images_are_scaled_down() {
        let img = image::RgbImage::from_pixel(3000, 1000, image::Rgb([200, 30, 30]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let (out, ext) = prepare(&bytes).unwrap();
        assert_eq!(ext, "png");
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (2000, 667));
        assert!(prepare(b"not an image").is_none());
    }
}
