//! Explicit metadata-only queries; existing resident/UI queries keep their contracts.

use crate::{HistoryEntry, HistoryFilter, UploadHistoryEntry};
use anyhow::{Context, Result, ensure};
use jiff::civil::Date;
use rusqlite::{Connection, Row, params_from_iter, types::Value};
use rusqlite::{functions::FunctionFlags, types::ValueRef};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const PAGE_SIZE: usize = 20;
const QUERY_DEADLINE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistorySort {
    #[default]
    Newest,
    Oldest,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryQuery {
    /// Exclusive ID in the selected sort direction; keep filters unchanged between pages.
    pub cursor: Option<i64>,
    pub sort: HistorySort,
    pub filter: HistoryFilter,
    pub text: String,
    /// Inclusive UTC dates, in strict YYYY-MM-DD form.
    pub from: Option<String>,
    pub through: Option<String>,
}

pub struct HistoryPage<T> {
    pub entries: Vec<T>,
    pub has_more: bool,
    pub next_cursor: Option<i64>,
}

/// Self-contained exclusive date boundary; never resolved by looking up its ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryDateCursor {
    pub created_at: String,
    pub id: i64,
}

impl std::fmt::Display for HistoryDateCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.created_at, self.id)
    }
}

impl std::str::FromStr for HistoryDateCursor {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        ensure!(value.len() <= 85, "Invalid history date cursor");
        let (created_at, id) = value
            .rsplit_once('/')
            .context("Invalid history date cursor")?;
        let cursor = Self {
            created_at: created_at.into(),
            id: id.parse().context("Invalid history date cursor")?,
        };
        cursor.validate()?;
        Ok(cursor)
    }
}

impl HistoryDateCursor {
    fn validate(&self) -> Result<()> {
        ensure!(self.id > 0, "Invalid history date cursor");
        timestamp_key(&self.created_at)?;
        Ok(())
    }
}

pub struct HistoryDatePage<T> {
    pub entries: Vec<T>,
    pub has_more: bool,
    pub next_cursor: Option<HistoryDateCursor>,
}

#[derive(Clone, Copy)]
enum Kind {
    Captures,
    Uploads,
}

fn date(value: &str) -> Result<Date> {
    ensure!(
        value.len() == 10
            && value.bytes().enumerate().all(|(index, byte)| {
                if index == 4 || index == 7 {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit()
                }
            }),
        "History dates must use YYYY-MM-DD"
    );
    value.parse::<Date>().context("Invalid history date")
}

fn validate_query(query: &HistoryQuery, kind: Kind) -> Result<Option<String>> {
    ensure!(
        query.cursor.is_none_or(|id| id > 0),
        "Invalid history cursor"
    );
    ensure!(
        query.text.len() <= 512,
        "Search is limited to 512 UTF-8 bytes"
    );
    let from = query.from.as_deref().map(date).transpose()?;
    let through = query.through.as_deref().map(date).transpose()?;
    ensure!(
        from.zip(through).is_none_or(|(a, b)| a <= b),
        "History date range is reversed"
    );
    match kind {
        Kind::Captures if !query.text.trim().is_empty() => {
            Ok(Some(crate::history::search_expression(&query.text)?))
        }
        Kind::Captures => Ok(None),
        Kind::Uploads => {
            ensure!(
                query.filter == HistoryFilter::All,
                "Upload history has no capture source filter"
            );
            Ok(None)
        }
    }
}

fn check_active(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "History request canceled");
    ensure!(Instant::now() < deadline, "History query deadline exceeded");
    Ok(())
}

