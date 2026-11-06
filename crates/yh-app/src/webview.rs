//! A WKWebView placed over the GPUI window: the agents' browser. GPUI draws everything else
//! itself; a web page can't be, so it's a native view laid over the spot GPUI keeps for it
//! (`set_frame` with the bounds GPUI laid out). Results of what's asked of the page come
//! back on the main thread, from WebKit's completion blocks, through channels.

#![allow(unexpected_cfgs, deprecated)]

use block::ConcreteBlock;
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use std::ffi::CStr;

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

fn ns_string(text: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(text) }
}

fn rust_string(s: id) -> Option<String> {
    if s == nil {
        return None;
    }
    unsafe {
        let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
        (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

pub struct WebView {
    view: id,
}

impl WebView {
    /// A web view in the window whose GPUI view is `host`, hidden until it's given a place.
    /// Its cookies and storage persist (the default data store), per app.
    pub fn new(host: id) -> WebView {
        unsafe {
            let config: id = msg_send![class!(WKWebViewConfiguration), new];
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
            let view: id = msg_send![class!(WKWebView), alloc];
            let view: id = msg_send![view, initWithFrame: frame configuration: config];
            let _: () = msg_send![config, release];
            let _: () = msg_send![view, setHidden: YES];
            // Safari's user agent: sites serve the page a Mac browser gets.
            let _: () = msg_send![view, setCustomUserAgent: ns_string("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")];
            let _: () = msg_send![host, addSubview: view];
            WebView { view }
        }
    }

    /// Where it shows, in the host view's top-left coordinates (GPUI's); `None` hides it.
    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) {
        unsafe {
            let Some((x, y, w, h)) = bounds else {
                let _: () = msg_send![self.view, setHidden: YES];
                return;
            };
            let host: id = msg_send![self.view, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let host_frame: NSRect = msg_send![host, bounds];
            let y = if flipped { y } else { host_frame.size.height - y - h };
            let _: () = msg_send![self.view, setFrame: NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))];
            let _: () = msg_send![self.view, setHidden: NO];
        }
    }

    pub fn load(&self, url: &str) {
        unsafe {
            let ns_url: id = msg_send![class!(NSURL), URLWithString: ns_string(url)];
            if ns_url == nil {
                return;
            }
            let request: id = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
            let _: id = msg_send![self.view, loadRequest: request];
        }
    }

    pub fn url(&self) -> Option<String> {
        unsafe {
            let url: id = msg_send![self.view, URL];
            if url == nil {
                return None;
            }
            rust_string(msg_send![url, absoluteString])
        }
    }

    pub fn title(&self) -> Option<String> {
        unsafe { rust_string(msg_send![self.view, title]) }
    }

    pub fn is_loading(&self) -> bool {
        unsafe { msg_send![self.view, isLoading] }
    }

    pub fn go_back(&self) {
        unsafe {
            let _: id = msg_send![self.view, goBack];
        }
    }

    pub fn reload(&self) {
        unsafe {
            let _: id = msg_send![self.view, reload];
        }
    }

    /// Runs JavaScript in the page; the reply is its result as JSON text (the script should
    /// return `JSON.stringify(...)`), or the error.
    pub fn evaluate(&self, script: &str, reply: flume::Sender<Result<String, String>>) {
        unsafe {
            let block = ConcreteBlock::new(move |result: id, error: id| {
                let answer = if error != nil {
                    let text: id = msg_send![error, localizedDescription];
                    Err(rust_string(text).unwrap_or_else(|| "JavaScript error".into()))
                } else if result == nil {
                    Ok("null".to_string())
                } else {
                    let is_string: bool = msg_send![result, isKindOfClass: class!(NSString)];
                    if is_string {
                        Ok(rust_string(result).unwrap_or_default())
                    } else {
                        let text: id = msg_send![result, description];
                        Ok(rust_string(text).unwrap_or_default())
                    }
                };
                let _ = reply.send(answer);
            });
            let block = block.copy();
            let _: () = msg_send![self.view, evaluateJavaScript: ns_string(script) completionHandler: &*block];
        }
    }

    /// What it shows now, as a PNG at `path`; the reply says whether it was written.
    pub fn snapshot(&self, path: String, reply: flume::Sender<Result<(), String>>) {
        unsafe {
            let config: id = msg_send![class!(WKSnapshotConfiguration), new];
            let block = ConcreteBlock::new(move |image: id, error: id| {
                if error != nil || image == nil {
                    let _ = reply.send(Err("couldn't take a screenshot".into()));
                    return;
                }
                let tiff: id = msg_send![image, TIFFRepresentation];
                let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
                let props: id = msg_send![class!(NSDictionary), dictionary];
                // NSBitmapImageFileTypePNG = 4
                let png: id = msg_send![rep, representationUsingType: 4usize properties: props];
                let ok: bool = msg_send![png, writeToFile: ns_string(&path) atomically: YES];
                let _ = reply.send(if ok { Ok(()) } else { Err("couldn't write the screenshot".into()) });
            });
            let block = block.copy();
            let _: () = msg_send![self.view, takeSnapshotWithConfiguration: config completionHandler: &*block];
            let _: () = msg_send![config, release];
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.view, removeFromSuperview];
            let _: () = msg_send![self.view, release];
        }
    }
}
