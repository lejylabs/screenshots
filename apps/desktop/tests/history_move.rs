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
        Self(root.canonicalize().unwrap())
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
        10
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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

fn capture_fixture(path: PathBuf) -> app_core::CapturedImage {
    app_core::CapturedImage {
        desktop_layout: None,
        display_id: "private-save-fixture".into(),
        window_id: None,
        width: 3,
        height: 2,
        pixels: std::sync::Arc::new(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([7, 19, 31, 127]),
        )),
        png_path: path,
        elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}

#[test]
fn executable_save_recovery_preserves_published_pixels_until_history_can_commit() {
    let f = Fixture::new();
    let database = f.path("save.sqlite3");
    let target = f.path("saved.png");
    assert!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .is_empty()
    );
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("CREATE TRIGGER owned_save_failure BEFORE INSERT ON captures BEGIN SELECT RAISE(FAIL,'owned fixture'); END;").unwrap();
    let capture = capture_fixture(target.clone());
    let report =
        app_core::save_capture_with_history(&database, &capture, &target, || false).unwrap();
    assert_eq!(report.status, "saved_unindexed");
    assert!(report.pending);
    let published = std::fs::read(&target).unwrap();
    let pending = success(
        f.command()
            .arg("pending-save")
            .arg(&database)
            .output()
            .unwrap(),
    );
    assert_eq!(pending["pending"]["operation_id"], report.operation_id);
    let canceled = success(
        f.command()
            .arg("recover-save")
            .arg(&database)
            .arg(&report.operation_id)
            .arg("cancel")
            .output()
            .unwrap(),
    );
    assert_eq!(canceled["status"], "saved_unindexed");
    assert_eq!(std::fs::read(&target).unwrap(), published);
    connection
        .execute_batch("DROP TRIGGER owned_save_failure")
        .unwrap();
    let finished = success(
        f.command()
            .arg("recover-save")
            .arg(&database)
            .arg(&report.operation_id)
            .arg("finish")
            .output()
            .unwrap(),
    );
    assert_eq!(finished["status"], "indexed");
    assert!(!finished["pending"].as_bool().unwrap());
    assert_eq!(std::fs::read(&target).unwrap(), published);
    let decoded = image::open(&target).unwrap().into_rgba8();
    assert_eq!(decoded, *capture.pixels);
    assert_eq!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .len(),
        1
    );
    assert!(
        success(
            f.command()
                .arg("pending-save")
                .arg(&database)
                .output()
                .unwrap()
        )["pending"]
            .is_null()
    );
    f.no_state();
}

#[test]
fn executable_save_recovery_refuses_a_replaced_stage_then_finishes_the_original() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let database = f.path("stage.sqlite3");
    let target = f.path("recovered.png");
    let kept = f.path("held-original.part");
    assert!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .is_empty()
    );
    let replaced = AtomicBool::new(false);
    let capture = capture_fixture(target.clone());
    let result = app_core::save_capture_with_history(&database, &capture, &target, || {
        if !replaced.load(Ordering::Relaxed)
            && let Some(pending) = app_core::pending_capture_save(&database).unwrap()
        {
            replaced.store(true, Ordering::Relaxed);
            std::fs::rename(&pending.staging_path, &kept).unwrap();
            std::fs::write(&pending.staging_path, b"foreign fixture bytes").unwrap();
        }
        false
    });
    assert!(result.is_err() && replaced.load(Ordering::Relaxed));
    assert!(!target.exists());
    let pending = app_core::pending_capture_save(&database).unwrap().unwrap();
    assert!(pending.ready && !pending.canceling);
    for action in ["finish", "cancel"] {
        let failure = f
            .command()
            .arg("recover-save")
            .arg(&database)
            .arg(&pending.operation_id)
            .arg(action)
            .output()
            .unwrap();
        assert!(!failure.status.success() && failure.stdout.is_empty());
        assert_eq!(
            std::fs::read(&pending.staging_path).unwrap(),
            b"foreign fixture bytes"
        );
        assert!(app_core::pending_capture_save(&database).unwrap().is_some());
        assert!(!target.exists());
    }
    let foreign = f.path("foreign-preserved.part");
    std::fs::rename(&pending.staging_path, &foreign).unwrap();
    std::fs::rename(&kept, &pending.staging_path).unwrap();
    let finished = success(
        f.command()
            .arg("recover-save")
            .arg(&database)
            .arg(&pending.operation_id)
            .arg("finish")
            .output()
            .unwrap(),
    );
    assert_eq!(finished["status"], "indexed");
    assert_eq!(image::open(&target).unwrap().into_rgba8(), *capture.pixels);
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign fixture bytes");
    assert!(app_core::pending_capture_save(&database).unwrap().is_none());
    f.no_state();
}