fn open_query(
    database: &Path,
    cancel: &Arc<AtomicBool>,
    deadline: Instant,
    kind: Kind,
    search: bool,
) -> Result<Connection> {
    check_active(cancel, deadline)?;
    let mut connection = crate::storage::open_read_only(database)?;
    check_active(cancel, deadline)?;
    let flag = cancel.clone();
    connection.progress_handler(
        1_000,
        Some(move || flag.load(Ordering::Relaxed) || Instant::now() >= deadline),
    )?;
    connection.execute_batch("PRAGMA cache_size=-512; BEGIN DEFERRED;")?;
    let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    ensure!(
        (1..=10).contains(&version),
        "Unsupported history schema version"
    );
    let (table, columns) = match kind {
        Kind::Captures => ("captures", "id,file_path,width,height,created_at,source"),
        Kind::Uploads => {
            ensure!(version >= 4, "Upload history requires schema version 4");
            ("uploads", "id,filename,origin,url,created_at")
        }
    };
    ensure!(
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 AND type='table')",
            [table],
            |row| row.get::<_, bool>(0)
        )?,
        "History database schema is incomplete"
    );
    // Both identifiers are selected from constants above, never user input.
    connection.prepare(&format!("SELECT {columns} FROM {table} LIMIT 0"))?;
    if matches!(kind, Kind::Captures) && version >= 2 {
        ensure!(
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='captures_source_id' AND type='index' AND tbl_name='captures')",
                [], |row| row.get::<_, bool>(0)
            )?,
            "History source index is unavailable"
        );
    }
    if search {
        ensure!(version >= 3, "Capture search requires schema version 3");
        ensure!(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='captures_search' AND type='table')",
            [], |row| row.get::<_, bool>(0)
        )?, "History search index is unavailable");
        connection
            .prepare("SELECT rowid,file_path,created_at,source FROM captures_search LIMIT 0")?;
    }
    let pages: u32 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let page_size: u32 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    ensure!(
        u64::from(pages) * u64::from(page_size) <= 1024 * 1024 * 1024,
        "History database exceeds the 1 GiB maintenance limit"
    );
    check_active(cancel, deadline)?;
    Ok(connection)
}

fn bounded_text(column: &str, bytes: usize) -> String {
    // Project NULL for malformed/oversized fields so decoding fails without copying their data.
    format!(
        "CASE WHEN typeof({column})='text' AND length(CAST({column} AS BLOB))<={bytes} THEN {column} END"
    )
}

fn bounded_integer(column: &str, maximum: i64) -> String {
    // SQLite column affinity does not prevent BLOB/TEXT/REAL values in released schemas.
    format!(
        "CASE WHEN typeof({column})='integer' AND {column} BETWEEN 1 AND {maximum} THEN {column} END"
    )
}

fn query_sql(query: &HistoryQuery, kind: Kind, expression: Option<&str>) -> (String, Vec<Value>) {
    query_sql_ordered(query, kind, expression, false, None)
}

