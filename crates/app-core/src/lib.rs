//! Application core. This crate must not depend on GPUI.

mod arrows;
mod layers;
mod pen;
pub use layers::{DrawAnnotation, HighlightAnnotation};
pub use pen::{MAX_DOCUMENT_PEN_POINTS, MAX_PEN_POINTS, PenAnnotation};
mod capture;
mod capture_catalog;
mod capture_native;
mod native_capture_job;
pub use native_capture_job::{
    NativeCaptureCall, NativeCaptureCancellation, NativeCaptureCheck, NativeCaptureExecutor,
    NativeCaptureReply, NativeSourceInventory, install_native_capture_executor,
    validate_native_frames,
};
mod capture_options;
mod image_cancel;
mod raster_ops;
pub use capture_catalog::{
    CaptureCatalog, CaptureMonitorBinding, CaptureMonitorIdentity, CaptureRegionTemplate,
    CaptureTemplateGeometry, PreparedRegionTemplate, capture_monitor_identities,
};
pub use capture_options::{
    CaptureCursor, CaptureOptions, CaptureWindowFrame, CaptureWindowShadow,
    CaptureWindowTransparency,
};
pub use raster_ops::{
    canvas_rgba_with_cancel, composite_rgba_with_cancel, flip_rgba_with_cancel,
    resize_nearest_rgba_with_cancel, rotate_rgba_with_cancel,
};
mod desktop_composite;
pub use desktop_composite::{capture_stitched_desktop, stitch_desktop};
mod resample;
pub use resample::{ExportSize, downscale_export, downscale_export_with_cancel};
mod filenames;
pub use arrows::ArrowAnnotation;
pub use filenames::{capture_file_name, validate_capture_name_template};
mod clipboard;
#[cfg(target_os = "windows")]
mod clipboard_windows;
#[cfg(target_os = "windows")]
pub use clipboard_windows::read_clipboard_image;
mod export;
pub use export::{ExportReport, export_saved_pngs, export_saved_pngs_with_metadata};
#[cfg(target_os = "windows")]
mod desktop_events;
#[cfg(target_os = "windows")]
mod single_instance;
#[cfg(target_os = "windows")]
pub use desktop_events::{DesktopEvent, desktop_events, request_reopen};
#[cfg(target_os = "windows")]
pub use single_instance::SingleInstance;
mod blur;
mod bmp_export;
mod bmp_input;
mod gif_assembly;
mod history_thumbnail;
mod scrolling;
mod strip;
pub use bmp_export::save_bmp;
pub use gif_assembly::assemble_gif;
pub use scrolling::{ScrollAppend, ScrollAssembler, ScrollMatchOptions};
pub use strip::{remove_image_strip, remove_image_strip_with_cancel};
mod tiff_export;
mod tiff_input;
pub use tiff_export::save_tiff;
mod text;
pub use text::TextAnnotation;
mod history;
mod history_query;
pub use history_query::{
    HistoryPage, HistoryQuery, HistorySort, query_capture_history, query_upload_history,
};
mod storage;
pub use storage::{
    HistoryHealth, PartInspection, backup_history_database, check_history_database,
    inspect_part_files,
};
mod pixelation;
mod webp_lossy;
pub use blur::blur_preview_bgra;
pub use webp_lossy::save_lossy_webp;
mod selection;
pub use pixelation::PixelationGrid;
mod oauth_provider;
mod sharex;
mod sharex_process;
pub use sharex::{
    ClassifiedUploadProfile, ShareXEvent, ShareXInputKind, ShareXOutcome, ShareXPrepared,
    classify_upload_profile,
};
mod provider_credentials;
pub use oauth_provider::{OAuthEvent, OAuthOutcome, OAuthProfile};
mod s3_credentials;
pub use provider_credentials::{
    read_provider_credentials, remove_provider_credentials, save_provider_credentials,
};
mod s3_upload;
mod settings;
mod upload;
mod upload_credentials;
pub use history::transfer_history_file;
pub use history::{
    HistoryEntry, HistoryFilter, ImportReport, MoveOutcome, MoveRecovery, PendingMove,
    UploadHistoryEntry, capture_history_page, capture_history_page_filtered, history_database_path,
    import_capture_files, lookup_history_entry, move_history_file, pending_history_move,
    record_saved_capture, record_uploaded_png, recover_history_move, relink_history_entry,
    remove_history_entries, remove_history_entry, remove_upload_history_entry,
    search_capture_history, upload_history_page,
};
pub use history::{IdentityIndexReport, prime_history_file_identities};
pub use history::{
    RetentionPlan, RetentionPolicy, RetentionReport, TrashEntry, TrashPage, apply_retention,
    plan_retention, purge_history_trash, recover_history_file_operation,
    remove_missing_history_entries, restore_history_trash, trash_history_file, trash_history_page,
};
pub use s3_credentials::{read_s3_credentials, remove_s3_credentials, save_s3_credentials};
pub use s3_upload::{
    S3Credentials, S3Outcome, S3Profile, upload_s3_file, upload_s3_file_with_cancellation,
};
pub use selection::{
    FrameAnnotation, PixelRegion, crop_snapshot, edit_snapshot, redact_snapshot,
    render_annotated_snapshot, render_document_snapshot, render_edited_snapshot,
    render_filtered_snapshot, render_ordered_snapshot, render_ordered_snapshot_with_cancel,
    render_pixelated_snapshot, render_text_snapshot,
};
pub use settings::{
    CaptureHotkeys, CaptureOutputProfile, CaptureSettings, DEFAULT_JPEG_QUALITY, RegionPreset,
    UiTheme,
};
pub use upload::{
    NamedUploadDestination, UploadAuthentication, UploadCancellation, UploadConfiguration,
    UploadDestination, UploadDestinationCatalog, UploadMethod, UploadOutcome, UploadProgress,
    UploadProgressReceiver, UploadProgressReporter, probe_upload, upload_file, upload_png,
    upload_png_bytes_with_progress, upload_png_with_progress, upload_progress_channel,
};

pub use clipboard::{
    ImageClipboard, load_editor_image, load_editor_image_with_limit, load_png_thumbnail,
    load_saved_png,
};

#[cfg(target_os = "windows")]
mod hotkey;
#[cfg(target_os = "windows")]
pub use hotkey::{CaptureCommand, CaptureHotkey};

#[cfg(target_os = "windows")]
mod native_appearance;
#[cfg(target_os = "windows")]
pub use native_appearance::apply_native_menus;
#[cfg(target_os = "windows")]
mod tray;
#[cfg(target_os = "windows")]
pub use tray::{SpikeTray, TrayCommand};

#[cfg(target_os = "windows")]
mod desktop_window;
#[cfg(target_os = "windows")]
pub use desktop_window::{
    DesktopWindow, cursor_position, foreground_display_capture, foreground_window_capture,
    wait_for_desktop_compositor,
};

pub use capture::{
    CaptureBackend, CaptureCapabilities, CaptureError, CaptureRequest, CapturedImage,
    DisplayGeometry, DisplayInfo, ImageExportFormat, MAX_CAPTURE_BYTES, PixelFormat,
    RememberedRegion, WindowInfo, XcapBackend, capture_output_dir, export_saved_png,
    export_saved_png_as, png_without_metadata, save_png_bytes, validate_image_size,
};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
