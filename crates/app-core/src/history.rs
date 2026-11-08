use anyhow::{Context, Result, ensure};
use directories::ProjectDirs;
use rusqlite::{Connection, params};
use rusqlite_migration::{M, Migrations};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct HistoryEntry {
    pub id: i64,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryFilter {
    #[default]
    All,
    Captures,
    Imports,
}

const SOURCE_PAGE_SQL: &str = "SELECT id,file_path,width,height,created_at FROM captures WHERE source=?2 AND id < ?1 ORDER BY id DESC LIMIT 21";

pub struct ImportReport {
    pub added: usize,
    pub existing: usize,
    pub failures: Vec<(PathBuf, String)>,
}

fn image_info(path: &Path) -> Result<(u32, u32, i64)> {
    ensure!(path.is_absolute(), "Image path must be absolute");
    path.to_str()
        .context("Image path cannot be stored as UTF-8")?;
    let mut reader = image::ImageReader::with_format(
        std::io::BufReader::new(std::fs::File::open(path)?),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    let (width, height) = reader.into_dimensions()?;
    crate::validate_image_size(width, height)?;
    let size = i64::try_from(std::fs::metadata(path)?.len()).context("Image file is too large")?;
    Ok((width, height, size))
}

fn insert_capture(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    info: (u32, u32, i64),
    source: &str,
) -> Result<usize> {
    Ok(transaction.execute(
        "INSERT INTO captures (file_path,mime_type,width,height,file_size,created_at,source)
        VALUES (?1,'image/png',?2,?3,?4,?5,?6) ON CONFLICT(file_path) DO NOTHING",
        params![
            path.to_str().context("Invalid image path")?,
            info.0,
            info.1,
            info.2,
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            source
        ],
    )?)
}

/// References existing files; never copies, modifies or decodes full image buffers.
pub fn import_capture_files(database: &Path, paths: &[PathBuf]) -> Result<ImportReport> {
    ensure!(
        paths.len() <= 100,
        "Select at most 100 PNG images at a time"
    );
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    let mut report = ImportReport {
        added: 0,
        existing: 0,
        failures: Vec::new(),
    };
    for path in paths {
        match image_info(path) {
            Ok(info) => {
                if insert_capture(&transaction, path, info, "import")? == 0 {
                    report.existing += 1;
                } else {
                    report.added += 1;
                }
            }
            Err(error) => report.failures.push((path.clone(), format!("{error:#}"))),
        }
    }
    transaction.commit()?;
    Ok(report)
}

pub fn history_database_path() -> Result<PathBuf> {
    Ok(ProjectDirs::from("", "lejylabs", "screenshots")
        .context("Application data folder is unavailable")?
        .data_local_dir()
        .join("history.sqlite3"))
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE captures (
        id INTEGER PRIMARY KEY, file_path TEXT NOT NULL UNIQUE,
        thumbnail_path TEXT, mime_type TEXT NOT NULL, width INTEGER NOT NULL,
        height INTEGER NOT NULL, file_size INTEGER NOT NULL,
        created_at TEXT NOT NULL, source TEXT NOT NULL
    );",
        ),
        M::up("CREATE INDEX captures_source_id ON captures(source,id DESC);"),
    ])
}

fn open(path: &Path) -> Result<Connection> {
    std::fs::create_dir_all(path.parent().context("Invalid history database path")?)?;
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-512;",
    )?;
    migrations().to_latest(&mut connection)?;
    Ok(connection)
}

/// Call on a worker, only after a complete PNG has been published.
pub fn record_saved_capture(database: &Path, path: &Path) -> Result<()> {
    let info = image_info(path)?;
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    insert_capture(&transaction, path, info, "capture")?;
    transaction.commit()?;
    Ok(())
}

/// Keyset pagination; at most 21 metadata rows, never decoded image buffers.
pub fn capture_history_page(database: &Path, before: Option<i64>) -> Result<Vec<HistoryEntry>> {
    capture_history_page_filtered(database, before, HistoryFilter::All, || false)
}

