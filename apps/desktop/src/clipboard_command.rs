//! Explicit clipboard content only; no passive viewer or startup access.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    #[cfg(unix)]
    if args.first().is_some_and(|arg| arg == "copy-file") {
        return crate::file_clipboard::command(&args[1..]);
    }
    let command = args
        .first()
        .and_then(|v| v.to_str())
        .context("Usage: screenshots clipboard inspect|text|files|image-info | image NEW.png")?;
    ensure!(
        args.len() == if command == "image" { 2 } else { 1 },
        "Usage: screenshots clipboard inspect|text|files|image-info | image NEW.png"
    );
    let cancel = app_core::NativeCaptureCancellation::default();
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let check = || -> std::result::Result<(), app_core::CaptureError> {
        #[cfg(unix)]
        if cancellation.check(deadline).is_err() {
            cancel.cancel();
        }
        if Instant::now() >= deadline {
            cancel.cancel();
        }
        cancel.check()
    };
    let backend = app_core::NativeClipboardBackend::Automatic;
    let value = match command {
        "inspect" | "text" | "files" => {
            let kind = match command {
                "inspect" => app_core::ClipboardInspectKind::Formats,
                "text" => app_core::ClipboardInspectKind::Text,
                _ => app_core::ClipboardInspectKind::Files,
            };
            serde_json::to_value(app_core::inspect_clipboard_with_cancel(
                kind, backend, &check,
            )?)?
        }
        "image-info" | "image" => {
            if command == "image" {
                let path = Path::new(&args[1]);
                ensure!(
                    app_core::ImageExportFormat::from_path(path)?
                        == app_core::ImageExportFormat::Png,
                    "Clipboard image output must be a new PNG"
                );
                ensure!(
                    path.symlink_metadata()
                        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                    "Clipboard output already exists or is unavailable"
                );
            }
            let frame = app_core::capture_clipboard_image_with_cancel(backend, &check)?;
            check()?;
            if command == "image" {
                frame.save_image_as_with_cancel(
                    Path::new(&args[1]),
                    app_core::DEFAULT_JPEG_QUALITY,
                    app_core::ExportSize::Original,
                    || check().is_err(),
                )?;
            }
            serde_json::json!({"kind":"image","width":frame.width,"height":frame.height,"rgba_bytes":frame.pixels.as_raw().len(),"saved":command=="image"})
        }
        _ => anyhow::bail!("Choose clipboard inspect, text, files, image-info or image NEW.png"),
    };
    #[cfg(unix)]
    cancellation.output_phase(true);
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}")?;
    output.flush()?;
    Ok(())
}
