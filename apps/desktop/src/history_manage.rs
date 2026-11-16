//! Explicit metadata and journaled physical-file operations.

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{
    ffi::OsString,
    io::Write as _,
    path::{Path, PathBuf},
};

fn positive_id(value: &OsString) -> Result<i64> {
    let value = value.to_str().context("History ID must be UTF-8")?;
    ensure!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "History ID must be a positive whole number"
    );
    let id = value.parse().context("History ID is too large")?;
    ensure!(id > 0, "History ID must be positive");
    Ok(id)
}

fn bounded_path(value: &OsString) -> Result<&Path> {
    ensure!(
        value.as_encoded_bytes().len() <= 4096,
        "Path exceeds the 4096-byte limit"
    );
    Ok(Path::new(value))
}

fn absolute_path(value: &OsString) -> Result<PathBuf> {
    let path = std::path::absolute(bounded_path(value)?)?;
    ensure!(
        path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Absolute path exceeds the 4096-byte limit"
    );
    Ok(path)
}

fn existing_database(database: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(database).context("History database is unavailable")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Choose an existing regular history database, not a symlink"
    );
    Ok(())
}

fn bounded_error(mut error: String) -> String {
    const LIMIT: usize = 2048;
    if error.len() > LIMIT {
        let mut boundary = LIMIT - 3;
        while !error.is_char_boundary(boundary) {
            boundary -= 1;
        }
        error.truncate(boundary);
        error.push_str("...");
    }
    error
}

fn print_result(value: serde_json::Value, completed: &str) -> Result<()> {
    let mut output = std::io::stdout().lock();
    (|| {
        writeln!(output, "{value}")?;
        output.flush()
    })()
    .with_context(|| format!("{completed}; could not write JSON result"))
}

fn trash_id(value: &OsString) -> Result<&str> {
    let value = value.to_str().context("Trash ID must be UTF-8")?;
    ensure!(
        value.len() == 36
            && uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value),
        "Trash ID must be a canonical UUID"
    );
    Ok(value)
}

fn policy(path: &Path) -> Result<app_core::RetentionPolicy> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let named = std::fs::symlink_metadata(path)?;
    ensure!(
        named.is_file() && !named.file_type().is_symlink() && named.len() <= 4096,
        "Retention policy must be a regular JSON file of at most 4096 bytes"
    );
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        ensure!(
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "Retention policy opened a reparse point; no file operation started"
        );
    }
    ensure!(
        metadata.is_file() && metadata.len() <= 4096,
        "Retention policy changed before opening"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(metadata.len()).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == metadata.len(),
        "Retention policy was truncated"
    );
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid retention policy JSON"))
}

