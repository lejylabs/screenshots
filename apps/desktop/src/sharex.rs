//! Explicit full SXCU execution; profile interaction stays in bounded result events.
use anyhow::{Context, Result, ensure};
use app_core::{ClassifiedUploadProfile, ShareXInputKind, ShareXPrepared, UploadCancellation};
use std::{ffi::OsString, io::Write, path::Path};

pub(crate) fn prepare(path: &Path, kind: ShareXInputKind) -> Result<ClassifiedUploadProfile> {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sxcu"))
    {
        app_core::classify_upload_profile(path, kind, None)
    } else {
        Ok(ClassifiedUploadProfile::Native(Box::new(
            crate::cli::load_upload_profile(path)?,
        )))
    }
}

pub(crate) fn upload_file(
    prepared: &ClassifiedUploadProfile,
    path: &Path,
    cancel: UploadCancellation,
) -> Result<serde_json::Value> {
    match prepared {
        ClassifiedUploadProfile::Native(destination) => {
            let result = app_core::upload_file(destination, path, cancel)?;
            Ok(
                serde_json::json!({"url":result.url,"thumbnail":result.thumbnail,"deletion":result.deletion}),
            )
        }
        ClassifiedUploadProfile::ShareX(prepared) => {
            Ok(serde_json::to_value(prepared.upload_file(path, cancel)?)?)
        }
    }
}

pub(crate) fn upload_png(
    prepared: &ClassifiedUploadProfile,
    png: Vec<u8>,
    cancel: UploadCancellation,
) -> Result<serde_json::Value> {
    match prepared {
        ClassifiedUploadProfile::Native(destination) => {
            let result = app_core::upload_png_bytes_with_progress(destination, png, cancel, None)?;
            Ok(
                serde_json::json!({"url":result.url,"thumbnail":result.thumbnail,"deletion":result.deletion}),
            )
        }
        ClassifiedUploadProfile::ShareX(prepared) => Ok(serde_json::to_value(
            prepared.upload_png_bytes(png, cancel)?,
        )?),
    }
}

pub(crate) fn confirmed(value: &serde_json::Value) -> Result<()> {
    ensure!(
        value.get("success").and_then(serde_json::Value::as_bool) != Some(false),
        "Custom uploader did not return a confirmed success; the remote object may exist and no retry was attempted"
    );
    Ok(())
}

pub(crate) fn single(args: &[OsString], image: bool) -> Result<()> {
    ensure!(
        args.len() == 2,
        "Usage: screenshots upload|upload-file PROFILE INPUT"
    );
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    #[cfg(unix)]
    let cancel = UploadCancellation::with_atomic_flag(cancellation.flag());
    #[cfg(not(unix))]
    let cancel = UploadCancellation::default();
    let kind = if image {
        ShareXInputKind::Image
    } else {
        ShareXInputKind::File
    };
    let prepared = prepare(Path::new(&args[0]), kind)?;
    let path = std::path::absolute(&args[1])?;
    let result = if image {
        match &prepared {
            ClassifiedUploadProfile::Native(destination) => {
                serde_json::json!({"url":app_core::upload_png(destination,&path,cancel)?})
            }
            ClassifiedUploadProfile::ShareX(prepared) => {
                serde_json::to_value(prepared.upload_file(&path, cancel)?)?
            }
        }
    } else {
        upload_file(&prepared, &path, cancel)?
    };
    #[cfg(unix)]
    cancellation.output_phase(true);
    let mut output = std::io::stdout().lock();
    writeln!(output,"{result}").and_then(|_|output.flush()).context("Upload finished but its result could not be written; the remote object may exist and no retry was attempted")?;
    confirmed(&result)
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let operation = args.first().and_then(|value| value.to_str()).context("Usage: screenshots sharex inspect PROFILE [--options FILE]; sharex image|file|text|url|url-sharing PROFILE INPUT_FILE [--options FILE]")?;
    let inspect = operation == "inspect";
    let required = if inspect { 2 } else { 3 };
    ensure!(
        args.len() == required || args.len() == required + 2,
        "Invalid sharex arguments; use --help"
    );
    let options = if args.len() == required + 2 {
        ensure!(args[required] == "--options", "Expected --options FILE");
        Some(Path::new(&args[required + 1]))
    } else {
        None
    };
    for path in std::iter::once(Path::new(&args[1]))
        .chain(options)
        .chain((!inspect).then(|| Path::new(&args[2])))
    {
        ensure!(
            !path.as_os_str().is_empty() && path.as_os_str().as_encoded_bytes().len() <= 4096,
            "SXCU paths must contain 1–4096 bytes"
        );
    }
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    #[cfg(unix)]
    let cancel = UploadCancellation::with_atomic_flag(cancellation.flag());
    #[cfg(not(unix))]
    let cancel = UploadCancellation::default();
    let result = if inspect {
        ShareXPrepared::inspect_with_cancel(Path::new(&args[1]), options, cancel)?
    } else {
        let kind = match operation {
            "image" => ShareXInputKind::Image,
            "file" => ShareXInputKind::File,
            "text" => ShareXInputKind::Text,
            "url" => ShareXInputKind::Url,
            "url-sharing" => ShareXInputKind::UrlSharing,
            _ => anyhow::bail!("Unknown sharex input kind"),
        };
        serde_json::to_value(
            ShareXPrepared::read(Path::new(&args[1]), kind, options)?
                .upload_file(Path::new(&args[2]), cancel)?,
        )?
    };
    #[cfg(unix)]
    cancellation.output_phase(true);
    let mut output = std::io::stdout().lock();
    writeln!(output,"{result}").and_then(|_|output.flush()).context("SXCU operation completed but its result could not be written; outputs or a remote object may remain")?;
    confirmed(&result)
}
