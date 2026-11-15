//! Where a page shows, for either engine: a container view over the spot GPUI keeps for it,
//! whose bounds are the page's CSS pixels and frame its points, so a viewport bigger than the
//! panel is drawn scaled; the engine's own view inside it, at CSS size; parked off screen when
//! no panel shows it; and the overlay that marks where the agent acts.

use super::{ns_string, PARKED_SCALE};
use cocoa::base::{id, nil, NO, YES};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use objc::{class, msg_send, sel, sel_impl};
use std::cell::Cell;

pub struct Page {
    /// What sits over the window: it draws the page scaled when the page's viewport is
    /// bigger than the panel (its bounds are CSS pixels, its frame points).
    pub page: id,
    /// The engine's view (a WKWebView, or the view Chromium's browser is made in).
    pub view: id,
    /// The page's viewport in CSS pixels; None fits the panel.
    viewport: Cell<Option<(f64, f64)>>,
    /// While a full-page screenshot is taken: the view is this many CSS pixels tall, the page
    /// still laid out at the viewport's size (WebKit; Chromium captures beyond the viewport).
    full_height: Cell<Option<f64>>,
    /// Where it was last given a place (`set_frame`), to lay it out again.
    bounds: Cell<Option<(f64, f64, f64, f64)>>,
    /// Over the page, under nothing that takes the mouse: where the agent just acted, ringed,
    /// and what it's doing, labelled (`show_activity`). Not part of the page, so screenshots
    /// and snapshots never see it. Made on first use.
    overlay: Cell<id>,
    activity: Cell<u64>,
    /// In the parking window (`parking_view`), off screen, rather than over a panel.
    parked: Cell<bool>,
    /// The engine's view sits beside the container, not in it, at its frame in points (Chromium
    /// sizes its page by what it covers on screen, and scales it itself: `layout`). The
    /// container is then only the overlay's, and lets the mouse through.
    beside: bool,
}

impl Page {
    /// `view` in a new container over `host`, both hidden until given a place, laid out at the
    /// laptop size.
    pub fn new(host: id, view: id) -> Page {
        Page::make(host, view, false)
    }

    /// `view` beside a new container over `host` (`beside`).
    pub fn new_beside(host: id, view: id) -> Page {
        Page::make(host, view, true)
    }

    fn make(host: id, view: id, beside: bool) -> Page {
        unsafe {
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
            let class = if beside { super::pass_through_class() } else { class!(NSView) };
            let page: id = msg_send![class, alloc];
            let page: id = msg_send![page, initWithFrame: frame];
            let _: () = msg_send![page, setHidden: YES];
            if beside {
                let _: () = msg_send![view, setHidden: YES];
                let _: () = msg_send![host, addSubview: view];
            } else {
                let _: () = msg_send![view, setHidden: NO];
                let _: () = msg_send![page, addSubview: view];
            }
            let _: () = msg_send![host, addSubview: page];
            let p = Page {
                page,
                view,
                viewport: Cell::new(Some(super::LAPTOP)),
                full_height: Default::default(),
                bounds: Default::default(),
                overlay: Cell::new(nil),
                activity: Default::default(),
                parked: Default::default(),
                beside,
            };
            p.lay_out(super::LAPTOP.0, super::LAPTOP.1, super::LAPTOP.0, super::LAPTOP.1);
            p
        }
    }

    /// Where it is, for test runs: its window's title, shown or hidden, its frame.
    pub fn report_place(&self) -> String {
        unsafe {
            let window: id = msg_send![self.page, window];
            let title = if window == nil {
                "no window".to_string()
            } else {
                let t: id = msg_send![window, title];
                super::rust_string(t).unwrap_or_default()
            };
            let hidden: bool = msg_send![self.page, isHidden];
            let frame: NSRect = msg_send![self.page, frame];
            let state = if self.parked.get() { "parked, hidden off screen," } else if hidden { "hidden" } else { "shown" };
            format!("in window \"{title}\" {state} at {:.0},{:.0} {:.0}x{:.0}", frame.origin.x, frame.origin.y, frame.size.width, frame.size.height)
        }
    }

