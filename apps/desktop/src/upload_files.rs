//! Explicit sequential file uploads; no decode, queue or resident work.

use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_FILES: usize = 20;
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

fn paths(args: &[OsString]) -> Result<Vec<PathBuf>> {
    ensure!((1..=MAX_FILES).contains(&args.len()), "Select 1–20 files");
    args.iter()
        .map(|value| {
            ensure!(
                value.as_encoded_bytes().len() <= 4096,
                "Upload input path exceeds 4096 bytes"
            );
            let path = std::path::absolute(value)?;
            ensure!(
                path.as_os_str().as_encoded_bytes().len() <= 4096,
                "Absolute upload input path exceeds 4096 bytes"
            );
            let metadata = path.metadata().context("An upload input is unavailable")?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_INPUT_BYTES,
                "Upload inputs must be regular files of at most 64 MiB"
            );
            Ok(path)
        })
        .collect()
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        (2..=MAX_FILES + 1).contains(&args.len()),
        "Usage: screenshots upload-files PROFILE FILE... (1–20 files)"
    );
    ensure!(
        args[0].as_encoded_bytes().len() <= 4096,
        "Upload profile path exceeds 4096 bytes"
    );
    let destination = crate::upload_file::load_profile(Path::new(&args[0]))?;
    ensure!(
        destination.request_json().is_empty(),
        "Arbitrary file uploads support multipart POST or binary PUT, not image JSON bodies"
    );
    let paths = paths(&args[1..])?;
    let mut output = std::io::stdout().lock();
    for (index, path) in paths.iter().enumerate() {
        let outcome = match app_core::upload_file(
            &destination,
            path,
            app_core::UploadCancellation::default(),
        ) {
            Ok(outcome) => outcome,
            Err(_) => bail!(
                "File {} of {} failed after {} confirmed uploads: upload failed; check the destination and credentials. Prior NDJSON links remain valid; the failing upload may already exist remotely. No retry or later upload was attempted.",
                index + 1,
                paths.len(),
                index
            ),
        };
        let written = (|| -> Result<()> {
            serde_json::to_writer(
                &mut output,
                &serde_json::json!({
                    "type":"uploaded","index":index+1,"total":paths.len(),
                    "url":outcome.url,"thumbnail":outcome.thumbnail,"deletion":outcome.deletion,
                }),
            )?;
            output.write_all(b"\n")?;
            output.flush()?;
            Ok(())
        })();
        ensure!(
            written.is_ok(),
            "File {} was uploaded but its result could not be written; {} earlier complete NDJSON results are confirmed. The current line may be partial; the remote file exists and no later file was attempted",
            index + 1,
            index
        );
    }
    let written = (|| -> Result<()> {
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({"type":"complete","uploaded":paths.len()}),
        )?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    })();
    ensure!(
        written.is_ok(),
        "All {} files were uploaded but the final summary could not be written; confirmed remote files remain",
        paths.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_accepts_empty_duplicates_and_rejects_nonregular_or_oversized_inputs() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-files-preflight-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let empty = root.join("empty");
        std::fs::write(&empty, []).unwrap();
        let args = [
            empty.clone().into_os_string(),
            empty.clone().into_os_string(),
        ];
        assert_eq!(paths(&args).unwrap(), [empty.clone(), empty]);
        assert!(paths(&[]).is_err());
        assert!(paths(&vec![args[0].clone(); 21]).is_err());
        assert!(paths(&[root.clone().into_os_string()]).is_err());
        assert!(paths(&[root.join("missing").into_os_string()]).is_err());
        let huge = root.join("huge");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        assert!(paths(&[huge.into_os_string()]).is_err());
        assert!(paths(&[OsString::from("x".repeat(4097))]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
