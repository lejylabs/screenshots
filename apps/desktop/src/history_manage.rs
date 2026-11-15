//! Explicit capture metadata mutations; referenced images are never modified.

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

pub fn run(args: &[OsString]) -> Result<()> {
    let command = args
        .first()
        .and_then(|value| value.to_str())
        .context("Use history-manage import, remove or relink")?;
    match command {
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
        "remove" => {
            ensure!(
                (3..=22).contains(&args.len()),
                "Usage: screenshots history-manage remove DATABASE ID [ID...] (1–20 IDs)"
            );
            let ids: Vec<_> = args[2..].iter().map(positive_id).collect::<Result<_>>()?;
            let database = bounded_path(&args[1])?;
            existing_database(database)?;
            let removed = app_core::remove_history_entries(database, &ids)?;
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