fn query_sql_ordered(
    query: &HistoryQuery,
    kind: Kind,
    expression: Option<&str>,
    chronological: bool,
    cursor: Option<&HistoryDateCursor>,
) -> (String, Vec<Value>) {
    let mut values = Vec::new();
    // FTS can produce rowids in order directly; ordering its joined alias creates a temp sort.
    let key = if expression.is_some() {
        "captures_search.rowid"
    } else {
        "c.id"
    };
    let selected_id = if chronological {
        bounded_integer("c.id", i64::MAX)
    } else {
        "c.id".into()
    };
    let timestamp_key = if chronological {
        format!(
            "screenshots_history_time({}, {selected_id})",
            bounded_text("c.created_at", 64)
        )
    } else {
        String::new()
    };
    let mut sql = match kind {
        Kind::Captures => format!(
            "SELECT {selected_id},{},{},{},{} FROM captures c{} WHERE 1=1",
            bounded_text("c.file_path", 4096),
            bounded_text("c.created_at", 64),
            if chronological {
                bounded_integer("c.width", i64::from(u32::MAX))
            } else {
                "c.width".into()
            },
            if chronological {
                bounded_integer("c.height", i64::from(u32::MAX))
            } else {
                "c.height".into()
            },
            if expression.is_some() {
                // Date admission proves c.id aliases the fixed-width INTEGER rowid.
                " JOIN captures_search ON captures_search.rowid=c.id".into()
            } else {
                String::new()
            }
        ),
        Kind::Uploads => format!(
            "SELECT {selected_id},{},{},{},{} FROM uploads c WHERE 1=1",
            bounded_text("c.filename", 1024),
            bounded_text("c.origin", 2048),
            bounded_text("c.url", 2048),
            bounded_text("c.created_at", 64)
        ),
    };
    if chronological {
        if let Some(cursor) = cursor {
            values.push(Value::Text(cursor.created_at.clone()));
            values.push(Value::Integer(cursor.id));
            let comparison = if query.sort == HistorySort::Newest {
                "<"
            } else {
                ">"
            };
            sql.push_str(&format!(
                " AND ({timestamp_key},{selected_id}){comparison}(screenshots_history_time(?1,?2),?2)"
            ));
        }
    } else if let Some(cursor) = query.cursor {
        values.push(Value::Integer(cursor));
        let comparison = if query.sort == HistorySort::Newest {
            "<"
        } else {
            ">"
        };
        sql.push_str(&format!(" AND {key}{comparison}?{}", values.len()));
    }
    // No date index exists. Comparing ten-byte calendar prefixes also accepts legacy dates.
    for (value, comparison) in [(&query.from, ">="), (&query.through, "<=")] {
        if let Some(value) = value {
            values.push(Value::Text(value.clone()));
            sql.push_str(&format!(
                " AND substr(c.created_at,1,10){comparison}?{}",
                values.len()
            ));
        }
    }
    if matches!(kind, Kind::Captures) {
        let source = match query.filter {
            HistoryFilter::All => None,
            HistoryFilter::Captures => Some("capture"),
            HistoryFilter::Imports => Some("import"),
        };
        if let Some(source) = source {
            values.push(Value::Text(source.into()));
            sql.push_str(&format!(" AND c.source=?{}", values.len()));
        }
        if let Some(expression) = expression {
            values.push(Value::Text(expression.into()));
            sql.push_str(&format!(" AND captures_search MATCH ?{}", values.len()));
        }
    } else if !query.text.is_empty() {
        values.push(Value::Text(query.text.clone()));
        let parameter = values.len();
        // Malformed fields reach the bounded projection and fail decoding, instead of
        // allocating their entire contents for lower() or silently disappearing.
        sql.push_str(&format!(" AND CASE WHEN typeof(c.filename)!='text' OR length(CAST(c.filename AS BLOB))>1024 OR typeof(c.origin)!='text' OR length(CAST(c.origin AS BLOB))>2048 OR typeof(c.url)!='text' OR length(CAST(c.url AS BLOB))>2048 THEN 1 ELSE (instr(lower(c.filename),lower(?{parameter}))>0 OR instr(lower(c.origin),lower(?{parameter}))>0 OR instr(lower(c.url),lower(?{parameter}))>0) END"));
    }
    let direction = if query.sort == HistorySort::Newest {
        "DESC"
    } else {
        "ASC"
    };
    if chronological {
        // A computed first key prevents a partial/index-prefix sort; LIMIT keeps a top-21 tree.
        sql.push_str(&format!(
            " ORDER BY {timestamp_key} {direction},{selected_id} {direction} LIMIT 21"
        ));
    } else {
        sql.push_str(&format!(" ORDER BY {key} {direction} LIMIT 21"));
    }
    (sql, values)
}

fn validate_timestamp(value: &str) -> Result<()> {
    if value.len() == 10 {
        date(value)?;
    } else {
        let parsed =
            shared::timestamp::parse_rfc3339(value).context("Invalid history timestamp")?;
        ensure!(
            parsed.offset_seconds == 0
                && value.get(..10).is_some_and(|prefix| date(prefix).is_ok()),
            "History timestamp must use UTC"
        );
    }
    Ok(())
}

// Preserve every accepted fractional digit; SQLite's date functions round to milliseconds.
fn timestamp_key(value: &str) -> Result<String> {
    ensure!(value.len() <= 64, "Invalid history timestamp");
    validate_timestamp(value)?;
    if value.len() == 10 {
        return Ok(format!("{value}T00:00:00."));
    }
    // Validation guarantees the fixed-width date/time prefix; normalize the RFC3339 separator.
    let mut key = format!("{}T{}.", &value[..10], &value[11..19]);
    if value.as_bytes().get(19) == Some(&b'.') {
        let fraction = value[20..]
            .split(|character: char| !character.is_ascii_digit())
            .next()
            .unwrap_or("");
        key.push_str(fraction.trim_end_matches('0'));
    }
    Ok(key)
}

fn validate_date_primary_key(connection: &Connection, kind: Kind) -> Result<()> {
    let table = match kind {
        Kind::Captures => "captures",
        Kind::Uploads => "uploads",
    };
    // No PK index distinguishes an INTEGER rowid alias from affinity-only PK DESC/WITHOUT ROWID.
    ensure!(connection.query_row(
        "SELECT count(*)=1 AND coalesce(max(name='id' AND upper(type)='INTEGER' AND pk=1),0) AND NOT EXISTS(SELECT 1 FROM pragma_index_list(?1) WHERE origin='pk') FROM pragma_table_info(?1) WHERE pk>0",
        [table], |row| row.get::<_, bool>(0),
    )?, "History date ordering requires an INTEGER PRIMARY KEY rowid ID");
    Ok(())
}