/// Indexed source filtering; cancellation is checked before storage work and while reading rows.
pub fn capture_history_page_filtered(
    database: &Path,
    before: Option<i64>,
    filter: HistoryFilter,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<HistoryEntry>> {
    ensure!(!cancelled(), "History request canceled");
    let connection = open(database)?;
    ensure!(!cancelled(), "History request canceled");
    let source = match filter {
        HistoryFilter::All => None,
        HistoryFilter::Captures => Some("capture"),
        HistoryFilter::Imports => Some("import"),
    };
    let sql = if source.is_some() {
        SOURCE_PAGE_SQL
    } else {
        "SELECT id,file_path,width,height,created_at FROM captures WHERE id < ?1 ORDER BY id DESC LIMIT 21"
    };
    let mut query = connection.prepare(sql)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(HistoryEntry {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, String>(1)?),
            width: row.get(2)?,
            height: row.get(3)?,
            created_at: row.get(4)?,
        })
    };
    let before = before.unwrap_or(i64::MAX);
    let rows = if let Some(source) = source {
        query.query_map(params![before, source], map)?
    } else {
        query.query_map([before], map)?
    };
    let mut entries = Vec::with_capacity(21);
    for row in rows {
        ensure!(!cancelled(), "History request canceled");
        entries.push(row?);
    }
    Ok(entries)
}

