//! Explicit, read-only inspection of one page of capture file references.

use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{ErrorKind, Write as _},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 1 || args.len() == 3,
        "Usage: screenshots history-audit DATABASE [--cursor ID]"
    );
    ensure!(
        !args[0].is_empty() && args[0].as_encoded_bytes().len() <= 4096,
        "Database path must contain 1–4096 bytes"
    );
    let cursor = if args.len() == 3 {
        ensure!(args[1] == "--cursor", "Expected --cursor ID");
        let value = args[2].to_str().context("History cursor must be UTF-8")?;
        ensure!(
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
            "History cursor must be a positive whole number"
        );
        let id = value
            .parse::<i64>()
            .context("History cursor is too large")?;
        ensure!(id > 0, "History cursor must be positive");
        Some(id)
    } else {
        None
    };
    let page = app_core::query_capture_history(
        Path::new(&args[0]),
        &app_core::HistoryQuery {
            cursor,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
    )?;
    let entries: Vec<_> = page
        .entries
        .iter()
        .map(|entry| {
            // Follow links like an explicit file open, without opening or decoding the image.
            let status = match std::fs::metadata(&entry.path) {
                Ok(metadata) if metadata.is_file() => "present",
                Ok(_) => "not_file",
                Err(error) if error.kind() == ErrorKind::NotFound => "missing",
                Err(_) => "unreadable",
            };
            json!({
                "id": entry.id,
                "path": entry.path.to_string_lossy(),
                "status": status,
            })
        })
        .collect();
    let result = json!({
        "entries": entries,
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
        "path_encoding": "utf8-lossy",
    });
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}").context("Could not write history audit JSON")?;
    output.flush().context("Could not flush history audit JSON")
}
