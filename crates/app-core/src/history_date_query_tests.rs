use super::*;
use rusqlite::params;

struct Fixture {
    root: PathBuf,
    database: PathBuf,
}

impl Fixture {
    fn new(count: i64) -> Result<Self> {
        let root =
            std::env::temp_dir().join(format!("screenshots-date-query-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let database = root.join("history.sqlite3");
        crate::capture_history_page(&database, None)?;
        let mut connection = crate::history::open_existing(&database)?;
        let transaction = connection.transaction()?;
        for id in 1..=count {
            // Dates deliberately disagree with ID order; no images exist.
            let created_at = stamp(id);
            transaction.execute("INSERT INTO captures(id,file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,?2,'image/png',2,3,42,?3,?4)", params![id, root.join(format!("capture-{id}.png")).to_str().unwrap(), created_at, if id % 2 == 0 { "capture" } else { "import" }])?;
            transaction.execute("INSERT INTO uploads(id,filename,origin,url,created_at) VALUES(?1,?2,'https://example.invalid',?3,?4)", params![id, format!("image-{id}.png"), format!("https://example.invalid/share/{id}"), created_at])?;
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

fn stamp(id: i64) -> &'static str {
    match id % 8 {
        0 => "2024-03-01",
        1 => "2024-02-29T23:59:59.1000000002Z",
        2 => "2024-03-01T00:00:00+00:00",
        3 => "2024-02-29T23:59:59.1000000001Z",
        4 => "2024-02-29T23:59:59.1Z",
        5 => "2024-03-01t00:00:00z",
        6 => "2024-02-29T23:59:59.1000000000-00:00",
        _ => "2024-03-01T00:00:00.000Z",
    }
}

fn active() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[test]
fn exact_date_keys_normalize_utc_without_rounding_fractional_digits() -> Result<()> {
    let midnight = timestamp_key("2024-03-01")?;
    for value in [
        "2024-03-01T00:00:00Z",
        "2024-03-01t00:00:00z",
        "2024-03-01T00:00:00.000+00:00",
        "2024-03-01T00:00:00-00:00",
    ] {
        assert_eq!(timestamp_key(value)?, midnight);
    }
    let values = [
        "2024-02-29T23:59:59Z",
        "2024-02-29T23:59:59.1Z",
        "2024-02-29T23:59:59.1000000001Z",
        "2024-02-29T23:59:59.1000000002Z",
        "2024-02-29T23:59:59.99999999999999999999999Z",
        "2024-02-29T23:59:60Z",
        "2024-03-01",
    ];
    for pair in values.windows(2) {
        assert!(timestamp_key(pair[0])? < timestamp_key(pair[1])?);
    }
    for value in [
        "2024-02-30",
        "2024-03-01T00:00:00+01:00",
        "2024-03-01T00:00:00Z/private",
        "",
        "∞",
    ] {
        assert!(timestamp_key(value).is_err());
    }
    assert!(timestamp_key(&format!("2024-03-01T00:00:00.{}Z", "1".repeat(64))).is_err());
    for value in [
        "2024-03-01/0",
        "2024-03-01/-1",
        "2024-03-01/9223372036854775808",
        "garbage/1",
        &"x".repeat(86),
    ] {
        assert!(value.parse::<HistoryDateCursor>().is_err());
    }
    let cursor: HistoryDateCursor = "2024-02-29T23:59:59.1000000002Z/42".parse()?;
    assert_eq!(cursor.to_string(), "2024-02-29T23:59:59.1000000002Z/42");
    Ok(())
}

#[test]
fn chronological_ten_thousand_rows_page_exactly_in_both_directions() -> Result<()> {
    let fixture = Fixture::new(10_000)?;
    let before = std::fs::read(&fixture.database)?;
    for sort in [HistorySort::Oldest, HistorySort::Newest] {
        let mut expected = (1..=10_000).collect::<Vec<i64>>();
        expected.sort_unstable_by_key(|id| {
            (
                match id % 8 {
                    4 | 6 => 0,
                    3 => 1,
                    1 => 2,
                    _ => 3,
                },
                *id,
            )
        });
        if sort == HistorySort::Newest {
            expected.reverse();
        }
        let mut insertion_order = (1..=10_000).collect::<Vec<i64>>();
        if sort == HistorySort::Newest {
            insertion_order.reverse();
        }
        assert_ne!(expected, insertion_order);
        let query = HistoryQuery {
            sort,
            ..Default::default()
        };
        let mut cursor = None;
        let mut visited = Vec::new();
        loop {
            let page = query_capture_history_by_date(
                &fixture.database,
                &query,
                cursor.as_ref(),
                active(),
            )?;
            assert!(page.entries.len() <= 20);
            let upload =
                query_upload_history_by_date(&fixture.database, &query, cursor.as_ref(), active())?;
            assert_eq!(
                page.entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
                upload
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>()
            );
            visited.extend(page.entries.iter().map(|entry| entry.id));
            assert_eq!(page.has_more, page.next_cursor.is_some());
            assert_eq!(upload.next_cursor, page.next_cursor);
            cursor = page.next_cursor;
            if !page.has_more {
                break;
            }
            assert!(visited.len() < 10_000);
        }
        assert_eq!(visited, expected);
    }
    assert_eq!(std::fs::read(&fixture.database)?, before);
    assert!(!fixture.root.join("capture-1.png").exists());
    Ok(())
}

#[test]
fn chronological_filters_and_id_order_remain_distinct() -> Result<()> {
    let fixture = Fixture::new(60)?;
    let query = HistoryQuery {
        sort: HistorySort::Oldest,
        filter: HistoryFilter::Captures,
        text: "capture".into(),
        from: Some("2024-02-29".into()),
        through: Some("2024-02-29".into()),
        ..Default::default()
    };
    let page = query_capture_history_by_date(&fixture.database, &query, None, active())?;
    assert!(
        page.entries
            .iter()
            .all(|entry| entry.id % 2 == 0 && entry.created_at.starts_with("2024-02-29"))
    );
    assert_eq!(page.entries.first().unwrap().id, 4);
    let old = query_capture_history(
        &fixture.database,
        &HistoryQuery {
            sort: HistorySort::Oldest,
            ..Default::default()
        },
        active(),
    )?;
    assert_eq!(old.entries.first().unwrap().id, 1);
    assert_eq!(old.next_cursor, Some(20));
    let uploads = query_upload_history_by_date(
        &fixture.database,
        &HistoryQuery {
            sort: HistorySort::Oldest,
            text: "/share/1".into(),
            ..Default::default()
        },
        None,
        active(),
    )?;
    assert!(
        uploads
            .entries
            .iter()
            .all(|entry| entry.url.contains("/share/1"))
    );
    assert!(query_upload_history_by_date(&fixture.database, &query, None, active()).is_err());
    Ok(())
}

#[test]
fn date_cursor_survives_anchor_delete_or_retimestamp_and_inserts() -> Result<()> {
    for sort in [HistorySort::Newest, HistorySort::Oldest] {
        let fixture = Fixture::new(100)?;
        let query = HistoryQuery {
            sort,
            ..Default::default()
        };
        let first = query_capture_history_by_date(&fixture.database, &query, None, active())?;
        let cursor = first.next_cursor.unwrap();
        let expected =
            query_capture_history_by_date(&fixture.database, &query, Some(&cursor), active())?;
        let expected_ids = expected
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        let connection = crate::history::open_existing(&fixture.database)?;
        // Update the anchor to the already-visited side; its saved token must still win.
        let moved = if sort == HistorySort::Newest {
            "9999-12-31"
        } else {
            "0001-01-01"
        };
        connection.execute(
            "UPDATE captures SET created_at=?1 WHERE id=?2",
            params![moved, cursor.id],
        )?;
        let next =
            query_capture_history_by_date(&fixture.database, &query, Some(&cursor), active())?;
        assert_eq!(
            next.entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            expected_ids
        );
        connection.execute("DELETE FROM captures WHERE id=?1", [cursor.id])?;
        connection.execute("INSERT INTO captures(file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,'image/png',2,3,42,?2,'capture')", params![fixture.root.join("new.png").to_str().unwrap(), moved])?;
        let next =
            query_capture_history_by_date(&fixture.database, &query, Some(&cursor), active())?;
        assert_eq!(
            next.entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            expected_ids
        );
        let moved_id = expected_ids[0];
        connection.execute(
            "UPDATE captures SET created_at=?1 WHERE id=?2",
            params![moved, moved_id],
        )?;
        let next =
            query_capture_history_by_date(&fixture.database, &query, Some(&cursor), active())?;
        assert!(!next.entries.iter().any(|entry| entry.id == moved_id));
        assert_eq!(next.entries.len(), 20);
    }
    Ok(())
}

#[test]
fn date_query_validation_precedes_storage_and_rejects_malformed_fields() -> Result<()> {
    let root =
        std::env::temp_dir().join(format!("screenshots-date-absent-{}", uuid::Uuid::new_v4()));
    let database = root.join("absent.sqlite3");
    assert!(
        query_capture_history_by_date(
            &database,
            &HistoryQuery {
                cursor: Some(1),
                ..Default::default()
            },
            None,
            active()
        )
        .is_err()
    );
    assert!(
        query_capture_history_by_date(
            &database,
            &HistoryQuery::default(),
            Some(&HistoryDateCursor {
                created_at: "2024-03-01".into(),
                id: 0
            }),
            active()
        )
        .is_err()
    );
    assert!(
        query_capture_history_by_date(
            &database,
            &HistoryQuery::default(),
            None,
            Arc::new(AtomicBool::new(true))
        )
        .is_err()
    );
    assert!(!root.exists());
    let fixture = Fixture::new(1)?;
    let connection = crate::history::open_existing(&fixture.database)?;
    for value in [
        "garbage".to_string(),
        "2024-03-01T00:00:00+01:00".into(),
        "x".repeat(65),
    ] {
        connection.execute("UPDATE captures SET created_at=?1", [&value])?;
        connection.execute("UPDATE uploads SET created_at=?1", [&value])?;
        assert!(
            query_capture_history_by_date(
                &fixture.database,
                &HistoryQuery::default(),
                None,
                active()
            )
            .is_err()
        );
        assert!(
            query_upload_history_by_date(
                &fixture.database,
                &HistoryQuery::default(),
                None,
                active()
            )
            .is_err()
        );
    }
    // Model foreign/corrupt metadata that bypassed the current writer constraints.
    connection.execute_batch("PRAGMA ignore_check_constraints=ON;")?;
    connection.execute(
        "UPDATE captures SET created_at='2024-03-01',file_path=?1",
        ["x".repeat(4097)],
    )?;
    connection.execute(
        "UPDATE uploads SET created_at='2024-03-01',filename=?1",
        ["x".repeat(1025)],
    )?;
    assert!(
        query_capture_history_by_date(&fixture.database, &HistoryQuery::default(), None, active())
            .is_err()
    );
    assert!(
        query_upload_history_by_date(&fixture.database, &HistoryQuery::default(), None, active())
            .is_err()
    );
    Ok(())
}

#[test]
fn date_sort_is_read_only_on_legacy_schemas() -> Result<()> {
    let fixture = Fixture::new(0)?;
    let database = fixture.root.join("legacy.sqlite3");
    let connection = Connection::open(&database)?;
    connection.execute_batch("CREATE TABLE captures(id INTEGER PRIMARY KEY,file_path TEXT,width INTEGER,height INTEGER,created_at TEXT,source TEXT); PRAGMA user_version=1;")?;
    for (id, timestamp) in [(1, "2024-03-01"), (2, "2024-02-29T23:59:59.0000000001Z")] {
        connection.execute(
            "INSERT INTO captures VALUES(?1,?2,2,3,?3,'capture')",
            params![
                id,
                fixture.root.join(format!("{id}.png")).to_str().unwrap(),
                timestamp
            ],
        )?;
    }
    drop(connection);
    let before = std::fs::read(&database)?;
    let page = query_capture_history_by_date(
        &database,
        &HistoryQuery {
            sort: HistorySort::Oldest,
            ..Default::default()
        },
        None,
        active(),
    )?;
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert!(
        query_upload_history_by_date(&database, &HistoryQuery::default(), None, active()).is_err()
    );
    assert_eq!(std::fs::read(&database)?, before);
    let connection = Connection::open(&database)?;
    connection.execute_batch("CREATE INDEX captures_source_id ON captures(source,id DESC); CREATE TABLE uploads(id INTEGER PRIMARY KEY,filename TEXT,origin TEXT,url TEXT,created_at TEXT); INSERT INTO uploads VALUES(1,'image-1.png','https://example.invalid','https://example.invalid/1','2024-03-01'),(2,'image-2.png','https://example.invalid','https://example.invalid/2','2024-02-29T23:59:59.1000000001Z'); PRAGMA user_version=4;")?;
    drop(connection);
    let before = std::fs::read(&database)?;
    let page = query_upload_history_by_date(
        &database,
        &HistoryQuery {
            sort: HistorySort::Oldest,
            ..Default::default()
        },
        None,
        active(),
    )?;
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(std::fs::read(&database)?, before);
    Ok(())
}

#[test]
fn date_sort_uses_bounded_top_n_and_interruptible_scalar_work() -> Result<()> {
    let fixture = Fixture::new(100)?;
    for kind in [Kind::Captures, Kind::Uploads] {
        for sort in [HistorySort::Newest, HistorySort::Oldest] {
            let cancel = active();
            let deadline = Instant::now() + QUERY_DEADLINE;
            let connection = open_query(&fixture.database, &cancel, deadline, kind, false)?;
            register_timestamp_key(&connection, cancel.clone(), deadline)?;
            for mode in 0..3 {
                let mut query = HistoryQuery {
                    sort,
                    ..Default::default()
                };
                if mode == 2 {
                    query.from = Some("2024-02-29".into());
                    query.text = if matches!(kind, Kind::Captures) {
                        "capture"
                    } else {
                        "image"
                    }
                    .into();
                    if matches!(kind, Kind::Captures) {
                        query.filter = HistoryFilter::Captures;
                    }
                }
                let cursor = HistoryDateCursor {
                    created_at: "2024-03-01".into(),
                    id: 50,
                };
                let expression = validate_query(&query, kind)?;
                let (sql, values) = query_sql_ordered(
                    &query,
                    kind,
                    expression.as_deref(),
                    true,
                    if mode == 1 { Some(&cursor) } else { None },
                );
                let instructions = connection
                    .prepare(&format!("EXPLAIN {sql}"))?
                    .query_map(params_from_iter(values), |row| {
                        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let opcodes = instructions
                    .iter()
                    .map(|(opcode, _)| opcode.as_str())
                    .collect::<Vec<_>>();
                assert!(!opcodes.contains(&"SorterOpen"));
                assert_eq!(
                    opcodes
                        .iter()
                        .filter(|opcode| **opcode == "OpenEphemeral")
                        .count(),
                    1
                );
                assert!(
                    instructions
                        .iter()
                        .any(|(opcode, value)| opcode == "Integer" && *value == 21)
                );
                for expected in ["OpenEphemeral", "IfNotZero", "Last", "Delete", "IdxInsert"] {
                    assert!(opcodes.contains(&expected), "{expected}: {opcodes:?}");
                }
            }
            cancel.store(true, Ordering::Relaxed);
            assert!(
                connection
                    .query_row(
                        "SELECT screenshots_history_time('2024-03-01',1)",
                        [],
                        |row| { row.get::<_, String>(0) }
                    )
                    .is_err()
            );
        }
    }
    let cancel = active();
    let deadline = Instant::now() + QUERY_DEADLINE;
    let connection = open_query(&fixture.database, &cancel, deadline, Kind::Captures, false)?;
    register_timestamp_key(&connection, cancel, Instant::now())?;
    assert!(
        connection
            .query_row(
                "SELECT screenshots_history_time('2024-03-01',1)",
                [],
                |row| { row.get::<_, String>(0) }
            )
            .is_err()
    );
    register_timestamp_key(&connection, active(), Instant::now() + QUERY_DEADLINE)?;
    for sql in [
        "SELECT screenshots_history_time(NULL,1)",
        "SELECT screenshots_history_time(42,1)",
        "SELECT screenshots_history_time(zeroblob(65),1)",
        "SELECT screenshots_history_time('2024-03-01',NULL)",
        "SELECT screenshots_history_time('2024-03-01',1.5)",
        "SELECT screenshots_history_time('2024-03-01',0)",
    ] {
        assert!(
            connection
                .query_row(sql, [], |row| row.get::<_, String>(0))
                .is_err()
        );
    }
    Ok(())
}

#[test]
#[ignore = "optimized 10k chronological metadata-only timing; no images/network"]
fn history_date_query_10k_cost() -> Result<()> {
    let fixture = Fixture::new(10_000)?;
    for (label, query, upload, chronological) in [
        ("existing ID", HistoryQuery::default(), false, false),
        ("capture date", HistoryQuery::default(), false, true),
        (
            "capture date source/text",
            HistoryQuery {
                text: "capture".into(),
                filter: HistoryFilter::Captures,
                ..Default::default()
            },
            false,
            true,
        ),
        ("upload date", HistoryQuery::default(), true, true),
        (
            "upload date substring",
            HistoryQuery {
                text: "image-1".into(),
                ..Default::default()
            },
            true,
            true,
        ),
    ] {
        let mut samples = Vec::new();
        for _ in 0..25 {
            let start = Instant::now();
            let count = if upload {
                query_upload_history_by_date(&fixture.database, &query, None, active())?
                    .entries
                    .len()
            } else if chronological {
                query_capture_history_by_date(&fixture.database, &query, None, active())?
                    .entries
                    .len()
            } else {
                query_capture_history(&fixture.database, &query, active())?
                    .entries
                    .len()
            };
            assert!(count <= 20);
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "10k {label} p50={:?} p95={:?} max={:?}",
            samples[12], samples[23], samples[24]
        );
    }
    Ok(())
}

#[test]
fn malformed_numeric_dimensions_are_bounded_before_sort_payload_on_released_schema() -> Result<()> {
    use rusqlite::types::Value;
    let fixture = Fixture::new(1)?;
    let connection = crate::history::open_existing(&fixture.database)?;
    let malformed = [
        Value::Blob(vec![0; 8 * 1024 * 1024]),
        Value::Text("x".repeat(8 * 1024 * 1024)),
        Value::Real(1.5),
        Value::Integer(-1),
        Value::Integer(0),
        Value::Integer(i64::from(u32::MAX) + 1),
        Value::Integer(i64::MAX),
    ];
    for column in ["width", "height"] {
        for value in &malformed {
            // Released tables use INTEGER affinity/NOT NULL, which still admit these values.
            connection.execute(&format!("UPDATE captures SET {column}=?1"), [value])?;
            let bounded = bounded_integer(&format!("c.{column}"), i64::from(u32::MAX));
            let projected: rusqlite::types::Value =
                connection.query_row(&format!("SELECT {bounded} FROM captures c"), [], |row| {
                    row.get(0)
                })?;
            assert_eq!(projected, Value::Null);
            for sort in [HistorySort::Newest, HistorySort::Oldest] {
                let query = HistoryQuery {
                    sort,
                    ..Default::default()
                };
                assert!(
                    query_capture_history_by_date(&fixture.database, &query, None, active())
                        .is_err()
                );
            }
        }
        connection.execute(&format!("UPDATE captures SET {column}=2"), [])?;
    }
    // Range-valid integers still receive the existing decoded-pixel budget validation.
    connection.execute("UPDATE captures SET width=4294967295,height=4294967295", [])?;
    assert!(
        query_capture_history_by_date(&fixture.database, &HistoryQuery::default(), None, active())
            .is_err()
    );
    Ok(())
}

#[test]
fn legacy_numeric_payload_guards_reject_foreign_ids_and_keep_exact_keyset_boundaries() -> Result<()>
{
    use rusqlite::types::Value;
    let fixture = Fixture::new(0)?;
    for version in [1, 4, 10] {
        let database = fixture.root.join(format!("numeric-{version}.sqlite3"));
        let connection = Connection::open(&database)?;
        // DESC prevents SQLite's INTEGER PRIMARY KEY rowid alias: affinity is not a type guarantee.
        connection.execute_batch("CREATE TABLE captures(id INTEGER PRIMARY KEY DESC,file_path TEXT,width INTEGER NOT NULL,height INTEGER NOT NULL,created_at TEXT,source TEXT); CREATE INDEX captures_source_id ON captures(source,id DESC); CREATE TABLE uploads(id INTEGER PRIMARY KEY DESC,filename TEXT,origin TEXT,url TEXT,created_at TEXT);")?;
        connection.pragma_update(None, "user_version", version)?;
        connection.execute(
            "INSERT INTO captures VALUES(1,?1,2,3,'2024-03-01','capture')",
            [fixture.root.join("legacy.png").to_str().unwrap()],
        )?;
        connection.execute("INSERT INTO uploads VALUES(1,'legacy.png','https://example.invalid','https://example.invalid/1','2024-03-01')", [])?;
        for malformed in [
            Value::Blob(vec![0; 8 * 1024 * 1024]),
            Value::Text("x".repeat(8 * 1024 * 1024)),
            Value::Real(1.5),
            Value::Integer(-1),
            Value::Integer(0),
        ] {
            connection.execute("UPDATE captures SET id=?1", [&malformed])?;
            connection.execute("UPDATE uploads SET id=?1", [&malformed])?;
            let before = std::fs::read(&database)?;
            for sort in [HistorySort::Newest, HistorySort::Oldest] {
                let query = HistoryQuery {
                    sort,
                    ..Default::default()
                };
                for cursor in [
                    None,
                    Some(HistoryDateCursor {
                        created_at: "2024-03-01".into(),
                        id: 1,
                    }),
                ] {
                    assert!(
                        query_capture_history_by_date(&database, &query, cursor.as_ref(), active())
                            .is_err()
                    );
                    if version >= 4 {
                        assert!(
                            query_upload_history_by_date(
                                &database,
                                &query,
                                cursor.as_ref(),
                                active()
                            )
                            .is_err()
                        );
                    }
                }
            }
            assert_eq!(std::fs::read(&database)?, before);
        }
        connection.execute("UPDATE captures SET id=1,width=zeroblob(8388608)", [])?;
        assert!(
            query_capture_history_by_date(&database, &HistoryQuery::default(), None, active())
                .is_err()
        );
        connection.execute("UPDATE captures SET width='not-a-number',height=1.5", [])?;
        assert!(
            query_capture_history_by_date(&database, &HistoryQuery::default(), None, active())
                .is_err()
        );
        connection.execute("UPDATE captures SET width=-1,height=4294967296", [])?;
        assert!(
            query_capture_history_by_date(&database, &HistoryQuery::default(), None, active())
                .is_err()
        );
    }
    let database = fixture.root.join("duplicate-ids.sqlite3");
    let connection = Connection::open(&database)?;
    connection.execute_batch("CREATE TABLE captures(id INTEGER,file_path TEXT,width INTEGER,height INTEGER,created_at TEXT,source TEXT); PRAGMA user_version=1;")?;
    // The existing subset-column admission does not prove uniqueness. Date paging must.
    let before = std::fs::read(&database)?;
    assert!(
        query_capture_history_by_date(&database, &HistoryQuery::default(), None, active()).is_err()
    );
    assert_eq!(std::fs::read(&database)?, before);
    Ok(())
}

#[test]
fn genuine_legacy_numeric_dimensions_fail_without_sorting_foreign_payloads() -> Result<()> {
    use rusqlite::types::Value;
    let fixture = Fixture::new(0)?;
    for version in [1, 4, 10] {
        let database = fixture
            .root
            .join(format!("rowid-numeric-{version}.sqlite3"));
        let connection = Connection::open(&database)?;
        connection.execute_batch("CREATE TABLE captures(id INTEGER PRIMARY KEY,file_path TEXT,width INTEGER NOT NULL,height INTEGER NOT NULL,created_at TEXT,source TEXT); CREATE INDEX captures_source_id ON captures(source,id DESC); CREATE TABLE uploads(id INTEGER PRIMARY KEY,filename TEXT,origin TEXT,url TEXT,created_at TEXT);")?;
        connection.pragma_update(None, "user_version", version)?;
        connection.execute(
            "INSERT INTO captures VALUES(1,?1,2,3,'2024-03-01','capture')",
            [fixture.root.join("legacy.png").to_str().unwrap()],
        )?;
        validate_date_primary_key(&connection, Kind::Captures)?;
        for column in ["width", "height"] {
            for malformed in [
                Value::Blob(vec![0; 8 * 1024 * 1024]),
                Value::Text("x".repeat(8192)),
                Value::Real(1.5),
                Value::Integer(-1),
                Value::Integer(0),
                Value::Integer(4294967296),
            ] {
                connection.execute(&format!("UPDATE captures SET {column}=?1"), [&malformed])?;
                let before = std::fs::read(&database)?;
                for sort in [HistorySort::Oldest, HistorySort::Newest] {
                    assert!(
                        query_capture_history_by_date(
                            &database,
                            &HistoryQuery {
                                sort,
                                ..Default::default()
                            },
                            None,
                            active()
                        )
                        .is_err()
                    );
                }
                assert_eq!(std::fs::read(&database)?, before);
            }
            connection.execute(&format!("UPDATE captures SET {column}=2"), [])?;
        }
        for malformed_id in [0, -1] {
            connection.execute("UPDATE captures SET id=?1", [malformed_id])?;
            assert!(
                query_capture_history_by_date(&database, &HistoryQuery::default(), None, active())
                    .is_err()
            );
        }
    }
    Ok(())
}

#[test]
#[ignore = "optimized bundled-SQLite allocator probe; run alone with --test-threads=1"]
fn history_date_sort_numeric_payload_cost() -> Result<()> {
    fn memory(reset: bool) -> Result<(i64, i64)> {
        let mut current = 0_i64;
        let mut peak = 0_i64;
        // SQLite writes two live i64 outputs; the allocator counters are process-global.
        let status = unsafe {
            rusqlite::ffi::sqlite3_status64(
                rusqlite::ffi::SQLITE_STATUS_MEMORY_USED,
                &mut current,
                &mut peak,
                i32::from(reset),
            )
        };
        ensure!(
            status == rusqlite::ffi::SQLITE_OK,
            "SQLite memory counters unavailable"
        );
        Ok((current, peak))
    }
    let fixture = Fixture::new(10_000)?;
    {
        let connection = crate::history::open_existing(&fixture.database)?;
        connection.execute("UPDATE captures SET width=zeroblob(8388608),height=zeroblob(8388608),created_at='9999-12-31' WHERE id=10000", [])?;
    }
    let mut peaks = Vec::new();
    for guarded in [false, true] {
        let cancel = active();
        let deadline = Instant::now() + QUERY_DEADLINE;
        let connection = open_query(&fixture.database, &cancel, deadline, Kind::Captures, false)?;
        validate_date_primary_key(&connection, Kind::Captures)?;
        register_timestamp_key(&connection, cancel, deadline)?;
        let (mut sql, values) =
            query_sql_ordered(&HistoryQuery::default(), Kind::Captures, None, true, None);
        if !guarded {
            sql = sql
                .replace(&bounded_integer("c.width", i64::from(u32::MAX)), "c.width")
                .replace(
                    &bounded_integer("c.height", i64::from(u32::MAX)),
                    "c.height",
                );
        }
        let (baseline, _) = memory(true)?;
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            if count == 0 {
                for column in [3, 4] {
                    match (guarded, row.get_ref(column)?) {
                        (true, ValueRef::Null) => {}
                        (false, ValueRef::Blob(bytes)) => assert_eq!(bytes.len(), 8388608),
                        _ => panic!("Unexpected numeric sort payload"),
                    }
                }
            }
            count += 1;
        }
        assert_eq!(count, 21);
        let (_, peak) = memory(false)?;
        peaks.push(peak - baseline);
        eprintln!(
            "10k numeric guard={guarded} SQLite query peak delta={} bytes (excludes process/Rust/OS allocations)",
            peak - baseline
        );
    }
    assert!(
        peaks[0] >= 16 * 1024 * 1024,
        "raw reproduction must exercise the oversized sorter payload"
    );
    assert!(
        peaks[1] < 4 * 1024 * 1024,
        "guarded SQLite query allocator growth exceeded 4 MiB probe allowance"
    );
    Ok(())
}
