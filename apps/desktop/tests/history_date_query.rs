use rusqlite::params;
use std::{path::PathBuf, process::Command};

fn fixture_connection(database: &std::path::Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(database).unwrap();
    // Seed metadata without private filesystem-reservation callbacks; retain the real schema.
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            false,
        )
        .unwrap();
    connection
}

#[test]
fn executable_date_history_queries_use_exact_timestamp_tokens_and_private_metadata_only() {
    let root = std::env::temp_dir().join(format!("screenshots-date-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let database = root.join("history.sqlite3");
    app_core::capture_history_page(&database, None).unwrap();
    let mut connection = fixture_connection(&database);
    let transaction = connection.transaction().unwrap();
    let timestamps = [
        "2024-03-01",
        "2024-02-29T23:59:59.1000000002Z",
        "2024-03-01T00:00:00+00:00",
        "2024-02-29T23:59:59.1000000001Z",
        "2024-02-29T23:59:59.1Z",
        "2024-03-01t00:00:00z",
        "2024-02-29T23:59:59.1000000000-00:00",
        "2024-03-01T00:00:00.000Z",
    ];
    for id in 1..=10_000_i64 {
        let timestamp = timestamps[(id % 8) as usize];
        transaction.execute("INSERT INTO captures(id,file_path,mime_type,width,height,file_size,created_at,source) VALUES(?1,?2,'image/png',2,3,42,?3,'capture')", params![id, root.join(format!("image-{id}.png")).to_str().unwrap(), timestamp]).unwrap();
        transaction.execute("INSERT INTO uploads(id,filename,origin,url,created_at) VALUES(?1,?2,'https://example.invalid',?3,?4)", params![id,format!("image-{id}.png"),format!("https://example.invalid/share/{id}"),timestamp]).unwrap();
    }
    transaction.commit().unwrap();
    connection
        .execute_batch("INSERT INTO captures_search(captures_search) VALUES('rebuild');")
        .unwrap();
    drop(connection);
    let run = |kind: &str, args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["history-query", kind])
            .arg(&database)
            .args(args);
        for variable in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            command.env(variable, root.join(variable));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap()
    };
    let page = |kind: &str, args: &[&str]| {
        let output = run(kind, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let mut expected = (1..=10_000_i64).collect::<Vec<_>>();
    // Independent chronological ranks, including fractional values past nanoseconds.
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
    let ids = |value: &serde_json::Value| {
        value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_i64().unwrap())
            .collect::<Vec<_>>()
    };
    let before = std::fs::read(&database).unwrap();
    for kind in ["captures", "uploads"] {
        let first = page(kind, &["--order", "date-oldest"]);
        assert_eq!(ids(&first), expected[..20]);
        assert_eq!(first["has_more"], true);
        let token = first["next_cursor"].as_str().unwrap();
        assert!(token.ends_with("/78"));
        let second = page(kind, &["--cursor", token, "--order", "date-oldest"]);
        assert_eq!(ids(&second), expected[20..40]);
        let newest = page(kind, &["--order", "date-newest"]);
        assert_eq!(
            ids(&newest),
            expected.iter().rev().take(20).copied().collect::<Vec<_>>()
        );
        let id_order = page(kind, &["--order", "oldest"]);
        assert_eq!(ids(&id_order), (1..=20).collect::<Vec<_>>());
        assert_eq!(id_order["next_cursor"], 20);
        let filtered = page(
            kind,
            &[
                "--order",
                "date-oldest",
                "--from",
                "2024-02-29",
                "--through",
                "2024-02-29",
                "--search",
                "image-3",
            ],
        );
        // Capture FTS uses word-prefix tokens; upload substring can return image-3xx.
        assert!(filtered["entries"].as_array().unwrap().iter().all(|entry| {
            entry["created_at"]
                .as_str()
                .unwrap()
                .starts_with("2024-02-29")
        }));
        for args in [
            vec!["--order", "date-oldest", "--cursor", "20"],
            vec!["--order", "oldest", "--cursor", token],
            vec!["--order", "date-oldest", "--cursor", "2024-02-30/1"],
            vec!["--order", "date-oldest", "--cursor", "2024-03-01/0"],
            vec![
                "--order",
                "date-oldest",
                "--cursor",
                "2024-03-01T00:00:00+01:00/1",
            ],
            vec!["--order", "date-oldest", "--order", "date-newest"],
        ] {
            let output = run(kind, &args);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
    }
    assert_eq!(std::fs::read(&database).unwrap(), before);
    for kind in ["captures", "uploads"] {
        let first = page(kind, &["--order", "date-oldest"]);
        let token = first["next_cursor"].as_str().unwrap();
        let connection = fixture_connection(&database);
        // Test the actual executable with a deleted anchor and its original timestamp token.
        connection
            .execute(&format!("DELETE FROM {kind} WHERE id=78"), [])
            .unwrap();
        drop(connection);
        let before = std::fs::read(&database).unwrap();
        let second = page(kind, &["--order", "date-oldest", "--cursor", token]);
        assert_eq!(ids(&second), expected[20..40]);
        assert_eq!(std::fs::read(&database).unwrap(), before);
    }
    let connection = fixture_connection(&database);
    connection
        .execute(
            "UPDATE captures SET width=zeroblob(8388608),created_at='9999-12-31' WHERE id=10000",
            [],
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&database).unwrap();
    let malformed = run("captures", &["--order", "date-newest"]);
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
    assert_eq!(std::fs::read(&database).unwrap(), before);
    for entry in std::fs::read_dir(&root).unwrap() {
        assert!(matches!(
            entry.unwrap().file_name().to_str(),
            Some("history.sqlite3" | "history.sqlite3-wal" | "history.sqlite3-shm")
        ));
    }
}
