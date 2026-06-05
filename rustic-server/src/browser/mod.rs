//! Embedded VM browser (web/server build only).
//!
//! A real Chromium runs headless on the VM, co-located with the user's dev
//! servers so `http://localhost:3000` and `localhost` OAuth redirects just work.
//! [`BrowserManager`] owns its strict lifecycle; [`proxy`] reverse-proxies the
//! CDP WebSocket + DevTools frontend through the authed server port; [`cdp`] has
//! the one-shot control helpers and Chromium binary discovery.

pub mod cdp;
pub mod manager;
pub mod proxy;

pub use cdp::TabInfo;
pub use manager::{BrowserManager, CdpEndpoint};