pub fn run(args: &[OsString]) -> Result<()> {
    let command = args
        .first()
        .and_then(|value| value.to_str())
        .context("Use a history-manage command; see --help")?;
    match command {
        "index-identities" => {
            ensure!(
                (2..=3).contains(&args.len()),
                "Usage: screenshots history-manage index-identities DATABASE [CURSOR]"
            );
            let cursor = args.get(2).map(positive_id).transpose()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            #[cfg(unix)]
            let cancellation = crate::record::unix::Cancellation::new()?;
            #[cfg(unix)]
            let flag = cancellation.flag();
            let report = app_core::prime_history_file_identities(database, cursor, || {
                #[cfg(unix)]
                {
                    flag.load(std::sync::atomic::Ordering::Relaxed)
                }
                #[cfg(not(unix))]
                {
                    false
                }
            })?;
            #[cfg(unix)]
            cancellation.output_phase(true);
            let complete = report.complete;
            print_result(
                serde_json::to_value(report)?,
                "Identity metadata preparation finished; image files are unchanged",
            )?;
            ensure!(
                complete,
                "Identity refresh is incomplete; see remaining, protected, unavailable_ids and next_cursor in the JSON result; image files are unchanged"
            );
            Ok(())
        }
        "trash" => {
            ensure!(
                (4..=103).contains(&args.len()),
                "Usage: screenshots history-manage trash DATABASE DIRECTORY ID... (1–100 IDs)"
            );
            let ids = args[3..]
                .iter()
                .map(positive_id)
                .collect::<Result<Vec<_>>>()?;
            ensure!(
                ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len(),
                "Duplicate trash capture ID"
            );
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let directory = absolute_path(&args[2])?;
            for id in &ids {
                ensure!(
                    app_core::lookup_history_entry(database, *id)?.is_some(),
                    "A selected history ID was not found"
                );
            }
            let mut completed = 0;
            for id in ids {
                let outcome=app_core::trash_history_file(database,id,&directory)
                    .with_context(||format!("{completed} prior trash operations committed; inspect pending-move after this failure"))?;
                completed += 1;
                print_result(
                    serde_json::to_value(outcome)?,
                    "Trash operation committed; files remain in trash",
                )?;
            }
            print_result(
                json!({"status":"complete","trashed":completed}),
                "Trash batch completed",
            )
        }
        "trash-list" => {
            ensure!(
                (2..=3).contains(&args.len()),
                "Usage: screenshots history-manage trash-list DATABASE [CURSOR]"
            );
            let cursor = args.get(2).map(positive_id).transpose()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            print_result(
                serde_json::to_value(app_core::trash_history_page(database, cursor)?)?,
                "Trash page read",
            )
        }
        "restore" => {
            ensure!(
                (3..=4).contains(&args.len()),
                "Usage: screenshots history-manage restore DATABASE TRASH_UUID [NEW.png]"
            );
            let id = trash_id(&args[2])?;
            let destination = args.get(3).map(absolute_path).transpose()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let outcome = app_core::restore_history_trash(database, id, destination.as_deref())?;
            print_result(
                serde_json::to_value(outcome)?,
                "Trash restoration committed",
            )
        }
        "purge" => {
            ensure!(
                (3..=102).contains(&args.len()),
                "Usage: screenshots history-manage purge DATABASE TRASH_UUID... (1–100 IDs)"
            );
            let ids = args[2..].iter().map(trash_id).collect::<Result<Vec<_>>>()?;
            ensure!(
                ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len(),
                "Duplicate purge trash ID"
            );
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let mut completed = 0;
            for id in ids {
                let outcome=app_core::purge_history_trash(database,id)
                    .with_context(||format!("{completed} prior purges committed; inspect pending-move after this failure"))?;
                completed += 1;
                print_result(
                    serde_json::to_value(outcome)?,
                    "Purge committed; the selected trash file was permanently removed",
                )?;
            }
            print_result(
                json!({"status":"complete","purged":completed}),
                "Purge batch completed",
            )
        }
        "retention" => {
            ensure!(
                args.len() == 4,
                "Usage: screenshots history-manage retention DATABASE POLICY.json preview|apply"
            );
            ensure!(
                matches!(args[3].to_str(), Some("preview" | "apply")),
                "Choose preview or apply for retention"
            );
            let policy = policy(bounded_path(&args[2])?)?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            if args[3] == "preview" {
                print_result(
                    serde_json::to_value(app_core::plan_retention(database, &policy)?)?,
                    "Retention preview read",
                )
            } else {
                #[cfg(unix)]
                let cancellation = crate::record::unix::Cancellation::new()?;
                #[cfg(unix)]
                let flag = cancellation.flag();
                let report = app_core::apply_retention(database, &policy, || {
                    #[cfg(unix)]
                    {
                        flag.load(std::sync::atomic::Ordering::Relaxed)
                    }
                    #[cfg(not(unix))]
                    {
                        false
                    }
                })?;
                #[cfg(unix)]
                cancellation.output_phase(true);
                let failed = report.failure.is_some();
                print_result(
                    serde_json::to_value(report)?,
                    "Retention finished; confirmed earlier file operations remain committed",
                )?;
                ensure!(
                    !failed,
                    "Retention stopped; confirmed outcomes are in the JSON result; inspect pending-move before retrying"
                );
                Ok(())
            }
        }
        "move" | "transfer" => {
            ensure!(
                args.len() == 4,
                "Usage: screenshots history-manage move DATABASE ID NEW.png"
            );
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let id = positive_id(&args[2])?;
            let destination = absolute_path(&args[3])?;
            let outcome = if command == "transfer" {
                app_core::transfer_history_file(database, id, &destination)?
            } else {
                app_core::move_history_file(database, id, &destination)?
            };
            print_result(
                serde_json::to_value(outcome)?,
                "History file move committed",
            )
        }
        "pending-move" => {
            ensure!(
                args.len() == 2,
                "Usage: screenshots history-manage pending-move DATABASE"
            );
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            print_result(
                serde_json::json!({"pending": app_core::pending_history_move(database)?}),
                "Could not report pending history move",
            )
        }
        "recover-move" | "recover-file" => {
            ensure!(
                args.len() == 4,
                "Usage: screenshots history-manage recover-move DATABASE OPERATION finish|cancel"
            );
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let operation = args[2]
                .to_str()
                .context("Move operation ID must be UTF-8")?;
            let action = match args[3].to_str() {
                Some("finish") => app_core::MoveRecovery::Finish,
                Some("cancel") => app_core::MoveRecovery::Cancel,
                _ => bail!("Choose finish or cancel for move recovery"),
            };
            let outcome = if command == "recover-file" {
                app_core::recover_history_file_operation(database, operation, action)?
            } else {
                app_core::recover_history_move(database, operation, action)?
            };
            print_result(
                serde_json::to_value(outcome)?,
                "History move recovery committed",
            )
        }
        "export" => {
            ensure!(
                (5..=104).contains(&args.len()),
                "Usage: screenshots history-manage export DATABASE DIRECTORY original|strip-metadata ID... (1–100 IDs)"
            );
            let strip = match args[3].to_str() {
                Some("original") => false,
                Some("strip-metadata") => true,
                _ => bail!("Export policy must be original or strip-metadata"),
            };
            let ids = args[4..]
                .iter()
                .map(positive_id)
                .collect::<Result<Vec<_>>>()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let directory = absolute_path(&args[2])?;
            let sources = ids
                .into_iter()
                .map(|id| {
                    app_core::lookup_history_entry(database, id)?
                        .map(|entry| entry.path)
                        .context("History export ID was not found")
                })
                .collect::<Result<Vec<_>>>()?;
            let mut exported = 0;
            let mut not_exported = 0;
            let mut failures = Vec::new();
            for chunk in sources.chunks(20) {
                let report =
                    app_core::export_saved_pngs_with_metadata(chunk, &directory, strip, || false)
                        .with_context(|| {
                        format!("{exported} files already exported; published files remain")
                    })?;
                exported += report.exported;
                not_exported += report.not_exported;
                failures.extend(report.failures.into_iter().map(|(path,error)|json!({"path":path.to_string_lossy(),"error":bounded_error(error)})));
            }
            print_result(
                json!({"exported":exported,"failed":failures.len(),"not_exported":not_exported,"failures":failures,"metadata_policy":if strip {"strip-metadata"}else{"original"},"path_encoding":"lossy-native"}),
                "History export completed; published files remain",
            )
        }
        "import" => {
            ensure!(
                (3..=102).contains(&args.len()),
                "Usage: screenshots history-manage import DATABASE PNG [PNG...] (1–100 inputs)"
            );
            let database = bounded_path(&args[1])?;
            let paths: Vec<_> = args[2..].iter().map(absolute_path).collect::<Result<_>>()?;
            let report = app_core::import_capture_files(database, &paths)?;
            let failed = report.failures.len();
            let failures: Vec<_> = report.failures.into_iter().map(|(path, error)| json!({"path":path.to_string_lossy(),"error":bounded_error(error)})).collect();
            print_result(
                json!({"added":report.added,"existing":report.existing,"failed":failed,"failures":failures,"path_encoding":"lossy-native"}),
                "History import transaction committed",
            )
        }
        "remove" | "remove-missing" => {
            ensure!(
                (3..=102).contains(&args.len()),
                "Usage: screenshots history-manage remove|remove-missing DATABASE ID [ID...] (1–100 IDs)"
            );
            let ids: Vec<_> = args[2..].iter().map(positive_id).collect::<Result<_>>()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let removed = if command == "remove-missing" {
                app_core::remove_missing_history_entries(database, &ids)?
            } else {
                app_core::remove_history_entries(database, &ids)?
            };
            print_result(
                json!({"removed":removed}),
                "History removal transaction committed",
            )
        }
        "relink" => {
            ensure!(
                args.len() == 4,
                "Usage: screenshots history-manage relink DATABASE ID PNG"
            );
            let id = positive_id(&args[2])?;
            let path = absolute_path(&args[3])?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let original = app_core::lookup_history_entry(database, id)?
                .context("History entry was not found")?;
            app_core::relink_history_entry(database, &original, &path)?;
            let entry = app_core::lookup_history_entry(database, id)
                .context("History relink committed, but the updated entry could not be read")?
                .context("History relink committed, but the entry was removed before rereading")?;
            print_result(
                json!({"id":entry.id,"path":entry.path.to_string_lossy(),"width":entry.width,"height":entry.height,"path_encoding":"lossy-native"}),
                "History relink transaction committed",
            )
        }
        _ => bail!("Unknown history-manage command; use --help"),
    }
}
