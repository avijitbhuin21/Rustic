//! Simple-fullscreen chrome for the borderless macOS window. Simple fullscreen
//! auto-hides the menu bar, which slides down over the top of the screen when
//! the pointer touches the edge, so a black band the height of the menu bar is
//! reserved there and the webview starts below it. Screens with no menu bar
//! get rounded top corners instead. Windowed mode stays square and untouched.

use objc2::encode::{Encode, Encoding};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send, sel};

/// Top-corner radius in simple fullscreen on a screen with no menu bar band.
pub const FULLSCREEN_TOP_RADIUS: f64 = 16.0;

/// Used when the system hides the menu bar itself, so its height can't be measured.
const FALLBACK_MENU_BAR_HEIGHT: f64 = 24.0;
const MAX_MENU_BAR_HEIGHT: f64 = 64.0;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSEdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}

unsafe impl Encode for NSEdgeInsets {
    const ENCODING: Encoding = Encoding::Struct(
        "NSEdgeInsets",
        &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
    );
}

// CACornerMask bits (layer coordinate space).
const MIN_X_MIN_Y: usize = 1;
const MAX_X_MIN_Y: usize = 2;
const MIN_X_MAX_Y: usize = 4;
const MAX_X_MAX_Y: usize = 8;

/// Clip the window's top-left/top-right corners to `radius` (0 squares them again).
pub fn set_top_corner_radius<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, radius: f64) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
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

/// Height of the menu bar on the window's screen, or 0 when that screen has none.
/// Call before entering simple fullscreen: once the menu bar auto-hides it is no longer measurable.
pub fn menu_bar_height<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> f64 {
    let Ok(ns_window) = window.ns_window() else {
        return 0.0;
    };
    let ns_window = ns_window as *mut AnyObject;
    if ns_window.is_null() {
        return 0.0;
    }
    unsafe {
        let screen: *mut AnyObject = msg_send![ns_window, screen];
        if screen.is_null() {
            return 0.0;
        }
        let frame: CGRect = msg_send![screen, frame];
        let visible: CGRect = msg_send![screen, visibleFrame];
        let mut height =
            (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);

        // The notch's safe area, which the menu bar fills on built-in displays (macOS 12+).
        let has_safe_area: bool = msg_send![screen, respondsToSelector: sel!(safeAreaInsets)];
        if has_safe_area {
            let insets: NSEdgeInsets = msg_send![screen, safeAreaInsets];
            height = height.max(insets.top);
        }

        if height < 1.0 && screen_has_menu_bar(screen) {
            height = main_menu_bar_height().unwrap_or(FALLBACK_MENU_BAR_HEIGHT);
        }
        if height.is_finite() {
            height.clamp(0.0, MAX_MENU_BAR_HEIGHT)
        } else {
            0.0
        }
    }
}

/// Whether macOS shows a menu bar on `screen`: every screen when displays have separate Spaces, else only the primary.
unsafe fn screen_has_menu_bar(screen: *mut AnyObject) -> bool {
    let ns_screen: &AnyClass = class!(NSScreen);
    let separate_spaces: bool = msg_send![ns_screen, screensHaveSeparateSpaces];
    if separate_spaces {
        return true;
    }
    let screens: *mut AnyObject = msg_send![ns_screen, screens];
    if screens.is_null() {
        return true;
    }
    let primary: *mut AnyObject = msg_send![screens, firstObject];
    primary.is_null() || primary == screen
}

/// The app main menu's bar height, when there is a main menu.
unsafe fn main_menu_bar_height() -> Option<f64> {
    let ns_application: &AnyClass = class!(NSApplication);
    let app: *mut AnyObject = msg_send![ns_application, sharedApplication];
    if app.is_null() {
        return None;
    }
    let menu: *mut AnyObject = msg_send![app, mainMenu];
    if menu.is_null() {
        return None;
    }
    let height: f64 = msg_send![menu, menuBarHeight];
    (height >= 1.0).then_some(height)
}

/// Reserve a black band of `height` points above the webview (0 lets the webview fill the window again).
pub fn set_menu_bar_strip<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, height: f64) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window as *mut AnyObject;
    if ns_window.is_null() {
        return;
    }
    let Some(wk_web_view) = AnyClass::get(c"WKWebView") else {
        return;
    };
    unsafe {
        let content_view: *mut AnyObject = msg_send![ns_window, contentView];
        if content_view.is_null() {
            return;
        }
        let bounds: CGRect = msg_send![content_view, bounds];
        let flipped: bool = msg_send![content_view, isFlipped];
        let height = height.clamp(0.0, (bounds.size.height - 1.0).max(0.0));
        let frame = CGRect {
            origin: CGPoint {
                x: bounds.origin.x,
                y: bounds.origin.y + if flipped { height } else { 0.0 },
            },
            size: CGSize {
                width: bounds.size.width,
                height: bounds.size.height - height,
            },
        };

        // The webview keeps its width/height autoresizing mask, so the band
        // stays a fixed height across later window resizes.
        let subviews: *mut AnyObject = msg_send![content_view, subviews];
        if !subviews.is_null() {
            let count: usize = msg_send![subviews, count];
            for index in 0..count {
                let view: *mut AnyObject = msg_send![subviews, objectAtIndex: index];
                if view.is_null() {
                    continue;
                }
                let is_web_view: bool = msg_send![view, isKindOfClass: wk_web_view];
                if is_web_view {
                    let _: () = msg_send![view, setFrame: frame];
                }
            }
        }

        if height > 0.0 {
            let ns_color: &AnyClass = class!(NSColor);
            let black: *mut AnyObject = msg_send![ns_color, blackColor];
            let _: () = msg_send![ns_window, setBackgroundColor: black];
            let _: () = msg_send![ns_window, setOpaque: true];
        }
    }
}
