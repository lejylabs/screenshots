//! Application core. This crate must not depend on GPUI.

mod capture;
mod clipboard;

pub use clipboard::ImageClipboard;

#[cfg(target_os = "windows")]
mod hotkey;
#[cfg(target_os = "windows")]
pub use hotkey::CaptureHotkey;

#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
pub use tray::{SpikeTray, TrayCommand};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CapturedImage, DisplayInfo, PixelFormat,
    WindowInfo, XcapBackend, capture_output_dir,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
