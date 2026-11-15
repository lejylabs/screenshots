//! Explicit, worker-side history inspection and online backup. No idle cleanup.

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;
const CHECK_DEADLINE: Duration = Duration::from_secs(5);
const BACKUP_DEADLINE: Duration = Duration::from_secs(30);
const MAX_INSPECTED_ENTRIES: usize = 10_000;
const MAX_PART_CANDIDATES: usize = 100;

#[derive(Debug)]
pub struct HistoryHealth {
    pub schema_version: u32,
    pub quick_check: String,
}

#[derive(Debug)]
pub struct PartInspection {
    pub candidates: Vec<PathBuf>,
    pub inspected_entries: usize,
    /// At least one limit was reached; absence from this partial result proves nothing.
    pub truncated: bool,
}

fn resolve_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().context("Choose a database file path")?;
    Ok(parent.canonicalize()?.join(name))
}

pub(crate) fn open_read_only(path: &Path) -> Result<Connection> {
    // SQLite NOFOLLOW rejects ancestor links too, including macOS /var -> /private/var.
    // Resolve existing ancestry only; the selected file itself must never be followed.
    let path = resolve_parent(path)?;
    let metadata = fs::symlink_metadata(&path).context("History database is unavailable")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Choose a regular history database, not a symlink"
    );
    ensure!(
        metadata.len() <= MAX_DATABASE_BYTES,
        "History database exceeds the 1 GiB maintenance limit"
    );
    let mut connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    let deadline = Instant::now() + CHECK_DEADLINE;
    connection.progress_handler(1_000, Some(move || Instant::now() >= deadline))?;
    Ok(connection)
}

fn check_connection(connection: &Connection) -> Result<HistoryHealth> {
    let schema_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=5).contains(&schema_version),
        "Unsupported history schema version"
    );
    let require_object = |name: &str, kind: &str| -> Result<()> {
        ensure!(
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 AND type=?2)",
                [name, kind],
                |row| row.get::<_, bool>(0),
            )?,
            "History database schema is incomplete"
        );
        Ok(())
    };
    require_object("captures", "table")?;
    if schema_version >= 5 {
        require_object("pending_file_move", "table")?;
        connection.prepare("SELECT singleton,operation_id,capture_id,source_path,destination_path,payload FROM pending_file_move LIMIT 0")?;
        for name in [
            "captures_move_insert",
            "captures_move_update",
            "captures_move_delete",
        ] {
            require_object(name, "trigger")?;
        }
    }
    // Preparing these queries checks columns without reading image paths or uploaded links.
    connection.prepare("SELECT id,file_path,thumbnail_path,mime_type,width,height,file_size,created_at,source FROM captures LIMIT 0")?;
    if schema_version >= 2 {
        require_object("captures_source_id", "index")?;
    }
    if schema_version >= 3 {
        require_object("captures_search", "table")?;
        for name in [
            "captures_search_insert",
            "captures_search_delete",
            "captures_search_update",
        ] {
            require_object(name, "trigger")?;
        }
        connection
            .prepare("SELECT rowid,file_path,created_at,source FROM captures_search LIMIT 0")?;
    }
    if schema_version >= 4 {
        require_object("uploads", "table")?;
        connection.prepare("SELECT id,filename,origin,url,created_at FROM uploads LIMIT 0")?;
    }
    let pages: u32 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let page_size: u32 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    ensure!(
        u64::from(pages)
            .checked_mul(u64::from(page_size))
            .is_some_and(|bytes| bytes <= MAX_DATABASE_BYTES),
        "History database exceeds the 1 GiB maintenance limit"
    );
    let result: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    // Do not echo untrusted database contents from a corrupt-page diagnostic.
    ensure!(result == "ok", "History database integrity check failed");
    Ok(HistoryHealth {
        schema_version,
        quick_check: result,
    })
}

/// Read-only, bounded SQLite quick_check; never creates or migrates a database.
pub fn check_history_database(path: &Path) -> Result<HistoryHealth> {
    check_connection(&open_read_only(path)?)
}

