//! A video's first frame as a picture, for its tile in the transcript (the Swift app's
//! `AVAssetImageGenerator` thumbnail): made once, aside, kept in the data folder's `thumbnails/`.

#![allow(unexpected_cfgs, deprecated)]

use cocoa::base::{id, nil};
use cocoa::foundation::NSString;
use objc::{class, msg_send, sel, sel_impl};
use std::path::{Path, PathBuf};

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

/// Where a video's thumbnail goes (it may not be made yet).
pub fn path_for(video: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    video.hash(&mut h);
    std::fs::metadata(video).ok().and_then(|m| m.modified().ok()).hash(&mut h);
    yh_core::library::data_directory().join("thumbnails").join(format!("{:016x}.png", h.finish()))
}

/// Makes the thumbnail (blocking: aside). True when it's there.
pub fn make(video: &str, out: &Path) -> bool {
    if out.exists() {
        return true;
    }
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let done = first_frame(video, out);
        let _: () = msg_send![pool, drain];
        done
    }
}

fn first_frame(video: &str, out: &Path) -> bool { unsafe {
    let path = NSString::alloc(nil).init_str(video);
    let url: id = msg_send![class!(NSURL), fileURLWithPath: path];
    let asset: id = msg_send![class!(AVURLAsset), URLAssetWithURL: url options: nil];
    if asset == nil {
        return false;
    }
    let generator: id = msg_send![class!(AVAssetImageGenerator), assetImageGeneratorWithAsset: asset];
    if generator == nil {
        return false;
    }
    // Upright, as it plays; no bigger than a tile needs.
    let _: () = msg_send![generator, setAppliesPreferredTrackTransform: true];
    let _: () = msg_send![generator, setMaximumSize: CGSize { width: 720., height: 720. }];
    let zero = CMTime { value: 0, timescale: 1, flags: 1, epoch: 0 };
    let mut actual = zero;
    let mut error: id = nil;
    let image: *mut std::ffi::c_void = msg_send![generator, copyCGImageAtTime: zero actualTime: &mut actual error: &mut error];
    if image.is_null() {
        return false;
    }
    let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
    let rep: id = msg_send![rep, initWithCGImage: image];
    unsafe extern "C" {
        fn CGImageRelease(image: *mut std::ffi::c_void);
    }
    CGImageRelease(image);
    if rep == nil {
        return false;
    }
    // NSBitmapImageFileTypePNG
    let data: id = msg_send![rep, representationUsingType: 4u64 properties: nil];
    let file = NSString::alloc(nil).init_str(&out.to_string_lossy());
    let ok: bool = data != nil && msg_send![data, writeToFile: file atomically: true];
    let _: () = msg_send![rep, release];
    let _: () = msg_send![file, release];
    let _: () = msg_send![path, release];
    ok
}}
