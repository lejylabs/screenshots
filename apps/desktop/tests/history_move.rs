use std::{
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-move-cli-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        c.arg("history-manage")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            c.env(name, self.path(name));
        }
        c
    }
    fn no_state(&self) {
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.path(name).exists());
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
    serde_json::from_slice(&output.stdout).unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn executable_move_preserves_pixels_identity_and_updates_history() {
    let f = Fixture::new();
    let source = f.path("original.png");
    let dest = f.path("renamed.png");
    let db = f.path("history.sqlite3");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([7, 19, 31, 127]))
        .save(&source)
        .unwrap();
    let original = std::fs::read(&source).unwrap();
    success(
        f.command()
            .arg("import")
            .arg(&db)
            .arg(&source)
            .output()
            .unwrap(),
    );
    let value = success(
        f.command()
            .arg("move")
            .arg(&db)
            .arg("1")
            .arg(&dest)
            .output()
            .unwrap(),
    );
    assert_eq!(value["status"], "moved");
    assert_eq!(value["capture_id"], 1);
    assert!(!source.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), original);
    let entry = app_core::lookup_history_entry(&db, 1).unwrap().unwrap();
    assert_eq!(entry.path, dest);
    assert!(
        success(f.command().arg("pending-move").arg(&db).output().unwrap())["pending"].is_null()
    );
    assert_eq!(
        app_core::check_history_database(&db)
            .unwrap()
            .schema_version,
        5
    );
    f.no_state();
}

#[test]
fn invalid_move_commands_do_not_create_databases_or_files() {
    let f = Fixture::new();
    let db = f.path("missing.sqlite3");
    let dest = f.path("out.png");
    for action in ["move", "pending-move", "recover-move"] {
        let out = f.command().arg(action).arg(&db).output().unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
    assert!(!db.exists());
    assert!(!dest.exists());
    f.no_state();
}

#[cfg(target_os = "linux")]
#[test]
fn collision_never_moves_source_or_publishes_pending_intent() {
    let f = Fixture::new();
    let db = f.path("history.sqlite3");
    let source = f.path("source.png");
    let dest = f.path("keep.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
        .save(&source)
        .unwrap();
    std::fs::write(&dest, b"keep").unwrap();
    success(
        f.command()
            .arg("import")
            .arg(&db)
            .arg(&source)
            .output()
            .unwrap(),
    );
    let bytes = std::fs::read(&source).unwrap();
    let result = f
        .command()
        .arg("move")
        .arg(&db)
        .arg("1")
        .arg(&dest)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert_eq!(std::fs::read(dest).unwrap(), b"keep");
    assert!(app_core::pending_history_move(&db).unwrap().is_none());
    f.no_state();
}
