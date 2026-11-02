//! Rounded top corners for the borderless macOS window while in simple
//! fullscreen, so the UI follows the display's curved top edge. Windowed mode
//! stays square. The content view's layer is clipped directly because titled
//! windows bring native chrome the user rejected.

use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};

/// Top-corner radius in simple fullscreen, following the display's rounded top corners.
pub const FULLSCREEN_TOP_RADIUS: f64 = 16.0;

// CACornerMask bits (layer coordinate space).
const MIN_X_MIN_Y: usize = 1;
const MAX_X_MIN_Y: usize = 2;
const MIN_X_MAX_Y: usize = 4;
const MAX_X_MAX_Y: usize = 8;

/// Clip the window's top-left/top-right corners to `radius` (0 squares them again).
pub fn set_top_corner_radius<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, radius: f64) {
    let Ok(ns_window) = window.ns_window() else { return };
    let ns_window = ns_window as *mut AnyObject;
    if ns_window.is_null() {
        return;
    }
    unsafe {
        let content_view: *mut AnyObject = msg_send![ns_window, contentView];
        if content_view.is_null() {
            return;
        }
        let _: () = msg_send![content_view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![content_view, layer];
        if layer.is_null() {
            return;
        }
        // A flipped layer has its MinY edge at the top of the screen.
        let flipped: bool = msg_send![layer, isGeometryFlipped];
        let top_corners = if flipped {
            MIN_X_MIN_Y | MAX_X_MIN_Y
        } else {
            MIN_X_MAX_Y | MAX_X_MAX_Y
        };
        let _: () = msg_send![layer, setCornerRadius: radius];
        let _: () = msg_send![layer, setMaskedCorners: top_corners];
        let _: () = msg_send![layer, setMasksToBounds: radius > 0.0];

        // The clipped corners must reveal the desktop, not the window background.
        let ns_color: &AnyClass = class!(NSColor);
        let clear: *mut AnyObject = msg_send![ns_color, clearColor];
        let _: () = msg_send![ns_window, setOpaque: false];
        let _: () = msg_send![ns_window, setBackgroundColor: clear];
        let _: () = msg_send![ns_window, invalidateShadow];
    }
}
