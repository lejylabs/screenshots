//! Application core. This crate must not depend on GPUI.

mod arrows;
mod layers;
mod pen;
pub use layers::{DrawAnnotation, HighlightAnnotation};
pub use pen::{MAX_DOCUMENT_PEN_POINTS, MAX_PEN_POINTS, PenAnnotation};
mod capture;
mod filenames;
pub use filenames::{capture_file_name, validate_capture_name_template};
pub use arrows::ArrowAnnotation;
mod clipboard;
#[cfg(target_os = "windows")]
mod clipboard_windows;
#[cfg(target_os = "windows")]
pub use clipboard_windows::read_clipboard_image;
mod export;
pub use export::{ExportReport, export_saved_pngs};
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
mod upload;
mod upload_credentials;
pub use history::{
    HistoryEntry, HistoryFilter, ImportReport, UploadHistoryEntry, capture_history_page,
    capture_history_page_filtered, history_database_path, import_capture_files,
    record_saved_capture, record_uploaded_png, relink_history_entry, remove_history_entries,
    remove_history_entry, remove_upload_history_entry, search_capture_history, upload_history_page,
};
pub use selection::{
    FrameAnnotation, PixelRegion, crop_snapshot, edit_snapshot, redact_snapshot,
    render_annotated_snapshot, render_document_snapshot, render_edited_snapshot,
    render_filtered_snapshot, render_ordered_snapshot, render_pixelated_snapshot,
    render_text_snapshot,
};
pub use settings::{CaptureHotkeys, CaptureOutputProfile, CaptureSettings, RegionPreset, UiTheme, DEFAULT_JPEG_QUALITY};
pub use upload::{
    UploadCancellation, UploadDestination, UploadProgress, UploadProgressReceiver,
    UploadProgressReporter, upload_png, upload_png_with_progress, upload_progress_channel,
};

pub use clipboard::{ImageClipboard, load_editor_image, load_png_thumbnail, load_saved_png};

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
pub use desktop_window::{DesktopWindow, cursor_position, foreground_window_capture, wait_for_desktop_compositor};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CaptureRequest, CapturedImage, DisplayInfo, DisplayGeometry, RememberedRegion,
    MAX_CAPTURE_BYTES, PixelFormat, WindowInfo, XcapBackend, capture_output_dir, export_saved_png, export_saved_png_as, ImageExportFormat,
    validate_image_size,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
