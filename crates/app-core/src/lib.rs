//! Application core. This crate must not depend on GPUI.

mod capture;
mod clipboard;
#[cfg(target_os = "windows")]
mod desktop_events;
#[cfg(target_os = "windows")]
pub use desktop_events::{DesktopEvent, desktop_events};
mod selection;
pub use selection::{PixelRegion, crop_snapshot};

pub use clipboard::ImageClipboard;

#[cfg(target_os = "windows")]
mod hotkey;
#[cfg(target_os = "windows")]
pub use hotkey::{CaptureCommand, CaptureHotkey};

#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
pub use tray::{SpikeTray, TrayCommand};

#[cfg(target_os = "windows")]
mod desktop_window;
#[cfg(target_os = "windows")]
pub use desktop_window::{DesktopWindow, wait_for_desktop_compositor};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CaptureRequest, CapturedImage, DisplayInfo,
    MAX_CAPTURE_BYTES, PixelFormat, WindowInfo, XcapBackend, capture_output_dir,
    validate_image_size,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