struct BackupPart(PathBuf);

impl Drop for BackupPart {
    fn drop(&mut self) {
        // Only the uniquely created staging file and its SQLite sidecars belong to this job.
        for path in [
            self.0.clone(),
            sidecar(&self.0, "-wal"),
            sidecar(&self.0, "-shm"),
            sidecar(&self.0, "-journal"),
        ] {
            let _ = fs::remove_file(path);
        }
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

/// Copy a consistent live WAL snapshot into a new file, without replacing any destination.
/// Cancellation and the 30-second deadline are checked between 64-page SQLite steps.
pub fn backup_history_database(
    source: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    let started = Instant::now();
    let check = || -> Result<()> {
        ensure!(!cancelled(), "History backup canceled");
        ensure!(
            started.elapsed() < BACKUP_DEADLINE,
            "History backup deadline exceeded"
        );
        Ok(())
    };
    check()?;
    let destination = resolve_parent(destination)?;
    ensure!(
        fs::symlink_metadata(&destination)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Backup destination already exists or is unavailable"
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(
            fs::symlink_metadata(sidecar(&destination, suffix))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "Backup destination has existing SQLite sidecars"
        );
    }
    let source_connection = open_read_only(source)?;
    source_connection.execute_batch("BEGIN DEFERRED")?;
    check_connection(&source_connection)?;
    check()?;
    let parent = destination.parent().context("Invalid backup directory")?;
    let temporary = parent.join(format!(".screenshots-{}.part", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let reservation = options.open(&temporary)?;
    let part = BackupPart(temporary);
    drop(reservation);
    let mut target = Connection::open_with_flags(
        &part.0,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    target.busy_timeout(Duration::from_millis(100))?;
    {
        let backup = Backup::new(&source_connection, &mut target)?;
        loop {
            check()?;
            match backup.step(64)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => {
                    bail!("History database is busy; retry the explicit backup later")
                }
                _ => bail!("History backup did not complete"),
            }
        }
    }
    // The published backup is a standalone database, not a main file needing a WAL sibling.
    target.pragma_update(None, "journal_mode", "DELETE")?;
    target.close().map_err(|(_, error)| error)?;
    check_history_database(&part.0)?;
    check()?;
    {
        // Windows FlushFileBuffers requires write access; never truncate the verified copy.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&part.0)?;
        file.sync_all()?;
    }
    check()?;
    crate::capture::publish_output(&part.0, &destination)?;
    Ok(())
}

fn part_candidate(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".part") else {
        return false;
    };
    let Some(id) = stem
        .strip_prefix(".screenshots-")
        .or_else(|| stem.strip_prefix("settings-"))
    else {
        return false;
    };
    id.len() == 36
        && uuid::Uuid::parse_str(id).is_ok_and(|value| value.hyphenated().to_string() == id)
}

/// Inspect only one directory. Names identify candidates, never proof of abandonment.
/// No file is opened, deleted, followed, decoded or considered safe to remove.
pub fn inspect_part_files(
    directory: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<PartInspection> {
    ensure!(!cancelled(), "Part inspection canceled");
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "Choose a directory, not a symlink"
    );
    let started = Instant::now();
    let mut result = PartInspection {
        candidates: Vec::new(),
        inspected_entries: 0,
        truncated: false,
    };
    for entry in fs::read_dir(directory)? {
        ensure!(!cancelled(), "Part inspection canceled");
        if result.inspected_entries == MAX_INSPECTED_ENTRIES
            || result.candidates.len() == MAX_PART_CANDIDATES
            || started.elapsed() >= CHECK_DEADLINE
        {
            result.truncated = true;
            break;
        }
        let entry = entry?;
        result.inspected_entries += 1;
        if part_candidate(&entry.file_name()) && entry.file_type()?.is_file() {
            result.candidates.push(entry.path());
        }
    }
    result.candidates.sort();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn maintenance_uses_sqlite_with_the_wal_page_size_disclosure_fix() {
        assert!(
            rusqlite::version_number() >= 3_054_000,
            "SQLite must include the RUSTSEC-2026-0459 fix"
        );
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("screenshots-storage-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn database(&self) -> Result<PathBuf> {
            let path = self.0.join("history.sqlite3");
            crate::record_uploaded_png(
                &path,
                Path::new("fixture.png"),
                "https://example.invalid",
                "https://example.invalid/fixture",
            )?;
            Ok(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn inspection_rejects_missing_foreign_and_future_databases_without_creating_or_migrating()
    -> Result<()> {
        let fixture = Fixture::new();
        let missing = fixture.0.join("missing/history.sqlite3");
        assert!(check_history_database(&missing).is_err());
        assert!(!fixture.0.join("missing").exists());
        let foreign = fixture.0.join("foreign.sqlite3");
        let connection = Connection::open(&foreign)?;
        connection.execute_batch("CREATE TABLE user_data(secret TEXT); PRAGMA user_version=4;")?;
        drop(connection);
        let original = fs::read(&foreign)?;
        assert!(check_history_database(&foreign).is_err());
        assert_eq!(fs::read(&foreign)?, original);
        let corrupt = fixture.0.join("corrupt.sqlite3");
        fs::write(&corrupt, b"not a SQLite database")?;
        assert!(check_history_database(&corrupt).is_err());
        assert_eq!(fs::read(&corrupt)?, b"not a SQLite database");
        let source = fixture.database()?;
        let connection = Connection::open(&source)?;
        connection.pragma_update(None, "user_version", 6)?;
        drop(connection);
        assert!(check_history_database(&source).is_err());
        Ok(())
    }

    #[test]
    fn live_wal_backup_is_standalone_and_never_overwrites_or_changes_source() -> Result<()> {
        let fixture = Fixture::new();
        let source = fixture.database()?;
        let writer = Connection::open(&source)?;
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO uploads(filename,origin,url,created_at) VALUES('wal-only.png','https://example.invalid','https://example.invalid/wal','2026-11-14');")?;
        assert!(sidecar(&source, "-wal").is_file());
        let source_before = fs::read(&source)?;
        let wal_before = fs::read(sidecar(&source, "-wal"))?;
        let destination = fixture.0.join("backup.sqlite3");
        backup_history_database(&source, &destination, || false)?;
        assert_eq!(check_history_database(&destination)?.schema_version, 5);
        assert_eq!(fs::read(&source)?, source_before);
        assert_eq!(fs::read(sidecar(&source, "-wal"))?, wal_before);
        let copied = Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        assert_eq!(
            copied.query_row("SELECT count(*) FROM uploads", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        drop(copied);
        let backup_before = fs::read(&destination)?;
        assert!(backup_history_database(&source, &destination, || false).is_err());
        assert_eq!(fs::read(&destination)?, backup_before);
        assert!(!sidecar(&destination, "-wal").exists());
        assert!(!sidecar(&destination, "-shm").exists());
        assert!(
            inspect_part_files(&fixture.0, || false)?
                .candidates
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn backup_cancellation_and_invalid_destinations_leave_no_output_or_part() -> Result<()> {
        let fixture = Fixture::new();
        let source = fixture.database()?;
        let connection = Connection::open(&source)?;
        connection.execute_batch("CREATE TABLE backup_fixture(data BLOB); INSERT INTO backup_fixture VALUES(zeroblob(1048576));")?;
        drop(connection);
        let destination = fixture.0.join("backup.sqlite3");
        assert!(backup_history_database(&source, &destination, || true).is_err());
        let calls = Cell::new(0);
        assert!(
            backup_history_database(&source, &destination, || {
                calls.set(calls.get() + 1);
                calls.get() >= 4
            })
            .is_err()
        );
        assert!(!destination.exists());
        assert!(
            inspect_part_files(&fixture.0, || false)?
                .candidates
                .is_empty()
        );
        fs::write(sidecar(&destination, "-wal"), b"preserve")?;
        assert!(backup_history_database(&source, &destination, || false).is_err());
        assert_eq!(fs::read(sidecar(&destination, "-wal"))?, b"preserve");
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn part_inspection_is_bounded_nonrecursive_and_preserves_every_file() -> Result<()> {
        let fixture = Fixture::new();
        for prefix in [".screenshots-", "settings-"] {
            fs::write(
                fixture
                    .0
                    .join(format!("{prefix}{}.part", uuid::Uuid::new_v4())),
                b"possibly active",
            )?;
        }
        fs::write(fixture.0.join("unknown.part"), b"preserve")?;
        fs::write(fixture.0.join("settings-not-a-uuid.part"), b"preserve")?;
        fs::create_dir(fixture.0.join("nested"))?;
        fs::write(
            fixture
                .0
                .join("nested")
                .join(format!("settings-{}.part", uuid::Uuid::new_v4())),
            b"preserve",
        )?;
        let result = inspect_part_files(&fixture.0, || false)?;
        assert_eq!(result.candidates.len(), 2);
        assert_eq!(result.inspected_entries, 5);
        assert!(!result.truncated);
        assert!(inspect_part_files(&fixture.0, || true).is_err());
        for _ in 0..105 {
            fs::write(
                fixture
                    .0
                    .join(format!("settings-{}.part", uuid::Uuid::new_v4())),
                b"preserve",
            )?;
        }
        let result = inspect_part_files(&fixture.0, || false)?;
        assert_eq!(result.candidates.len(), MAX_PART_CANDIDATES);
        assert!(result.truncated);
        assert_eq!(fs::read_dir(&fixture.0)?.count(), 110);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn linked_ancestors_preserve_live_wal_backup_but_final_file_links_are_rejected() -> Result<()> {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let source = fixture.database()?;
        let writer = Connection::open(&source)?;
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; INSERT INTO uploads(filename,origin,url,created_at) VALUES('wal-only.png','https://example.invalid','https://example.invalid/wal','2026-11-15');")?;
        let root = fixture.0.canonicalize()?;
        let alias = root.join("parent-link");
        symlink(&root, &alias)?;
        let linked_source = alias.join("history.sqlite3");
        let destination = alias.join("backup.sqlite3");
        let source_before = fs::read(&source)?;
        let wal_before = fs::read(sidecar(&source, "-wal"))?;
        assert_eq!(check_history_database(&linked_source)?.schema_version, 5);
        backup_history_database(&linked_source, &destination, || false)?;
        let copied = Connection::open_with_flags(&destination, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        assert_eq!(
            copied.query_row("SELECT count(*) FROM uploads", [], |row| row
                .get::<_, i64>(0))?,
            2
        );
        assert_eq!(fs::read(&source)?, source_before);
        assert_eq!(fs::read(sidecar(&source, "-wal"))?, wal_before);
        let final_link = alias.join("file-link.sqlite3");
        symlink(&source, &final_link)?;
        assert!(check_history_database(&final_link).is_err());
        assert!(
            backup_history_database(&final_link, &alias.join("unused.sqlite3"), || false).is_err()
        );
        assert!(!root.join("unused.sqlite3").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_sources_destinations_and_part_entries_are_rejected_or_skipped() -> Result<()> {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let source = fixture.database()?;
        let link = fixture.0.join("source-link.sqlite3");
        symlink(&source, &link)?;
        assert!(check_history_database(&link).is_err());
        let destination = fixture.0.join("destination.sqlite3");
        symlink(fixture.0.join("missing"), &destination)?;
        assert!(backup_history_database(&source, &destination, || false).is_err());
        symlink(
            &source,
            fixture
                .0
                .join(format!("settings-{}.part", uuid::Uuid::new_v4())),
        )?;
        let directory_link = fixture.0.join("directory-link");
        symlink(&fixture.0, &directory_link)?;
        assert!(inspect_part_files(&directory_link, || false).is_err());
        assert!(
            inspect_part_files(&fixture.0, || false)?
                .candidates
                .is_empty()
        );
        Ok(())
    }
}