/// Remove only metadata. Capture files are never modified by this operation.
pub fn remove_history_entry(database: &Path, id: i64) -> Result<()> {
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM captures WHERE id=?1", [id])?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn populate_metadata(connection: &mut Connection, root: &Path, count: i64) -> Result<()> {
        let transaction = connection.transaction()?;
        for index in 0..count {
            let source = if index % 2 == 0 { "capture" } else { "import" };
            let path = root.join("missing").join(format!("{source}-{index}.png"));
            transaction.execute("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source) VALUES (?1,'image/png',2,3,42,'2026-01-01T00:00:00Z',?2)", params![path.to_str().unwrap(), source])?;
        }
        transaction.commit()?;
        Ok(())
    }
    #[test]
    fn source_filters_upgrade_legacy_history_and_use_indexed_keyset_pages() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-filters-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let database = root.join("history.sqlite3");
        let mut legacy = Connection::open(&database)?;
        migrations().to_version(&mut legacy, 1)?;
        populate_metadata(&mut legacy, &root, 80)?;
        drop(legacy);
        assert_eq!(capture_history_page(&database, None)?[0].id, 80);
        for (filter, newest, source) in [
            (HistoryFilter::Captures, 79, "capture"),
            (HistoryFilter::Imports, 80, "import"),
        ] {
            let first = capture_history_page_filtered(&database, None, filter, || false)?;
            assert_eq!(first.len(), 21);
            assert_eq!(first[0].id, newest);
            let older =
                capture_history_page_filtered(&database, Some(first[19].id), filter, || false)?;
            assert_eq!(older.len(), 20);
            let ids = first[..20]
                .iter()
                .chain(&older)
                .map(|entry| entry.id)
                .collect::<Vec<_>>();
            assert_eq!(ids.len(), 40);
            assert!(ids.windows(2).all(|pair| pair[0] - pair[1] == 2));
            assert!(first.iter().chain(&older).all(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(source)
            }));
            assert!(
                capture_history_page_filtered(&database, Some(1), filter, || false)?.is_empty()
            );
        }
        let connection = open(&database)?;
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM captures", [], |row| row
                .get::<_, i64>(0))?,
            80
        );
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {SOURCE_PAGE_SQL}"))?
            .query_map(params![i64::MAX, "import"], |row| row.get::<_, String>(3))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .join(" ");
        assert!(plan.contains("captures_source_id"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        assert!(!root.join("missing").exists());
        drop(connection);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn cancelled_history_fetch_does_not_open_storage_or_return_partial_pages() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-cancel-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("history.sqlite3");
        assert!(
            capture_history_page_filtered(&database, None, HistoryFilter::All, || true).is_err()
        );
        assert!(!root.exists());
        let mut connection = open(&database)?;
        populate_metadata(&mut connection, &root, 40)?;
        drop(connection);
        let calls = std::cell::Cell::new(0);
        assert!(
            capture_history_page_filtered(&database, None, HistoryFilter::Imports, || {
                calls.set(calls.get() + 1);
                calls.get() >= 4
            })
            .is_err()
        );
        assert_eq!(calls.get(), 4);
        assert_eq!(
            capture_history_page_filtered(&database, None, HistoryFilter::Imports, || false)?.len(),
            20
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    #[ignore = "short optimized 10k-row history/filter comparison; synthetic metadata only"]
    fn filtered_history_10k_cost() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-history-cost-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let database = root.join("history.sqlite3");
        let mut legacy = Connection::open(&database)?;
        migrations().to_version(&mut legacy, 1)?;
        populate_metadata(&mut legacy, &root, 10_000)?;
        let pages = legacy.pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))?;
        let page_size = legacy.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))?;
        drop(legacy);
        let start = std::time::Instant::now();
        let connection = open(&database)?;
        let elapsed = start.elapsed();
        let new_pages =
            connection.pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))?;
        drop(connection);
        eprintln!(
            "10k history index migration={elapsed:?}; logical database index growth={} bytes",
            (new_pages - pages) * page_size
        );
        for filter in [
            HistoryFilter::All,
            HistoryFilter::Captures,
            HistoryFilter::Imports,
        ] {
            let mut samples = Vec::new();
            for _ in 0..25 {
                let start = std::time::Instant::now();
                let entries =
                    capture_history_page_filtered(&database, Some(9000), filter, || false)?;
                assert_eq!(entries.len(), 21);
                std::hint::black_box(entries);
                samples.push(start.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "10k history {filter:?}: page p95={:?}; <=21 metadata rows, SQLite cache 512 KiB, no image decode",
                samples[23]
            );
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn importing_keeps_sources_and_reports_duplicates_and_invalid_files() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let png = root.join("existing.png");
        image::RgbaImage::new(3, 2).save(&png)?;
        let original = std::fs::read(&png)?;
        let invalid = root.join("invalid.png");
        std::fs::write(&invalid, b"not a PNG")?;
        let database = root.join("history.sqlite3");
        let report = import_capture_files(&database, &[png.clone(), png.clone(), invalid.clone()])?;
        assert_eq!(
            (report.added, report.existing, report.failures.len()),
            (1, 1, 1)
        );
        assert_eq!(report.failures[0].0, invalid);
        assert_eq!(std::fs::read(&png)?, original);
        assert_eq!(std::fs::read(&invalid)?, b"not a PNG");
        assert!(import_capture_files(&database, &vec![png; 101]).is_err());
        assert_eq!(capture_history_page(&database, None)?.len(), 1);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn history_survives_reopen_and_pages_without_duplicates() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let database = root.join("history.sqlite3");
        for index in 0..25 {
            let path = root.join(format!("{index}.png"));
            image::RgbaImage::new(2, 3).save(&path)?;
            record_saved_capture(&database, &path)?;
            record_saved_capture(&database, &path)?;
        }
        assert!(record_saved_capture(&database, &root.join("missing.png")).is_err());
        let first = capture_history_page(&database, None)?;
        assert_eq!(first.len(), 21);
        assert_eq!(first[0].width, 2);
        let older = capture_history_page(&database, Some(first[19].id))?;
        assert_eq!(older.len(), 5);
        assert!(older.iter().all(|entry| entry.id < first[19].id));
        assert_eq!(
            capture_history_page(&database, None)?[0].path,
            first[0].path
        );
        let image_before = std::fs::read(&first[0].path)?;
        remove_history_entry(&database, first[0].id)?;
        remove_history_entry(&database, first[0].id)?;
        assert_eq!(std::fs::read(&first[0].path)?, image_before);
        assert!(
            capture_history_page(&database, None)?
                .iter()
                .all(|entry| entry.id != first[0].id)
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
