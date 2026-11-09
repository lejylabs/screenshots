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
        M::up("CREATE VIRTUAL TABLE captures_search USING fts5(file_path,created_at,source,content='captures',content_rowid='id');
            CREATE TRIGGER captures_search_insert AFTER INSERT ON captures BEGIN
                INSERT INTO captures_search(rowid,file_path,created_at,source) VALUES(new.id,new.file_path,new.created_at,new.source); END;
            CREATE TRIGGER captures_search_delete AFTER DELETE ON captures BEGIN
                INSERT INTO captures_search(captures_search,rowid,file_path,created_at,source) VALUES('delete',old.id,old.file_path,old.created_at,old.source); END;
            CREATE TRIGGER captures_search_update AFTER UPDATE ON captures BEGIN
                INSERT INTO captures_search(captures_search,rowid,file_path,created_at,source) VALUES('delete',old.id,old.file_path,old.created_at,old.source);
                INSERT INTO captures_search(rowid,file_path,created_at,source) VALUES(new.id,new.file_path,new.created_at,new.source); END;
            INSERT INTO captures_search(captures_search) VALUES('rebuild');"),
        M::up("CREATE TABLE uploads (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            filename TEXT NOT NULL CHECK(length(filename)<=1024),
            origin TEXT NOT NULL CHECK(length(origin)<=2048),
            url TEXT NOT NULL CHECK(length(url)<=2048), created_at TEXT NOT NULL
        );"),
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

/// Successful upload metadata contains no pixels or full source path.
#[derive(Clone)]
pub struct UploadHistoryEntry {
    pub id: i64,
    pub filename: String,
    pub origin: String,
    pub url: String,
    pub created_at: String,
}

pub fn record_uploaded_png(database: &Path, source: &Path, origin: &str, url: &str) -> Result<()> {
    let filename = source
        .file_name()
        .context("Invalid upload filename")?
        .to_string_lossy();
    ensure!(
        filename.len() <= 1024 && origin.len() <= 2048,
        "Upload metadata exceeds its size limit"
    );
    let parsed = reqwest::Url::parse(origin)?;
    ensure!(
        matches!(parsed.scheme(), "https" | "http")
            && parsed.origin().ascii_serialization() == origin,
        "Invalid upload origin"
    );
    let link = crate::upload::validate_link(url)?;
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO uploads(filename,origin,url,created_at) VALUES(?1,?2,?3,?4)",
        params![
            filename,
            origin,
            link,
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn upload_history_page(
    database: &Path,
    before: Option<i64>,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<UploadHistoryEntry>> {
    ensure!(!cancelled(), "Upload history request canceled");
    let connection = open(database)?;
    ensure!(!cancelled(), "Upload history request canceled");
    let mut statement = connection.prepare("SELECT id,filename,origin,url,created_at FROM uploads WHERE id < ?1 ORDER BY id DESC LIMIT 21")?;
    let rows = statement.query_map([before.unwrap_or(i64::MAX)], |row| {
        Ok(UploadHistoryEntry {
            id: row.get(0)?,
            filename: row.get(1)?,
            origin: row.get(2)?,
            url: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut entries = Vec::with_capacity(21);
    for row in rows {
        ensure!(!cancelled(), "Upload history request canceled");
        let entry = row?;
        crate::upload::validate_link(&entry.url)?;
        entries.push(entry);
    }
    Ok(entries)
}

pub fn remove_upload_history_entry(database: &Path, id: i64) -> Result<()> {
    ensure!(id > 0, "Invalid upload history ID");
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM uploads WHERE id=?1", [id])?;
    transaction.commit()?;
    Ok(())
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

const SEARCH_PAGE_SQL: &str = "SELECT c.id,c.file_path,c.width,c.height,c.created_at
    FROM captures_search JOIN captures c ON c.id=captures_search.rowid
    WHERE captures_search MATCH ?1 AND captures_search.rowid < ?2
    AND (?3 IS NULL OR c.source=?3) ORDER BY captures_search.rowid DESC LIMIT 21";

fn search_expression(query: &str) -> Result<String> {
    ensure!(query.len() <= 512, "Search is limited to 512 UTF-8 bytes");
    let words: Vec<_> = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    ensure!(words.len() <= 16, "Use at most 16 search words");
    Ok(words
        .into_iter()
        .map(|word| format!("\"{word}\"*"))
        .collect::<Vec<_>>()
        .join(" AND "))
}

/// Literal word-prefix search; bounded pages, SQLite instruction-level cancellation and deadline.
pub fn search_capture_history(
    database: &Path,
    before: Option<i64>,
    filter: HistoryFilter,
    query: &str,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Vec<HistoryEntry>> {
    use std::sync::atomic::Ordering;
    ensure!(!cancel.load(Ordering::Relaxed), "History request canceled");
    let expression = search_expression(query)?;
    if query.trim().is_empty() {
        return capture_history_page_filtered(database, before, filter, || {
            cancel.load(Ordering::Relaxed)
        });
    }
    if expression.is_empty() {
        return Ok(Vec::new());
    }
    let connection = open(database)?;
    let flag = cancel.clone();
    let started = std::time::Instant::now();
    connection.progress_handler(
        1000,
        Some(move || {
            flag.load(Ordering::Relaxed) || started.elapsed() > Duration::from_millis(250)
        }),
    )?;
    ensure!(!cancel.load(Ordering::Relaxed), "History request canceled");
    let source = match filter {
        HistoryFilter::All => None,
        HistoryFilter::Captures => Some("capture"),
        HistoryFilter::Imports => Some("import"),
    };
    let mut query = connection.prepare(SEARCH_PAGE_SQL)?;
    let rows = query.query_map(
        params![expression, before.unwrap_or(i64::MAX), source],
        |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                width: row.get(2)?,
                height: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )?;
    let mut entries = Vec::with_capacity(21);
    for row in rows {
        ensure!(!cancel.load(Ordering::Relaxed), "History request canceled");
        entries.push(row?);
    }
    Ok(entries)
}

/// Reassign an existing entry without changing its source, creation time or image files.
pub fn relink_history_entry(database: &Path, entry: &HistoryEntry, path: &Path) -> Result<()> {
    ensure!(entry.id > 0, "Invalid history entry ID");
    let info = image_info(path)?;
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    let updated = transaction
        .execute(
            "UPDATE captures SET file_path=?1,width=?2,height=?3,file_size=?4,thumbnail_path=NULL
         WHERE id=?5 AND file_path=?6 AND created_at=?7",
            params![
                path.to_str().context("Invalid image path")?,
                info.0,
                info.1,
                info.2,
                entry.id,
                entry.path.to_str().context("Invalid original image path")?,
                entry.created_at
            ],
        )
        .context(
            "Could not relink image; the selected path may already belong to another history entry",
        )?;
    ensure!(
        updated == 1,
        "History entry changed or was removed; refresh history and try again"
    );
    transaction.commit()?;
    Ok(())
}

/// Remove only metadata. Capture files are never modified by this operation.
pub fn remove_history_entry(database: &Path, id: i64) -> Result<()> {
    remove_history_entries(database, &[id]).map(|_| ())
}

/// Atomically remove a displayed page's selected metadata; image files remain untouched.
pub fn remove_history_entries(database: &Path, ids: &[i64]) -> Result<usize> {
    ensure!(
        !ids.is_empty() && ids.len() <= 20,
        "Select 1 to 20 history entries"
    );
    ensure!(ids.iter().all(|id| *id > 0), "Invalid history entry ID");
    let mut connection = open(database)?;
    let transaction = connection.transaction()?;
    let mut removed = 0;
    {
        let mut delete = transaction.prepare("DELETE FROM captures WHERE id=?1")?;
        for id in ids {
            removed += delete.execute([id])?;
        }
    }
    transaction.commit()?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "short optimized 10k-row upload metadata paging probe; no files or networking"]
    fn upload_history_10k_page_cost() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-page-cost-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("history.sqlite3");
        let mut connection = open(&database)?;
        let transaction = connection.transaction()?;
        {
            let mut insert=transaction.prepare("INSERT INTO uploads(filename,origin,url,created_at) VALUES('image.png','https://example.invalid',?1,'2026-11-09T00:00:00Z')")?;
            for index in 0..10_000 {
                insert.execute([format!("https://example.invalid/share/{index}")])?;
            }
        }
        transaction.commit()?;
        drop(connection);
        let mut samples = Vec::new();
        for _ in 0..25 {
            let started = std::time::Instant::now();
            let page = upload_history_page(&database, Some(9000), || false)?;
            samples.push(started.elapsed());
            assert_eq!(page.len(), 21);
        }
        samples.sort_unstable();
        eprintln!(
            "10k upload metadata page p95={:?}; 21 rows, no image/file/network work",
            samples[23]
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn upload_history_upgrades_pages_and_removes_metadata_without_source_files() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-history-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("history.sqlite3");
        assert!(upload_history_page(&database, None, || true).is_err());
        assert!(!root.exists());
        assert!(
            record_uploaded_png(
                &database,
                Path::new("private.png"),
                "https://example.invalid",
                "javascript:bad"
            )
            .is_err()
        );
        assert!(!root.exists());
        std::fs::create_dir_all(&root)?;
        let mut legacy = Connection::open(&database)?;
        migrations().to_version(&mut legacy, 3)?;
        populate_metadata(&mut legacy, &root, 2)?;
        drop(legacy);
        let started = std::time::Instant::now();
        for index in 0..25 {
            record_uploaded_png(
                &database,
                &root
                    .join("private-folder")
                    .join(format!("image-{index}.png")),
                "https://example.invalid",
                &format!("https://example.invalid/share/{index}"),
            )?;
        }
        eprintln!(
            "25 durable upload metadata records: {:?}",
            started.elapsed()
        );
        let first = upload_history_page(&database, None, || false)?;
        assert_eq!(first.len(), 21);
        assert_eq!(first[0].filename, "image-24.png");
        let older = upload_history_page(&database, Some(first[19].id), || false)?;
        assert_eq!(older.len(), 5);
        assert!(older.iter().all(|entry| entry.id < first[19].id));
        assert_eq!(capture_history_page(&database, None)?.len(), 2);
        let highest = first[0].id;
        remove_upload_history_entry(&database, highest)?;
        remove_upload_history_entry(&database, highest)?;
        record_uploaded_png(
            &database,
            Path::new("new.png"),
            "https://example.invalid",
            "https://example.invalid/new",
        )?;
        assert!(upload_history_page(&database, None, || false)?[0].id > highest);
        assert!(!root.join("private-folder").exists());
        let connection = open(&database)?;
        let plan=connection.prepare("EXPLAIN QUERY PLAN SELECT id,filename,origin,url,created_at FROM uploads WHERE id < ?1 ORDER BY id DESC LIMIT 21")?.query_map([i64::MAX],|row|row.get::<_,String>(3))?.collect::<rusqlite::Result<Vec<_>>>()?.join(" ");
        assert!(
            plan.contains("INTEGER PRIMARY KEY") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
        drop(connection);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    fn search(
        database: &Path,
        before: Option<i64>,
        filter: HistoryFilter,
        text: &str,
    ) -> Result<Vec<HistoryEntry>> {
        search_capture_history(
            database,
            before,
            filter,
            text,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }
    #[test]
    fn indexed_search_upgrades_pages_and_tracks_relinks_and_removals() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-search-{}",
            uuid::Uuid::new_v4()
        ));
        let database = root.join("history.sqlite3");
        assert!(search(&database, None, HistoryFilter::All, &"x".repeat(513)).is_err());
        assert!(search(&database, None, HistoryFilter::All, &"word ".repeat(17)).is_err());
        assert!(
            search(&database, None, HistoryFilter::All, "*** ()")
                .unwrap()
                .is_empty()
        );
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        assert!(
            search_capture_history(&database, None, HistoryFilter::All, "capture", cancel).is_err()
        );
        assert!(!root.exists());
        std::fs::create_dir_all(&root)?;
        let mut legacy = Connection::open(&database)?;
        migrations().to_version(&mut legacy, 2)?;
        populate_metadata(&mut legacy, &root, 80)?;
        drop(legacy);
        let first = search(&database, None, HistoryFilter::All, "CAPT 2026-01")?;
        assert_eq!(first.len(), 21);
        assert_eq!(first[0].id, 79);
        let older = search(
            &database,
            Some(first[19].id),
            HistoryFilter::All,
            "capt 2026-01",
        )?;
        assert_eq!(older.len(), 20);
        let ids: Vec<_> = first[..20]
            .iter()
            .chain(&older)
            .map(|entry| entry.id)
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] - pair[1] == 2));
        assert!(search(&database, None, HistoryFilter::Imports, "capture")?.is_empty());
        assert!(search(&database, None, HistoryFilter::All, "capture\" OR \"import")?.is_empty());
        assert_eq!(
            search(&database, None, HistoryFilter::Imports, "import")?.len(),
            21
        );
        assert_eq!(search(&database, None, HistoryFilter::All, " ")?.len(), 21);
        let plan = open(&database)?
            .prepare(&format!("EXPLAIN QUERY PLAN {SEARCH_PAGE_SQL}"))?
            .query_map(
                params!["\"capture\"*", i64::MAX, Option::<&str>::None],
                |row| row.get::<_, String>(3),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .join(" ");
        assert!(plan.contains("VIRTUAL TABLE INDEX"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        let png = root.join("Holiday-Grüße.png");
        image::RgbaImage::new(2, 3).save(&png)?;
        import_capture_files(&database, &[png.clone()])?;
        let entry = search(&database, None, HistoryFilter::Imports, "HOLID grü")?
            .pop()
            .unwrap();
        let renamed = root.join("Invoice.png");
        std::fs::rename(&png, &renamed)?;
        relink_history_entry(&database, &entry, &renamed)?;
        assert!(search(&database, None, HistoryFilter::All, "holiday")?.is_empty());
        assert_eq!(
            search(&database, None, HistoryFilter::All, "invoice")?[0].id,
            entry.id
        );
        remove_history_entries(&database, &[entry.id])?;
        assert!(search(&database, None, HistoryFilter::All, "invoice")?.is_empty());
        assert!(renamed.exists());
        assert!(!root.join("missing").exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn relinking_preserves_identity_files_and_rejects_conflicts_or_stale_entries() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-relink-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let database = root.join("history.sqlite3");
        let original = root.join("original.png");
        let moved = root.join("moved.png");
        let imported = root.join("import.png");
        let replacement = root.join("replacement.png");
        image::RgbaImage::new(2, 3).save(&original)?;
        image::RgbaImage::new(4, 5).save(&imported)?;
        image::RgbaImage::new(6, 7).save(&replacement)?;
        record_saved_capture(&database, &original)?;
        import_capture_files(&database, &[imported.clone()])?;
        let initial = capture_history_page(&database, None)?;
        let entry = initial
            .iter()
            .find(|entry| entry.path == original)
            .unwrap()
            .clone();
        let import_entry = initial
            .iter()
            .find(|entry| entry.path == imported)
            .unwrap()
            .clone();
        let original_bytes = std::fs::read(&original)?;
        let import_bytes = std::fs::read(&imported)?;
        let replacement_bytes = std::fs::read(&replacement)?;
        std::fs::rename(&original, &moved)?;

        // A duplicate target or rejected header must leave the missing reference recoverable.
        let invalid = root.join("invalid.png");
        std::fs::write(&invalid, b"not a PNG")?;
        for path in [&imported, &invalid, &original, Path::new("relative.png")] {
            assert!(relink_history_entry(&database, &entry, path).is_err());
            assert_eq!(capture_history_page(&database, None)?[1].path, original);
        }
        let started = std::time::Instant::now();
        relink_history_entry(&database, &entry, &moved)?;
        println!(
            "PNG header validation + durable relink: {:?}",
            started.elapsed()
        );
        let linked = capture_history_page(&database, None)?[1].clone();
        assert_eq!((linked.id, linked.width, linked.height), (entry.id, 2, 3));
        assert_eq!(linked.created_at, entry.created_at);
        assert_eq!(linked.path, moved);
        assert!(relink_history_entry(&database, &entry, &replacement).is_err());

        relink_history_entry(&database, &import_entry, &replacement)?;
        let linked_import =
            capture_history_page_filtered(&database, None, HistoryFilter::Imports, || false)?;
        assert_eq!(linked_import.len(), 1);
        assert_eq!(
            (
                linked_import[0].id,
                linked_import[0].width,
                linked_import[0].height
            ),
            (import_entry.id, 6, 7)
        );
        assert_eq!(linked_import[0].created_at, import_entry.created_at);
        assert_eq!(linked_import[0].path, replacement);
        assert_eq!(
            capture_history_page_filtered(&database, None, HistoryFilter::Captures, || false)?
                .len(),
            1
        );
        remove_history_entry(&database, linked.id)?;
        assert!(relink_history_entry(&database, &linked, &moved).is_err());
        assert!(!original.exists());
        assert_eq!(std::fs::read(&moved)?, original_bytes);
        assert_eq!(std::fs::read(&imported)?, import_bytes);
        assert_eq!(std::fs::read(&replacement)?, replacement_bytes);
        assert_eq!(std::fs::read(&invalid)?, b"not a PNG");
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn bulk_removal_is_bounded_atomic_and_keeps_files() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-history-bulk-{}", uuid::Uuid::new_v4()));
        let database = root.join("history.sqlite3");
        for ids in [vec![], vec![1; 21], vec![0], vec![-1]] {
            assert!(remove_history_entries(&database, &ids).is_err());
        }
        assert!(!root.exists());
        let mut connection = open(&database)?;
        populate_metadata(&mut connection, &root, 40)?;
        let image = root.join("kept.png");
        image::RgbaImage::new(2, 3).save(&image)?;
        let bytes = std::fs::read(&image)?;
        connection.execute(
            "UPDATE captures SET file_path=?1 WHERE id=1",
            [image.to_str().unwrap()],
        )?;
        connection.execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON captures WHEN OLD.id=2 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;")?;
        assert!(remove_history_entries(&database, &[1, 2]).is_err());
        assert_eq!(
            search(&database, None, HistoryFilter::All, "kept")?[0].id,
            1
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM captures", [], |row| row
                .get::<_, i64>(0))?,
            40
        );
        assert_eq!(capture_history_page(&database, Some(3))?.len(), 2);
        connection.execute_batch("DROP TRIGGER reject_delete;")?;
        assert_eq!(remove_history_entries(&database, &[1, 1, 2, 999])?, 2);
        assert!(search(&database, None, HistoryFilter::All, "kept")?.is_empty());
        assert_eq!(std::fs::read(&image)?, bytes);
        let ids: Vec<_> = (3..=22).collect();
        let started = std::time::Instant::now();
        assert_eq!(remove_history_entries(&database, &ids)?, 20);
        println!(
            "20 metadata removals in one durable transaction: {:?}",
            started.elapsed()
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM captures", [], |row| row
                .get::<_, i64>(0))?,
            18
        );
        remove_history_entry(&database, 1)?;
        drop(connection);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

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
            4
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
        for (text, filter, expected) in [
            ("capt", HistoryFilter::All, 21),
            ("import 2026-01", HistoryFilter::Imports, 21),
            ("notfound", HistoryFilter::All, 0),
        ] {
            let mut samples = Vec::new();
            for _ in 0..25 {
                let start = std::time::Instant::now();
                assert_eq!(search(&database, Some(9000), filter, text)?.len(), expected);
                samples.push(start.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "10k indexed search {text:?}/{filter:?}: page p95={:?}",
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
