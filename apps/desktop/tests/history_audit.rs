use image::{Rgba, RgbaImage};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-audit-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn png(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        RgbaImage::from_pixel(2, 3, Rgba([12, 34, 56, 78]))
            .save(&path)
            .unwrap();
        path
    }

    fn run(&self, args: &[OsString]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("history-audit").args(args).current_dir(&self.0);
        for variable in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            command.env(variable, self.path(variable));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap()
    }

    fn audit(&self, database: &Path) -> serde_json::Value {
        success(self.run(&[database.as_os_str().to_owned()]))
    }

    fn no_user_state(&self) {
        for variable in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            assert!(!self.path(variable).exists());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

fn status<'a>(result: &'a serde_json::Value, path: &Path) -> &'a str {
    result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == path.to_string_lossy().as_ref())
        .unwrap()["status"]
        .as_str()
        .unwrap()
}

#[test]
fn executable_reports_missing_present_and_non_files_without_decoding_or_mutation() {
    let fixture = Fixture::new();
    let missing = fixture.png("removed.png");
    let existing = fixture.png("Größe $(ignored).png");
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, &[missing.clone(), existing.clone()]).unwrap();
    let before = std::fs::read(&database).unwrap();
    std::fs::remove_file(&missing).unwrap();
    let original = std::fs::read(&existing).unwrap();
    let result = fixture.audit(&database);
    assert_eq!(result["entries"].as_array().unwrap().len(), 2);
    assert_eq!(result["path_encoding"], "utf8-lossy");
    assert_eq!(result["has_more"], false);
    assert_eq!(result["next_cursor"], serde_json::Value::Null);
    assert_eq!(status(&result, &missing), "missing");
    assert_eq!(status(&result, &existing), "present");
    assert_eq!(std::fs::read(&existing).unwrap(), original);

    // An audit checks reference availability, not the PNG header or pixel contents.
    std::fs::write(&existing, b"not an image").unwrap();
    assert_eq!(status(&fixture.audit(&database), &existing), "present");
    assert_eq!(std::fs::read(&existing).unwrap(), b"not an image");
    std::fs::remove_file(&existing).unwrap();
    std::fs::create_dir(&existing).unwrap();
    assert_eq!(status(&fixture.audit(&database), &existing), "not_file");
    assert!(existing.is_dir());
    assert!(!missing.exists());
    assert_eq!(std::fs::read(&database).unwrap(), before);
    fixture.no_user_state();
}

#[test]
fn executable_pages_read_only_database_with_exclusive_newest_first_cursors() {
    let fixture = Fixture::new();
    let database = fixture.path("history.sqlite3");
    let paths: Vec<_> = (0..25)
        .map(|index| fixture.png(&format!("image-{index}.png")))
        .collect();
    app_core::import_capture_files(&database, &paths).unwrap();
    let before = std::fs::read(&database).unwrap();
    let permissions = std::fs::metadata(&database).unwrap().permissions();
    struct RestorePermissions(PathBuf, std::fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, self.1.clone());
        }
    }
    let _restore = RestorePermissions(database.clone(), permissions.clone());
    let mut readonly = permissions;
    readonly.set_readonly(true);
    std::fs::set_permissions(&database, readonly).unwrap();
    let first = fixture.audit(&database);
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 20);
    assert_eq!(entries[0]["id"], 25);
    assert_eq!(entries[19]["id"], 6);
    assert!(entries.iter().all(|entry| entry["status"] == "present"));
    assert_eq!(first["has_more"], true);
    assert_eq!(first["next_cursor"], 6);
    let second = success(fixture.run(&[
        database.as_os_str().to_owned(),
        "--cursor".into(),
        first["next_cursor"].as_i64().unwrap().to_string().into(),
    ]));
    let entries = second["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0]["id"], 5);
    assert_eq!(entries[4]["id"], 1);
    assert_eq!(second["has_more"], false);
    assert_eq!(second["next_cursor"], serde_json::Value::Null);
    let empty = success(fixture.run(&[
        database.as_os_str().to_owned(),
        "--cursor".into(),
        "1".into(),
    ]));
    assert!(empty["entries"].as_array().unwrap().is_empty());
    assert_eq!(empty["has_more"], false);
    assert_eq!(empty["next_cursor"], serde_json::Value::Null);
    let beyond = success(fixture.run(&[
        database.as_os_str().to_owned(),
        "--cursor".into(),
        i64::MAX.to_string().into(),
    ]));
    assert_eq!(beyond, first);
    assert_eq!(std::fs::read(&database).unwrap(), before);
    assert!(
        std::fs::metadata(&database)
            .unwrap()
            .permissions()
            .readonly()
    );
    assert!(paths.iter().all(|path| path.is_file()));
    fixture.no_user_state();
}

#[test]
fn executable_rejects_malformed_database_without_changing_it() {
    let fixture = Fixture::new();
    let database = fixture.path("invalid.sqlite3");
    std::fs::write(&database, b"not a database").unwrap();
    failure(fixture.run(&[database.as_os_str().to_owned()]));
    assert_eq!(std::fs::read(database).unwrap(), b"not a database");
    fixture.no_user_state();
}

#[test]
fn executable_rejects_invalid_arguments_without_creating_storage() {
    let fixture = Fixture::new();
    let database = fixture.path("missing/history.sqlite3");
    let db = database.as_os_str().to_owned();
    failure(fixture.run(&[]));
    failure(fixture.run(std::slice::from_ref(&db)));
    for cursor in [
        "",
        "0",
        "-1",
        "+1",
        "1.5",
        " 1",
        "1 ",
        "a",
        "9223372036854775808",
    ] {
        failure(fixture.run(&[db.clone(), "--cursor".into(), cursor.into()]));
    }
    for args in [
        vec![db.clone(), "--cursor".into()],
        vec![db.clone(), "--unknown".into(), "1".into()],
        vec![db.clone(), "--cursor=1".into()],
        vec![db.clone(), "--cursor".into(), "1".into(), "extra".into()],
        vec![
            db,
            "--cursor".into(),
            "1".into(),
            "--cursor".into(),
            "2".into(),
        ],
        vec!["".into()],
        vec!["x".repeat(4097).into()],
    ] {
        failure(fixture.run(&args));
    }
    assert!(!database.exists());
    assert!(!fixture.path("missing").exists());
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn executable_follows_file_symlinks_and_reports_unreadable_loops() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let source = fixture.png("source.png");
    let target = fixture.png("target.png");
    let original = std::fs::read(&target).unwrap();
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, std::slice::from_ref(&source)).unwrap();
    let before = std::fs::read(&database).unwrap();
    std::fs::remove_file(&source).unwrap();
    symlink(&target, &source).unwrap();
    assert_eq!(status(&fixture.audit(&database), &source), "present");
    std::fs::remove_file(&source).unwrap();
    symlink(fixture.path("absent.png"), &source).unwrap();
    assert_eq!(status(&fixture.audit(&database), &source), "missing");
    std::fs::remove_file(&source).unwrap();
    symlink(&fixture.0, &source).unwrap();
    assert_eq!(status(&fixture.audit(&database), &source), "not_file");
    std::fs::remove_file(&source).unwrap();
    symlink(&source, &source).unwrap();
    assert_eq!(status(&fixture.audit(&database), &source), "unreadable");
    assert_eq!(std::fs::read_link(&source).unwrap(), source);
    assert_eq!(std::fs::read(&target).unwrap(), original);
    assert_eq!(std::fs::read(&database).unwrap(), before);
    fixture.no_user_state();
}
