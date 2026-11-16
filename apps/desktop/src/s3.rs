//! Explicit S3 upload and native credential commands; no environment discovery.

use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::{IsTerminal, Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

const PREFIX: &[u8] = b"screenshots-s3-v1\0";

fn profile(value: &OsString) -> Result<app_core::S3Profile> {
    ensure!(
        value.as_encoded_bytes().len() <= 4096,
        "S3 profile path exceeds 4096 bytes"
    );
    app_core::S3Profile::load(Path::new(value))
}

fn print(value: &serde_json::Value, completed: &str) -> Result<()> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{value}")
        .and_then(|_| output.flush())
        .with_context(|| completed.to_owned())
}

pub(crate) fn upload(args: &[OsString]) -> Result<()> {
    ensure!(args.len() == 2, "Usage: screenshots s3-upload PROFILE FILE");
    let profile = profile(&args[0])?;
    ensure!(
        args[1].as_encoded_bytes().len() <= 4096,
        "S3 input path exceeds 4096 bytes"
    );
    let path = std::path::absolute(&args[1])?;
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "S3 input path exceeds 4096 bytes"
    );
    let outcome = upload_prepared(&profile, &path)?;
    print(
        &outcome,
        "S3 object was uploaded but its result could not be written; no retry was attempted",
    )
}

pub(crate) fn upload_prepared(
    profile: &app_core::S3Profile,
    path: &Path,
) -> Result<serde_json::Value> {
    upload_prepared_with_cancellation(profile, path, app_core::UploadCancellation::default())
}

pub(crate) fn upload_prepared_with_cancellation(
    profile: &app_core::S3Profile,
    path: &Path,
    cancel: app_core::UploadCancellation,
) -> Result<serde_json::Value> {
    ensure!(
        !cancel.is_canceled(),
        "S3 upload canceled before credential lookup"
    );
    let payload = app_core::read_s3_credentials(&profile.credential_target()?)?;
    let json = payload
        .strip_prefix(PREFIX)
        .context("Stored S3 credentials have an unsupported format")?;
    let credentials = app_core::S3Credentials::from_json(json)?;
    drop(payload);
    let outcome = app_core::upload_s3_file_with_cancellation(profile, path, credentials, cancel)?;
    let public = profile.public_url(&outcome.key)?;
    let mut result =
        serde_json::json!({"bucket":outcome.bucket,"key":outcome.key,"etag":outcome.etag});
    if let Some(url) = public {
        result["url"] = url.into();
    }
    Ok(result)
}

fn credential_payload(bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let credentials = app_core::S3Credentials::from_json(bytes)?;
    let mut payload = Zeroizing::new(Vec::with_capacity(4096));
    payload.extend_from_slice(PREFIX);
    #[derive(serde::Serialize)]
    struct Fields<'a> {
        access_key_id: &'a str,
        secret_access_key: &'a str,
    }
    serde_json::to_writer(
        &mut *payload,
        &Fields {
            access_key_id: credentials.access_key_id.as_str(),
            secret_access_key: credentials.secret_access_key.as_str(),
        },
    )?;
    Ok(payload)
}

pub(crate) fn credentials(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 2,
        "Usage: screenshots s3-credentials target|store|remove PROFILE"
    );
    let action = args[0]
        .to_str()
        .context("S3 credential action must be UTF-8")?;
    ensure!(
        matches!(action, "target" | "store" | "remove"),
        "Unknown S3 credential action"
    );
    let profile = profile(&args[1])?;
    let target = profile.credential_target()?;
    match action {
        "target" => print(
            &serde_json::json!({"target":target}),
            "Could not write credential target",
        ),
        "store" => {
            ensure!(
                !std::io::stdin().is_terminal(),
                "Pipe S3 credential JSON through stdin; interactive secret echo is disabled"
            );
            let mut bytes = Zeroizing::new(Vec::with_capacity(4097));
            std::io::stdin().lock().take(4097).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 4096,
                "S3 credential input exceeds 4096 bytes"
            );
            let payload = credential_payload(&bytes)?;
            app_core::save_s3_credentials(&target, &payload)?;
            print(
                &serde_json::json!({"stored":true,"profile_file_changed":false}),
                "S3 credentials were stored but their result could not be written",
            )
        }
        "remove" => {
            app_core::remove_s3_credentials(&target)?;
            print(
                &serde_json::json!({"removed":true,"profile_file_changed":false}),
                "S3 credentials were removed but their result could not be written",
            )
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_versioned_and_compacted_without_unknown_fields() {
        let bytes =
            br#"{ "access_key_id": "SYNTHETICID", "secret_access_key": "synthetic/key+value" }"#;
        let payload = credential_payload(bytes).unwrap();
        assert!(payload.starts_with(PREFIX));
        let credentials = app_core::S3Credentials::from_json(&payload[PREFIX.len()..]).unwrap();
        assert_eq!(credentials.access_key_id.as_str(), "SYNTHETICID");
        assert_eq!(
            credentials.secret_access_key.as_str(),
            "synthetic/key+value"
        );
        assert!(payload.len() < 2560);
        assert!(
            credential_payload(br#"{"access_key_id":"A","secret_access_key":"b","token":"c"}"#)
                .is_err()
        );
    }
}
