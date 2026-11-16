//! Explicit portal capture; all output and recipe admission precedes permission selection.
use anyhow::{Context, Result, bail, ensure};
use app_core::{ExportSize, ImageExportFormat, PortalCaptureCursor, PortalCaptureSource};
use std::{ffi::OsString, io::Write, path::Path, sync::Arc, time::Duration};
fn new_output(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Output path must contain 1–4096 native bytes"
    );
    match path.symlink_metadata() {
        Ok(_) => bail!("Output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    const USAGE: &str = "Usage: screenshots capture-portal display|window OUTPUT [--cursor exclude|include] [--delay-ms 0..3600000] [--recipe FILE]";
    ensure!(args.len() >= 2 && (args.len() - 2).is_multiple_of(2), USAGE);
    let source = match args[0].to_str() {
        Some("display") => PortalCaptureSource::Display,
        Some("window") => PortalCaptureSource::Window,
        _ => bail!(USAGE),
    };
    let output = Path::new(&args[1]);
    new_output(output)?;
    let format = ImageExportFormat::from_path(output)?;
    let mut cursor = None;
    let mut delay = None;
    let mut recipe = None;
    for pair in args[2..].chunks_exact(2) {
        match pair[0].to_str() {
            Some("--cursor") => {
                ensure!(cursor.is_none(), "Duplicate cursor option");
                cursor = Some(match pair[1].to_str() {
                    Some("exclude") => PortalCaptureCursor::Exclude,
                    Some("include") => PortalCaptureCursor::Include,
                    _ => bail!("Cursor must be exclude or include"),
                });
            }
            Some("--delay-ms") => {
                ensure!(delay.is_none(), "Duplicate delay option");
                let milliseconds: u64 = pair[1]
                    .to_str()
                    .context("Delay must be UTF-8")?
                    .parse()
                    .context("Delay must be a whole number")?;
                ensure!(
                    milliseconds <= 3_600_000,
                    "Delay must be 0–3600000 milliseconds"
                );
                delay = Some(Duration::from_millis(milliseconds));
            }
            Some("--recipe") => {
                ensure!(recipe.is_none(), "Duplicate recipe option");
                recipe = Some(crate::workflow::Recipe::read(Path::new(&pair[1]))?);
            }
            _ => bail!(USAGE),
        }
    }
    #[cfg(not(target_os = "linux"))]
    bail!("Portal capture requires Linux");
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
    new_output(output)?;
    let mut frame =
        app_core::capture_portal_with_cancel(source, cursor.unwrap_or_default(), &|| Ok(()))?;
    if let Some(recipe) = recipe {
        let pixels = Arc::try_unwrap(frame.pixels).unwrap_or_else(|pixels| (*pixels).clone());
        frame.pixels = Arc::new(recipe.apply(pixels)?);
        frame.width = frame.pixels.width();
        frame.height = frame.pixels.height();
    }
    frame.save_image_as(output, app_core::DEFAULT_JPEG_QUALITY, ExportSize::Original)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout,"{}",serde_json::json!({"published":true,"width":frame.width,"height":frame.height,"format":format.extension(),"source":source,"cursor":cursor.unwrap_or_default(),"capture_elapsed_ms":frame.elapsed_ms}))
        .and_then(|_|stdout.flush()).context("Portal capture output was saved, but its result could not be written")?;
    Ok(())
}
