//! Application core. This crate must not depend on GPUI.

mod arrows;
mod layers;
mod pen;
pub use layers::DrawAnnotation;
pub use pen::{MAX_DOCUMENT_PEN_POINTS, MAX_PEN_POINTS, PenAnnotation};
mod capture;
pub use arrows::ArrowAnnotation;
mod clipboard;
#[cfg(target_os = "windows")]
mod desktop_events;
#[cfg(target_os = "windows")]
mod single_instance;
#[cfg(target_os = "windows")]
pub use desktop_events::{DesktopEvent, desktop_events, request_reopen};
#[cfg(target_os = "windows")]
pub use single_instance::SingleInstance;
mod blur;
mod text;
pub use text::TextAnnotation;
mod history;
mod pixelation;
pub use blur::blur_preview_bgra;
mod selection;
pub use pixelation::PixelationGrid;
mod settings;
pub use history::{
    HistoryEntry, ImportReport, capture_history_page, history_database_path, import_capture_files,
    record_saved_capture, remove_history_entry,
};
pub use selection::{
    FrameAnnotation, PixelRegion, crop_snapshot, edit_snapshot, redact_snapshot,
    render_annotated_snapshot, render_document_snapshot, render_edited_snapshot,
    render_filtered_snapshot, render_ordered_snapshot, render_pixelated_snapshot,
    render_text_snapshot,
};
pub use settings::CaptureSettings;

pub use clipboard::{ImageClipboard, load_saved_png};

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
pub use desktop_window::{DesktopWindow, cursor_position, wait_for_desktop_compositor};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CaptureRequest, CapturedImage, DisplayInfo,
    MAX_CAPTURE_BYTES, PixelFormat, WindowInfo, XcapBackend, capture_output_dir, export_saved_png,
    validate_image_size,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
