//! Application core. This crate must not depend on GPUI.

mod capture;
mod clipboard;
mod selection;
pub use selection::{PixelRegion, crop_snapshot};

pub use clipboard::ImageClipboard;

#[cfg(target_os = "windows")]
mod hotkey;
#[cfg(target_os = "windows")]
pub use hotkey::CaptureHotkey;

#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
pub use tray::{SpikeTray, TrayCommand};

#[cfg(target_os = "windows")]
mod desktop_window;
#[cfg(target_os = "windows")]
pub use desktop_window::{DesktopWindow, wait_for_desktop_compositor};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CapturedImage, DisplayInfo, PixelFormat,
    WindowInfo, XcapBackend, capture_output_dir,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
