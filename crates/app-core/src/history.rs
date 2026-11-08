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

fn open(path: &Path) -> Result<Connection> {
    std::fs::create_dir_all(path.parent().context("Invalid history database path")?)?;
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-512;",
    )?;
    Migrations::new(vec![M::up(
        "CREATE TABLE captures (
        id INTEGER PRIMARY KEY, file_path TEXT NOT NULL UNIQUE,
        thumbnail_path TEXT, mime_type TEXT NOT NULL, width INTEGER NOT NULL,
        height INTEGER NOT NULL, file_size INTEGER NOT NULL,
        created_at TEXT NOT NULL, source TEXT NOT NULL
    );",
    )])
    .to_latest(&mut connection)?;
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
    let connection = open(database)?;
    let mut query = connection.prepare(
        "SELECT id,file_path,width,height,created_at FROM captures
        WHERE id < ?1 ORDER BY id DESC LIMIT 21",
    )?;
    let rows = query.query_map([before.unwrap_or(i64::MAX)], |row| {
        Ok(HistoryEntry {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, String>(1)?),
            width: row.get(2)?,
            height: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