    /// Where the page shows over `window`, in its top-left points, when it's shown there (not
    /// parked, not hidden, not in a window of its own): for demo recordings to draw it in.
    pub fn shown_in(&self, window: id) -> Option<(f64, f64, f64, f64)> {
        unsafe {
            let hidden: bool = msg_send![self.page, isHidden];
            let in_window: id = msg_send![self.page, window];
            if self.parked.get() || hidden || in_window != window {
                return None;
            }
            let host: id = msg_send![self.page, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let host_frame: NSRect = msg_send![host, bounds];
            let f: NSRect = msg_send![self.page, frame];
            let top = if flipped { f.origin.y } else { host_frame.size.height - f.origin.y - f.size.height };
            Some((f.origin.x, top, f.size.width, f.size.height))
        }
    }

    /// Moves it over another GPUI view (the browser's own window, or back), keeping the page.
    pub fn move_to(&self, host: id) {
        unsafe {
            let current: id = msg_send![self.page, superview];
            if current == host || host == nil {
                return;
            }
            // Held while it has no superview, which would let it go.
            let _: () = msg_send![self.page, retain];
            let _: () = msg_send![self.page, removeFromSuperview];
            if self.beside {
                let _: () = msg_send![self.view, retain];
                let _: () = msg_send![self.view, removeFromSuperview];
                let _: () = msg_send![host, addSubview: self.view];
                let _: () = msg_send![self.view, release];
            }
            let _: () = msg_send![host, addSubview: self.page];
            let _: () = msg_send![self.page, release];
        }
    }

    /// The engine's view beside the container: in the same view, at its frame, shown with it.
    fn follow(&self) {
        if !self.beside {
            return;
        }
        unsafe {
            let host: id = msg_send![self.page, superview];
            let current: id = msg_send![self.view, superview];
            if current != host && host != nil {
                let _: () = msg_send![self.view, retain];
                let _: () = msg_send![self.view, removeFromSuperview];
                // NSWindowBelow: under the container, whose overlay goes over the page.
                let _: () = msg_send![host, addSubview: self.view positioned: -1isize relativeTo: self.page];
                let _: () = msg_send![self.view, release];
            }
            let frame: NSRect = msg_send![self.page, frame];
            let now: NSRect = msg_send![self.view, frame];
            if now.origin.x != frame.origin.x || now.origin.y != frame.origin.y || now.size.width != frame.size.width || now.size.height != frame.size.height {
                let _: () = msg_send![self.view, setFrame: frame];
            }
            let hidden: bool = msg_send![self.page, isHidden];
            let _: () = msg_send![self.view, setHidden: hidden];
        }
    }

    /// The page's frame in points and its CSS size: what Chromium is told to scale (`beside`).
    pub fn layout(&self) -> (f64, f64, f64, f64) {
        unsafe {
            let f: NSRect = msg_send![self.page, frame];
            let b: NSRect = msg_send![self.page, bounds];
            (f.size.width, f.size.height, b.size.width, b.size.height)
        }
    }

    /// Where it shows, in the host view's top-left coordinates (GPUI's); `None` parks it (the
    /// page keeps its viewport). A viewport bigger than the panel is drawn scaled to fit,
    /// against the top.
    pub fn set_frame(&self, bounds: Option<(f64, f64, f64, f64)>) {
        self.bounds.set(bounds);
        unsafe {
            let Some((x, y, w, h)) = bounds else {
                // Parked: off screen in a window of its own, where the engine keeps laying it
                // out, running its timers and animation frames and drawing screenshots (hidden in
                // the main window, it got no frames, and every action waited on them). It keeps
                // its viewport's size: a screenshot needs no panel.
                let (cw, ch) = self.viewport.get().unwrap_or(self.size());
                let ch = self.full_height.get().unwrap_or(ch);
                let parking = super::parking_view();
                if parking != nil {
                    self.move_to(parking);
                    self.parked.set(true);
                    self.lay_out(cw * PARKED_SCALE, ch * PARKED_SCALE, cw, ch);
                    let _: () = msg_send![self.page, setFrameOrigin: NSPoint::new(0., 0.)];
                    let _: () = msg_send![self.page, setHidden: NO];
                } else {
                    let _: () = msg_send![self.page, setHidden: YES];
                    self.lay_out(cw, ch, cw, ch);
                }
                self.follow();
                return;
            };
            self.parked.set(false);
            let (cw, ch) = self.viewport.get().unwrap_or((w, h));
            let scale = (w / cw).min(h / ch);
            let (fw, fh) = (cw * scale, ch * scale);
            // A full-page shot: as tall as the page, from the same top-left.
            let (css_h, frame_h) = match self.full_height.get() {
                Some(full) => (full, full * scale),
                None => (ch, fh),
            };
            let host: id = msg_send![self.page, superview];
            let flipped: bool = msg_send![host, isFlipped];
            let host_frame: NSRect = msg_send![host, bounds];
            let left = x + (w - fw) / 2.;
            let bottom = if flipped { y } else { host_frame.size.height - y - frame_h };
            let _: () = msg_send![self.page, setFrame: NSRect::new(NSPoint::new(left, bottom), NSSize::new(fw, frame_h))];
            self.lay_out(fw, frame_h, cw, css_h);
            let _: () = msg_send![self.page, setHidden: NO];
        }
        self.follow();
    }

    /// The view `height` CSS pixels tall for a full-page screenshot, or back (None).
    pub fn hold_full_height(&self, height: Option<f64>) {
        self.full_height.set(height);
        self.set_frame(self.bounds.get());
    }

    /// The viewport's size in CSS pixels, whatever the view is now.
    pub fn size_for_layout(&self) -> (f64, f64) {
        match (self.viewport.get(), self.bounds.get()) {
            (Some(v), _) => v,
            (None, Some((_, _, w, h))) => (w, h),
            (None, None) => self.size(),
        }
    }

    /// The page view `frame` points big, the engine's view `css` points: a CSS pixel is a point,
    /// as in Safari, drawn scaled (page zoom would hold text to the engine's minimum size).
    fn lay_out(&self, fw: f64, fh: f64, cw: f64, ch: f64) {
        unsafe {
            let current: NSRect = msg_send![self.page, frame];
            if current.size.width != fw || current.size.height != fh {
                let _: () = msg_send![self.page, setFrameSize: NSSize::new(fw, fh)];
            }
            let _: () = msg_send![self.page, setBoundsSize: NSSize::new(cw, ch)];
            if !self.beside {
                let _: () = msg_send![self.view, setFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(cw, ch))];
            }
            let overlay = self.overlay.get();
            if overlay != nil {
                let _: () = msg_send![overlay, setFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(cw, ch))];
            }
        }
    }

    /// The page's viewport in CSS pixels, None to fit the panel; laid out at the next frame.
    pub fn set_viewport(&self, viewport: Option<(f64, f64)>) {
        self.viewport.set(viewport);
        if let Some((w, h)) = viewport {
            // Parked or hidden, it still takes the size, for a screenshot without the panel.
            let hidden: bool = unsafe { msg_send![self.page, isHidden] };
            if self.parked.get() {
                self.lay_out(w * PARKED_SCALE, h * PARKED_SCALE, w, h);
            } else if hidden {
                self.lay_out(w, h, w, h);
            }
            self.follow();
        }
    }

    pub fn viewport(&self) -> Option<(f64, f64)> {
        self.viewport.get()
    }

    /// Its size in CSS pixels (the container's bounds, which the engine lays the page out in).
    pub fn size(&self) -> (f64, f64) {
        unsafe {
            let b: NSRect = msg_send![self.page, bounds];
            (b.size.width, b.size.height)
        }
    }

    /// On screen, over a panel or the browser's window (not parked).
    pub fn is_shown(&self) -> bool {
        unsafe {
            let hidden: bool = msg_send![self.page, isHidden];
            !hidden && !self.parked.get()
        }
    }

    /// The overlay over the page, made the first time it's needed.
    fn ensure_overlay(&self) -> id {
        unsafe {
            let existing = self.overlay.get();
            if existing != nil {
                return existing;
            }
            let b: NSRect = msg_send![self.page, bounds];
            let overlay: id = msg_send![super::pass_through_class(), alloc];
            let overlay: id = msg_send![overlay, initWithFrame: b];
            let _: () = msg_send![overlay, setWantsLayer: YES];
            let _: () = msg_send![self.page, addSubview: overlay];
            self.overlay.set(overlay);
            overlay
        }
    }

    /// What the agent is doing (`label`: Click, Type, Scroll…), over the page's bottom edge, and
    /// where (CSS pixels), ringed. Returns its number, for
    /// `clear_activity` to take down this one and not a newer one.
    pub fn show_activity(&self, label: &str, rect: Option<(f64, f64, f64, f64)>) -> u64 {
        let seq = self.activity.get() + 1;
        self.activity.set(seq);
        unsafe {
            let overlay = self.ensure_overlay();
            let layer: id = msg_send![overlay, layer];
            if layer == nil {
                return seq;
            }
            let _: () = msg_send![layer, setSublayers: nil];
            let (cw, ch) = self.size();
            let page_frame: NSRect = msg_send![self.page, frame];
            // Screen points per CSS pixel: the ring and the label keep their size on screen.
            let zoom = if cw > 0. && page_frame.size.width > 0. { (page_frame.size.width / cw).clamp(0.05, 4.) } else { 1. };
            let window: id = msg_send![self.page, window];
            let backing: f64 = if window == nil { 2. } else { msg_send![window, backingScaleFactor] };
            let accent: id = msg_send![class!(NSColor), controlAccentColor];
            let cg = |c: id| -> id { msg_send![c, CGColor] };
            let add = |l: id| {
                let _: () = msg_send![layer, addSublayer: l];
                super::fade_in(l);
            };
            if let Some((x, y, w, h)) = rect {
                let (w, h) = (w.max(10. / zoom), h.max(10. / zoom));
                let ring: id = msg_send![class!(CALayer), layer];
                let _: () = msg_send![ring, setFrame: NSRect::new(NSPoint::new(x, ch - y - h), NSSize::new(w, h))];
                let _: () = msg_send![ring, setBorderWidth: 2. / zoom];
                let _: () = msg_send![ring, setBorderColor: cg(accent)];
                let soft: id = msg_send![accent, colorWithAlphaComponent: 0.12f64];
                let _: () = msg_send![ring, setBackgroundColor: cg(soft)];
                let _: () = msg_send![ring, setCornerRadius: 4. / zoom];
                add(ring);
            }
            if !label.is_empty() {
                let size = 11. / zoom;
                let font: id = msg_send![class!(NSFont), systemFontOfSize: size weight: 0.23f64];
                let attrs: id = msg_send![class!(NSDictionary), dictionaryWithObject: font forKey: ns_string("NSFont")];
                let text = ns_string(label);
                let measured: NSSize = msg_send![text, sizeWithAttributes: attrs];
                let (tw, th) = (measured.width.ceil(), measured.height.ceil());
                let (pad_x, pad_y) = (10. / zoom, 5. / zoom);
                let (pw, ph) = (tw + pad_x * 2., th + pad_y * 2.);
                let pill: id = msg_send![class!(CALayer), layer];
                let _: () = msg_send![pill, setFrame: NSRect::new(NSPoint::new(((cw - pw) / 2.).max(0.), 8. / zoom), NSSize::new(pw, ph))];
                let back: id = msg_send![class!(NSColor), windowBackgroundColor];
                let back: id = msg_send![back, colorWithAlphaComponent: 0.94f64];
                let _: () = msg_send![pill, setBackgroundColor: cg(back)];
                let _: () = msg_send![pill, setCornerRadius: ph / 2.];
                let line: id = msg_send![class!(NSColor), separatorColor];
                let _: () = msg_send![pill, setBorderColor: cg(line)];
                let _: () = msg_send![pill, setBorderWidth: 1. / zoom];
                let words: id = msg_send![class!(CATextLayer), layer];
                let _: () = msg_send![words, setFrame: NSRect::new(NSPoint::new(pad_x, pad_y), NSSize::new(tw, th))];
                let _: () = msg_send![words, setString: text];
                let _: () = msg_send![words, setFont: font];
                let _: () = msg_send![words, setFontSize: size];
                let ink: id = msg_send![class!(NSColor), labelColor];
                let _: () = msg_send![words, setForegroundColor: cg(ink)];
                let _: () = msg_send![words, setAlignmentMode: ns_string("center")];
                let _: () = msg_send![words, setContentsScale: (backing * zoom).clamp(0.5, 8.)];
                let _: () = msg_send![pill, addSublayer: words];
                add(pill);
            }
        }
        seq
    }

    /// Takes the overlay down, unless a newer one is up.
    pub fn clear_activity(&self, seq: u64) {
        if self.activity.get() != seq {
            return;
        }
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return;
            }
            let layer: id = msg_send![overlay, layer];
            if layer != nil {
                let _: () = msg_send![layer, setSublayers: nil];
            }
        }
    }

    /// Whether the overlay shows anything now, for test runs.
    pub fn shows_activity(&self) -> bool {
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return false;
            }
            let layer: id = msg_send![overlay, layer];
            let subs: id = if layer == nil { nil } else { msg_send![layer, sublayers] };
            let n: usize = if subs == nil { 0 } else { msg_send![subs, count] };
            n > 0
        }
    }

    /// The overlay alone drawn into a PNG (its CSS-pixel size), for test runs: what the user sees
    /// over the page, which the page's own screenshots leave out.
    pub fn overlay_png(&self, path: &str) -> bool {
        unsafe {
            let overlay = self.overlay.get();
            if overlay == nil {
                return false;
            }
            let layer: id = msg_send![overlay, layer];
            if layer == nil {
                return false;
            }
            let (w, h) = self.size();
            let (pw, ph) = (w.round() as isize, h.round() as isize);
            let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
            let rep: id = msg_send![rep, initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>() pixelsWide: pw pixelsHigh: ph bitsPerSample: 8isize samplesPerPixel: 4isize hasAlpha: YES isPlanar: NO colorSpaceName: ns_string("NSDeviceRGBColorSpace") bytesPerRow: 0isize bitsPerPixel: 0isize];
            let context: id = msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: rep];
            let cg: *mut std::ffi::c_void = msg_send![context, CGContext];
            let _: () = msg_send![layer, renderInContext: cg];
            let props: id = msg_send![class!(NSDictionary), dictionary];
            let png: id = msg_send![rep, representationUsingType: 4usize properties: props];
            let ok: bool = msg_send![png, writeToFile: ns_string(path) atomically: YES];
            let _: () = msg_send![rep, release];
            ok
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        unsafe {
            if self.beside {
                let _: () = msg_send![self.view, removeFromSuperview];
            }
            let _: () = msg_send![self.page, removeFromSuperview];
            let overlay = self.overlay.get();
            if overlay != nil {
                let _: () = msg_send![overlay, release];
            }
            let _: () = msg_send![self.page, release];
        }
    }
}
