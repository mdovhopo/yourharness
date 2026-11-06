//! Images you send: scaled to what the API takes (2000 px on the long side, under 5 MB) and
//! kept in the data folder's `attachments/`, as the Swift app does; sent as base64 image blocks.

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
