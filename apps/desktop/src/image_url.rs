//! Explicit URL image input through the existing isolated job and editor decoder.
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
fn output_available(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Output path must contain1–4096 native bytes"
    );
    match path.symlink_metadata() {
        Ok(_) => bail!("Output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    const USAGE: &str = "Usage: screenshots image-url URL OUTPUT [--allow-http] [--recipe FILE]";
    ensure!(args.len() >= 2, USAGE);
    let url = args[0].to_str().context("Image URL must be UTF-8")?;
    let output = Path::new(&args[1]);
    output_available(output)?;
    app_core::ImageExportFormat::from_path(output)?;
    let mut allow_http = false;
    let mut recipe = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].to_str() {
            Some("--allow-http") => {
                ensure!(!allow_http, "Duplicate HTTP option");
                allow_http = true;
                index += 1;
            }
            Some("--recipe") => {
                ensure!(recipe.is_none() && index + 1 < args.len(), USAGE);
                recipe = Some(crate::workflow::Recipe::read(Path::new(&args[index + 1]))?);
                index += 2;
            }
            _ => bail!(USAGE),
        }
    }
    let source = app_core::ImageUrlSource::new(url.to_owned(), allow_http)?;
    #[cfg(unix)]
    let cancel = crate::record::unix::Cancellation::new()?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let check = || -> Result<()> {
        #[cfg(unix)]
        cancel.check(deadline)?;
        ensure!(
            Instant::now() < deadline,
            "Image URL operation exceeded its duration budget"
        );
        Ok(())
    };
    let mut frame = source
        .capture_with_cancel(&|| check().map_err(|_| app_core::CaptureError::NativeCanceled))?;
    if let Some(recipe) = recipe {
        frame = crate::cli::frame(
            recipe.apply_with_cancel(
                Arc::try_unwrap(frame.pixels)
                    .map_err(|_| anyhow::anyhow!("Imported image pixels remain shared"))?,
                &check,
            )?,
        );
    }
    check()?;
    output_available(output)?;
    let format = frame.save_image_as_with_cancel(
        output,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
        || check().is_err(),
    )?;
    #[cfg(unix)]
    cancel.output_phase(true);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout,"{}",serde_json::json!({"published":true,"width":frame.width,"height":frame.height,"format":format.extension()}))
        .and_then(|_|stdout.flush()).context("Image URL output was saved, but its result could not be written")?;
    Ok(())
}
