use std::{path::PathBuf, process::Command};

#[test]
fn executable_history_queries_are_bounded_filtered_and_read_only() {
    let root = std::env::temp_dir().join(format!(
        "screenshots-history-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let db = root.join("history.sqlite3");
    for i in 0..25 {
        app_core::record_uploaded_png(
            &db,
            std::path::Path::new(&format!("fixture-{i}.png")),
            "https://example.invalid",
            &format!("https://example.invalid/share/{i}"),
        )
        .unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["history-query", "uploads"])
            .arg(&db)
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
    let first = run(&["--order", "oldest"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 20);
    assert_eq!(first["entries"][0]["id"], 1);
    assert_eq!(first["next_cursor"], 20);
    let second = run(&["--order", "oldest", "--cursor", "20"]);
    assert!(second.status.success());
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second["entries"].as_array().unwrap().len(), 5);
    assert_eq!(second["has_more"], false);
    assert_eq!(second["next_cursor"], serde_json::Value::Null);
    let search = run(&["--search", "/share/24"]);
    assert!(search.status.success());
    let search: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(search["entries"].as_array().unwrap().len(), 1);
    for args in [
        vec!["--order", "oops"],
        vec!["--source", "imports"],
        vec!["--from", "2025-02-30"],
        vec!["--cursor", "0"],
        vec!["--search", "a", "--search", "b"],
    ] {
        let result = run(&args);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
    assert_eq!(std::fs::read(db).unwrap(), before);
    for entry in std::fs::read_dir(root).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(matches!(
            name.to_str(),
            Some("history.sqlite3" | "history.sqlite3-wal" | "history.sqlite3-shm")
        ));
    }
}
