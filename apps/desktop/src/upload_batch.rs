//! Explicit sequential uploads of finalized image pixels; no background batch service.

use anyhow::{Context, Result, bail, ensure};
use std::{ffi::OsString, io::Write, path::PathBuf};

const MAX_IMAGES: usize = 20;
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        (2..=MAX_IMAGES + 1).contains(&args.len()),
        "Usage: screenshots upload-images PROFILE IMAGE... (1–20 images)"
    );
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    #[cfg(unix)]
    let cancel = app_core::UploadCancellation::with_atomic_flag(cancellation.flag());
    #[cfg(not(unix))]
    let cancel = app_core::UploadCancellation::default();
    let destination = crate::sharex::prepare(
        std::path::Path::new(&args[0]),
        app_core::ShareXInputKind::Image,
    )?;
    let paths = args[1..]
        .iter()
        .map(|path| {
            let path = std::path::absolute(path)?;
            let metadata = path.metadata().context("An upload input is unavailable")?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_INPUT_BYTES,
                "Upload inputs must be regular files of at most 32 MiB"
            );
            Ok(path)
        })
        .collect::<Result<Vec<PathBuf>>>()?;
    let mut output = std::io::stdout().lock();
    for (offset, path) in paths.iter().enumerate() {
        let mut stage = "Image decoding failed";
        let result = (|| {
            let pixels = app_core::load_editor_image(path)?;
            stage = "PNG preparation failed";
            let png = crate::cli::frame(pixels).png_bytes_with_cancel(|| cancel.is_canceled())?;
            stage = "Upload failed; check the destination and credentials";
            crate::sharex::upload_png(&destination, png, cancel.clone())
        })();
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => {
                // Codec/server details can contain input data; report only the failed stage.
                bail!(
                    "Image {} of {} failed after {} confirmed uploads: {}. Prior NDJSON links remain valid; the failing upload may already exist remotely. No retry was attempted.",
                    offset + 1,
                    paths.len(),
                    offset,
                    stage
                );
            }
        };
        #[cfg(unix)]
        cancellation.output_phase(true);
        let mut outcome = outcome;
        let confirmed = crate::sharex::confirmed(&outcome).is_ok();
        let object = outcome
            .as_object_mut()
            .context("Upload result must be an object")?;
        object.insert(
            "type".into(),
            if confirmed { "uploaded" } else { "failed" }.into(),
        );
        object.insert("index".into(), (offset + 1).into());
        object.insert("total".into(), paths.len().into());
        let written = (|| -> Result<()> {
            serde_json::to_writer(&mut output, &outcome)?;
            output.write_all(b"\n")?;
            output.flush()?;
            Ok(())
        })();
        ensure!(
            written.is_ok(),
            "Image {} was uploaded but its result could not be written; the remote file exists and no later image was attempted",
            offset + 1
        );
        ensure!(
            confirmed,
            "Image {} failed after {} confirmed uploads; the remote object may exist and no later image was attempted",
            offset + 1,
            offset
        );
        #[cfg(unix)]
        cancellation.output_phase(false);
    }
    #[cfg(unix)]
    cancellation.output_phase(true);
    let summary = (|| -> Result<()> {
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({"type": "complete", "uploaded": paths.len()}),
        )?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    })();
    ensure!(
        summary.is_ok(),
        "All {} images were uploaded but the final summary could not be written; confirmed remote files remain",
        paths.len()
    );
    Ok(())
}
