//! Is this machine's screen locked? Peer sync refuses incoming transfer and
//! pairing requests while it is, so nothing moves on an unattended machine.
//!
//! Every platform check fails *open* (reports unlocked) when the OS can't be
//! asked, so a detection problem never blocks normal use.

/// True when the interactive session is locked (lock screen showing).
pub fn is_locked() -> bool {
    imp::is_locked()
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_void, CString};

    type CFTypeRef = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const i8, encoding: u32) -> CFTypeRef;
        fn CFBooleanGetValue(b: CFTypeRef) -> u8;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFRelease(cf: CFTypeRef);
    }

    const UTF8: u32 = 0x0800_0100;

    /// `CGSSessionScreenIsLocked` is present (and true) in the session dictionary while locked.
    pub fn is_locked() -> bool {
        // SAFETY: plain CoreFoundation calls; every Copy/Create result is released,
        // and the Get result is only read while the dictionary is alive.
        unsafe {
            let dict = CGSessionCopyCurrentDictionary();
            if dict.is_null() {
                return false;
            }
            let Ok(name) = CString::new("CGSSessionScreenIsLocked") else {
                CFRelease(dict);
                return false;
            };
            let key = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8);
            let locked = if key.is_null() {
                false
            } else {
                let v = CFDictionaryGetValue(dict, key);
                let on = !v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v) != 0;
                CFRelease(key);
                on
            };
            CFRelease(dict);
            locked
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;

    const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;

    #[link(name = "user32")]
    extern "system" {
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> *mut c_void;
        fn SwitchDesktop(desktop: *mut c_void) -> i32;
        fn CloseDesktop(desktop: *mut c_void) -> i32;
    }

    /// While locked the input desktop is the secure Winlogon desktop: opening or
    /// switching to it from the user session fails.
    pub fn is_locked() -> bool {
        // SAFETY: the handle from OpenInputDesktop is checked for null and always closed.
        unsafe {
            let desk = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
            if desk.is_null() {
                return true;
            }
            let switched = SwitchDesktop(desk) != 0;
            CloseDesktop(desk);
            !switched
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    /// systemd-logind's `LockedHint` for the current session (set by GNOME/KDE lockers).
    pub fn is_locked() -> bool {
        let session = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "self".into());
        std::process::Command::new("loginctl")
            .args(["show-session", &session, "-p", "LockedHint", "--value"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
            .unwrap_or(false)
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn is_locked() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lock_check_runs_without_panicking() {
        // The test runner's session is normally unlocked; mostly this proves the FFI links.
        let _ = super::is_locked();
    }
}