fn register_timestamp_key(
    connection: &Connection,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
) -> Result<()> {
    connection.create_scalar_function(
        "screenshots_history_time",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_DIRECTONLY,
        move |context| {
            let result = (|| -> Result<String> {
                check_active(&cancel, deadline)?;
                // Validate the bounded ID before keyset comparison can exclude an invalid tie.
                ensure!(
                    matches!(context.get_raw(1), ValueRef::Integer(id) if id > 0),
                    "Invalid history ID"
                );
                // Borrow SQLite text and check size before parsing or allocating a Rust string.
                let ValueRef::Text(bytes) = context.get_raw(0) else {
                    anyhow::bail!("Invalid history timestamp");
                };
                ensure!(bytes.len() <= 64, "Invalid history timestamp");
                timestamp_key(std::str::from_utf8(bytes).context("Invalid history timestamp")?)
            })();
            result.map_err(|_| {
                rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                    "History timestamp/ID query failed or was interrupted",
                )))
            })
        },
    )?;
    Ok(())
}

fn capture_row(row: &Row<'_>) -> Result<HistoryEntry> {
    let entry = HistoryEntry {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        created_at: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
    };
    ensure!(
        entry.id > 0
            && entry.path.is_absolute()
            && !entry.path.to_string_lossy().chars().any(char::is_control),
        "Invalid capture history metadata"
    );
    crate::validate_image_size(entry.width, entry.height)?;
    validate_timestamp(&entry.created_at)?;
    Ok(entry)
}

fn upload_row(row: &Row<'_>) -> Result<UploadHistoryEntry> {
    let entry = UploadHistoryEntry {
        id: row.get(0)?,
        filename: row.get(1)?,
        origin: row.get(2)?,
        url: row.get(3)?,
        created_at: row.get(4)?,
    };
    ensure!(
        entry.id > 0
            && Path::new(&entry.filename)
                .file_name()
                .is_some_and(|name| name == entry.filename.as_str())
            && !entry.filename.chars().any(char::is_control),
        "Invalid upload history metadata"
    );
    let origin = reqwest::Url::parse(&entry.origin).context("Invalid upload history origin")?;
    ensure!(
        matches!(origin.scheme(), "http" | "https")
            && origin.origin().ascii_serialization() == entry.origin,
        "Invalid upload history origin"
    );
    crate::upload::validate_link(&entry.url)?;
    validate_timestamp(&entry.created_at)?;
    Ok(entry)
}

