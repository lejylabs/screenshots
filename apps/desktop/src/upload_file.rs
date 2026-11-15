//! Explicit byte-preserving file uploads through the existing transport.

use anyhow::{Context, Result, ensure};
use std::{ffi::OsString, io::Write, path::Path};

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2,
        "Usage: screenshots upload-file PROFILE FILE"
    );
    let destination = crate::cli::load_upload_profile(Path::new(&args[0]))?;
    let source = std::path::absolute(&args[1])?;
    let outcome = app_core::upload_file(
        &destination,
        &source,
        app_core::UploadCancellation::default(),
    )?;
    let mut stdout = std::io::stdout().lock();
    let result = (|| -> Result<()> {
        serde_json::to_writer(
            &mut stdout,
            &serde_json::json!({
                "url": outcome.url,
                "thumbnail": outcome.thumbnail,
                "deletion": outcome.deletion,
            }),
        )?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        Ok(())
    })();
    result.context("File was uploaded, but its result could not be written; the remote file remains and no retry was attempted")
}