fn read_page<T>(
    connection: &Connection,
    query: &HistoryQuery,
    kind: Kind,
    expression: Option<&str>,
    cancel: &AtomicBool,
    deadline: Instant,
    map: impl Fn(&Row<'_>) -> Result<T>,
    id: impl Fn(&T) -> i64,
) -> Result<HistoryPage<T>> {
    let (sql, values) = query_sql(query, kind, expression);
    let (entries, has_more) = read_entries(connection, &sql, values, cancel, deadline, map)?;
    let next_cursor = if has_more {
        entries.last().map(id)
    } else {
        None
    };
    Ok(HistoryPage {
        entries,
        has_more,
        next_cursor,
    })
}

fn read_entries<T>(
    connection: &Connection,
    sql: &str,
    values: Vec<Value>,
    cancel: &AtomicBool,
    deadline: Instant,
    map: impl Fn(&Row<'_>) -> Result<T>,
) -> Result<(Vec<T>, bool)> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut entries = Vec::with_capacity(PAGE_SIZE + 1);
    while let Some(row) = rows.next()? {
        check_active(cancel, deadline)?;
        entries.push(map(row)?);
    }
    check_active(cancel, deadline)?;
    let has_more = entries.len() > PAGE_SIZE;
    entries.truncate(PAGE_SIZE);
    Ok((entries, has_more))
}

fn read_date_page<T>(
    database: &Path,
    query: &HistoryQuery,
    cursor: Option<&HistoryDateCursor>,
    cancel: Arc<AtomicBool>,
    kind: Kind,
    map: impl Fn(&Row<'_>) -> Result<T>,
    boundary: impl Fn(&T) -> HistoryDateCursor,
) -> Result<HistoryDatePage<T>> {
    let deadline = Instant::now() + QUERY_DEADLINE;
    check_active(&cancel, deadline)?;
    ensure!(
        query.cursor.is_none(),
        "Date history queries require a timestamp/ID cursor"
    );
    if let Some(cursor) = cursor {
        cursor.validate()?;
    }
    let expression = validate_query(query, kind)?;
    let connection = open_query(database, &cancel, deadline, kind, expression.is_some())?;
    validate_date_primary_key(&connection, kind)?;
    register_timestamp_key(&connection, cancel.clone(), deadline)?;
    if expression.as_deref() == Some("") {
        check_active(&cancel, deadline)?;
        return Ok(HistoryDatePage {
            entries: Vec::new(),
            has_more: false,
            next_cursor: None,
        });
    }
    let (sql, values) = query_sql_ordered(query, kind, expression.as_deref(), true, cursor);
    let (entries, has_more) = read_entries(&connection, &sql, values, &cancel, deadline, map)?;
    let next_cursor = if has_more {
        entries.last().map(boundary)
    } else {
        None
    };
    Ok(HistoryDatePage {
        entries,
        has_more,
        next_cursor,
    })
}

/// Worker-side UTC timestamp ordering with ID ties, without changing insertion-ID queries.
/// `sort` chooses direction; `query.cursor` must be absent. Keep filters/direction unchanged.
/// Pages use independent snapshots: later mutations may move rows across the saved boundary.
pub fn query_capture_history_by_date(
    database: &Path,
    query: &HistoryQuery,
    cursor: Option<&HistoryDateCursor>,
    cancel: Arc<AtomicBool>,
) -> Result<HistoryDatePage<HistoryEntry>> {
    read_date_page(
        database,
        query,
        cursor,
        cancel,
        Kind::Captures,
        capture_row,
        |entry| HistoryDateCursor {
            created_at: entry.created_at.clone(),
            id: entry.id,
        },
    )
}

/// Worker-side upload timestamp ordering; no URL, image or anchor-row lookup occurs.
pub fn query_upload_history_by_date(
    database: &Path,
    query: &HistoryQuery,
    cursor: Option<&HistoryDateCursor>,
    cancel: Arc<AtomicBool>,
) -> Result<HistoryDatePage<UploadHistoryEntry>> {
    read_date_page(
        database,
        query,
        cursor,
        cancel,
        Kind::Uploads,
        upload_row,
        |entry| HistoryDateCursor {
            created_at: entry.created_at.clone(),
            id: entry.id,
        },
    )
}

/// Worker-side read-only query. Dates and optional prefix search never decode image files.
pub fn query_capture_history(
    database: &Path,
    query: &HistoryQuery,
    cancel: Arc<AtomicBool>,
) -> Result<HistoryPage<HistoryEntry>> {
    let deadline = Instant::now() + QUERY_DEADLINE;
    check_active(&cancel, deadline)?;
    let expression = validate_query(query, Kind::Captures)?;
    let connection = open_query(
        database,
        &cancel,
        deadline,
        Kind::Captures,
        expression.is_some(),
    )?;
    if expression.as_deref() == Some("") {
        return Ok(HistoryPage {
            entries: Vec::new(),
            has_more: false,
            next_cursor: None,
        });
    }
    read_page(
        &connection,
        query,
        Kind::Captures,
        expression.as_deref(),
        &cancel,
        deadline,
        capture_row,
        |entry| entry.id,
    )
}

/// Worker-side read-only upload metadata search; returned URLs are never fetched.
pub fn query_upload_history(
    database: &Path,
    query: &HistoryQuery,
    cancel: Arc<AtomicBool>,
) -> Result<HistoryPage<UploadHistoryEntry>> {
    let deadline = Instant::now() + QUERY_DEADLINE;
    check_active(&cancel, deadline)?;
    validate_query(query, Kind::Uploads)?;
    let connection = open_query(database, &cancel, deadline, Kind::Uploads, false)?;
    read_page(
        &connection,
        query,
        Kind::Uploads,
        None,
        &cancel,
        deadline,
        upload_row,
        |entry| entry.id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    struct Fixture {
        root: PathBuf,
        database: PathBuf,
    }

    impl Fixture {
        fn new(count: i64) -> Result<Self> {
            let root =
                std::env::temp_dir().join(format!("screenshots-query-{}", uuid::Uuid::new_v4()));
            let database = root.join("history.sqlite3");
            // Exercise the application's real migrations only while constructing the fixture.
            crate::capture_history_page(&database, None)?;
            let mut connection = crate::history::open_existing(&database)?;
            let transaction = connection.transaction()?;
            {
                let mut capture = transaction.prepare("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,'image/png',2,3,42,?2,?3)")?;
                let mut upload = transaction.prepare("INSERT INTO uploads(filename,origin,url,created_at) VALUES(?1,'https://example.invalid',?2,?3)")?;
                for id in 1..=count {
                    let source = if id % 2 == 0 { "capture" } else { "import" };
                    let date = if id % 2 == 0 {
                        "2024-02-29T23:59:59Z"
                    } else {
                        "2024-03-01"
                    };
                    capture.execute(params![
                        root.join(format!("{source}-{id}.png")).to_str().unwrap(),
                        date,
                        source
                    ])?;
                    upload.execute(params![
                        format!("image-{id}.png"),
                        format!("https://example.invalid/share/{id}?token=a%25_b"),
                        date
                    ])?;
                }
            }
            transaction.commit()?;
            Ok(Self { root, database })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn active() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn captures(fixture: &Fixture, query: &HistoryQuery) -> Result<HistoryPage<HistoryEntry>> {
        query_capture_history(&fixture.database, query, active())
    }

    fn uploads(fixture: &Fixture, query: &HistoryQuery) -> Result<HistoryPage<UploadHistoryEntry>> {
        query_upload_history(&fixture.database, query, active())
    }

    #[test]
    fn ten_thousand_metadata_rows_have_bounded_stable_pages_and_indexed_plans() -> Result<()> {
        let fixture = Fixture::new(10_000)?;
        for sort in [HistorySort::Newest, HistorySort::Oldest] {
            let mut query = HistoryQuery {
                sort,
                ..Default::default()
            };
            let first = captures(&fixture, &query)?;
            assert_eq!(first.entries.len(), 20);
            assert!(first.has_more);
            assert_eq!(
                first.entries[0].id,
                if sort == HistorySort::Newest {
                    10_000
                } else {
                    1
                }
            );
            query.cursor = first.next_cursor;
            let second = captures(&fixture, &query)?;
            assert_eq!(second.entries.len(), 20);
            assert!(
                first
                    .entries
                    .iter()
                    .all(|entry| second.entries.iter().all(|other| entry.id != other.id))
            );
            let upload = uploads(&fixture, &query)?;
            assert_eq!(
                upload
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
                second
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>()
            );
        }
        let connection = crate::history::open_existing(&fixture.database)?;
        for (kind, query, expression, index) in [
            (
                Kind::Captures,
                HistoryQuery {
                    cursor: Some(9_000),
                    ..Default::default()
                },
                None,
                "INTEGER PRIMARY KEY",
            ),
            (
                Kind::Uploads,
                HistoryQuery {
                    cursor: Some(9_000),
                    sort: HistorySort::Oldest,
                    ..Default::default()
                },
                None,
                "INTEGER PRIMARY KEY",
            ),
            (
                Kind::Captures,
                HistoryQuery {
                    cursor: Some(9_000),
                    filter: HistoryFilter::Captures,
                    ..Default::default()
                },
                None,
                "captures_source_id",
            ),
            (
                Kind::Captures,
                HistoryQuery {
                    cursor: Some(9_000),
                    ..Default::default()
                },
                Some("\"capture\"*"),
                "VIRTUAL TABLE INDEX",
            ),
        ] {
            let (sql, values) = query_sql(&query, kind, expression);
            let plan = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                .query_map(params_from_iter(values), |row| row.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .join(" ");
            assert!(
                plan.contains(index) && !plan.contains("TEMP B-TREE"),
                "{plan}"
            );
        }
        assert_eq!(
            std::fs::read_dir(&fixture.root)?
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "png"))
                .count(),
            0
        );
        Ok(())
    }

    #[test]
    fn source_text_and_inclusive_utc_dates_compose_without_timestamp_sorting() -> Result<()> {
        let fixture = Fixture::new(60)?;
        let mut query = HistoryQuery {
            filter: HistoryFilter::Captures,
            text: "capture".into(),
            from: Some("2024-02-29".into()),
            through: Some("2024-02-29".into()),
            ..Default::default()
        };
        let first = captures(&fixture, &query)?;
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            (22..=60).rev().step_by(2).collect::<Vec<_>>()
        );
        query.cursor = first.next_cursor;
        let last = captures(&fixture, &query)?;
        assert_eq!(last.entries.len(), 10);
        assert!(!last.has_more && last.next_cursor.is_none());
        query = HistoryQuery {
            from: Some("2024-03-01".into()),
            through: Some("2024-03-01".into()),
            ..Default::default()
        };
        assert_eq!(captures(&fixture, &query)?.entries[0].id, 59);
        let connection = crate::history::open_existing(&fixture.database)?;
        connection.execute(
            "UPDATE captures SET created_at='2001-01-01T00:00:00Z' WHERE id=60",
            [],
        )?;
        assert_eq!(
            captures(&fixture, &HistoryQuery::default())?.entries[0].id,
            60
        );
        Ok(())
    }

    #[test]
    fn upload_substrings_are_literal_and_do_not_open_urls() -> Result<()> {
        let fixture = Fixture::new(25)?;
        for text in ["IMAGE-25.PNG", "a%25_b", "EXAMPLE.INVALID"] {
            let page = uploads(
                &fixture,
                &HistoryQuery {
                    text: text.into(),
                    ..Default::default()
                },
            )?;
            assert!(!page.entries.is_empty());
        }
        for text in ["a%_b", "' OR 1=1 --", "nonexistent"] {
            assert!(
                uploads(
                    &fixture,
                    &HistoryQuery {
                        text: text.into(),
                        ..Default::default()
                    }
                )?
                .entries
                .is_empty()
            );
        }
        assert!(
            captures(
                &fixture,
                &HistoryQuery {
                    text: "***".into(),
                    ..Default::default()
                }
            )?
            .entries
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn invalid_options_and_precancel_do_not_create_storage() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-no-query-{}", uuid::Uuid::new_v4()));
        let database = root.join("missing.sqlite3");
        let invalid = [
            HistoryQuery {
                cursor: Some(0),
                ..Default::default()
            },
            HistoryQuery {
                cursor: Some(-1),
                ..Default::default()
            },
            HistoryQuery {
                from: Some("2023-02-29".into()),
                ..Default::default()
            },
            HistoryQuery {
                from: Some("2024-2-29".into()),
                ..Default::default()
            },
            HistoryQuery {
                from: Some("2025-01-01".into()),
                through: Some("2024-01-01".into()),
                ..Default::default()
            },
            HistoryQuery {
                text: "x".repeat(513),
                ..Default::default()
            },
            HistoryQuery {
                text: "x ".repeat(17),
                ..Default::default()
            },
        ];
        for query in invalid {
            assert!(query_capture_history(&database, &query, active()).is_err());
        }
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(query_upload_history(&database, &HistoryQuery::default(), cancel).is_err());
        assert!(
            query_upload_history(
                &database,
                &HistoryQuery {
                    filter: HistoryFilter::Imports,
                    ..Default::default()
                },
                active()
            )
            .is_err()
        );
        assert!(!root.exists());
        Ok(())
    }

    #[test]
    fn queries_preserve_database_bytes_and_legacy_schema() -> Result<()> {
        let fixture = Fixture::new(3)?;
        let before = std::fs::read(&fixture.database)?;
        captures(&fixture, &HistoryQuery::default())?;
        uploads(&fixture, &HistoryQuery::default())?;
        assert_eq!(std::fs::read(&fixture.database)?, before);
        let legacy = fixture.root.join("legacy.sqlite3");
        let connection = Connection::open(&legacy)?;
        connection.execute_batch("CREATE TABLE captures(id INTEGER PRIMARY KEY,file_path TEXT,width INTEGER,height INTEGER,created_at TEXT,source TEXT); PRAGMA user_version=1;")?;
        drop(connection);
        let before = std::fs::read(&legacy)?;
        assert!(
            query_capture_history(&legacy, &HistoryQuery::default(), active())?
                .entries
                .is_empty()
        );
        assert!(
            query_capture_history(
                &legacy,
                &HistoryQuery {
                    text: "capture".into(),
                    ..Default::default()
                },
                active()
            )
            .is_err()
        );
        assert!(query_upload_history(&legacy, &HistoryQuery::default(), active()).is_err());
        assert_eq!(std::fs::read(&legacy)?, before);
        Ok(())
    }

    #[test]
    fn malformed_selected_metadata_fails_without_truncation() -> Result<()> {
        let fixture = Fixture::new(1)?;
        let connection = crate::history::open_existing(&fixture.database)?;
        for value in [
            "x".repeat(4097),
            "relative.png".into(),
            "/absolute/control\n.png".into(),
        ] {
            connection.execute("UPDATE captures SET file_path=?1 WHERE id=1", [value])?;
            assert!(captures(&fixture, &HistoryQuery::default()).is_err());
        }
        connection.execute_batch("PRAGMA ignore_check_constraints=ON;")?;
        connection.execute(
            "UPDATE uploads SET url=?1 WHERE id=1",
            [format!("https://example.invalid/{}", "x".repeat(3000))],
        )?;
        assert!(
            uploads(
                &fixture,
                &HistoryQuery {
                    text: "unrelated".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        connection.execute("UPDATE uploads SET url='file:///secret',created_at='2024-02-29T00:00:00+01:00' WHERE id=1", [])?;
        assert!(uploads(&fixture, &HistoryQuery::default()).is_err());
        connection.execute(
            "UPDATE uploads SET url='https://example.invalid/valid' WHERE id=1",
            [],
        )?;
        assert!(uploads(&fixture, &HistoryQuery::default()).is_err());
        Ok(())
    }

    #[test]
    fn sqlite_work_is_interrupted_by_cancel_and_deadline() -> Result<()> {
        let fixture = Fixture::new(1)?;
        for cancelled in [false, true] {
            let cancel = active();
            let deadline = Instant::now() + QUERY_DEADLINE;
            let connection =
                open_query(&fixture.database, &cancel, deadline, Kind::Captures, false)?;
            cancel.store(cancelled, Ordering::Relaxed);
            assert!(connection.query_row("WITH RECURSIVE numbers(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM numbers WHERE x<100000000) SELECT sum(x) FROM numbers", [], |row| row.get::<_, i64>(0)).is_err());
            assert!(cancelled || Instant::now() >= deadline);
        }
        Ok(())
    }

    #[test]
    #[ignore = "optimized 10k metadata page timing; no images or networking"]
    fn history_query_10k_cost() -> Result<()> {
        let fixture = Fixture::new(10_000)?;
        for (label, kind, query) in [
            ("capture ID", Kind::Captures, HistoryQuery::default()),
            (
                "capture source/date",
                Kind::Captures,
                HistoryQuery {
                    filter: HistoryFilter::Captures,
                    from: Some("2024-02-29".into()),
                    through: Some("2024-02-29".into()),
                    ..Default::default()
                },
            ),
            (
                "capture FTS",
                Kind::Captures,
                HistoryQuery {
                    text: "capture".into(),
                    ..Default::default()
                },
            ),
            (
                "upload substring",
                Kind::Uploads,
                HistoryQuery {
                    text: "image-1".into(),
                    ..Default::default()
                },
            ),
            (
                "upload full scan",
                Kind::Uploads,
                HistoryQuery {
                    text: "no-match".into(),
                    ..Default::default()
                },
            ),
        ] {
            let mut samples = Vec::new();
            for _ in 0..25 {
                let started = Instant::now();
                match kind {
                    Kind::Captures => {
                        assert!(captures(&fixture, &query)?.entries.len() <= 20);
                    }
                    Kind::Uploads => {
                        assert!(uploads(&fixture, &query)?.entries.len() <= 20);
                    }
                }
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            eprintln!(
                "10k {label} page p50={:?} p95={:?} max={:?}",
                samples[12], samples[23], samples[24]
            );
        }
        let mut samples = Vec::new();
        for _ in 0..25 {
            let started = Instant::now();
            crate::capture_history_page(&fixture.database, None)?;
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "10k existing capture page p50={:?} p95={:?} max={:?}",
            samples[12], samples[23], samples[24]
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "history_date_query_tests.rs"]
mod date_tests;
