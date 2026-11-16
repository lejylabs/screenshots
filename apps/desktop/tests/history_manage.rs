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
            "screenshots-history-manage-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn png(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.path(name);
        RgbaImage::from_pixel(width, height, Rgba([12, 34, 56, 78]))
            .save(&path)
            .unwrap();
        path
    }
    fn run(&self, action: &str, database: &Path, values: &[OsString]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .args(["history-manage", action])
            .arg(database)
            .args(values)
            .current_dir(&self.0)
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("userprofile"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap()
    }
    fn no_user_state(&self) {
        for name in [
            "home",
            "userprofile",
            "config",
            "data",
            "cache",
            "appdata",
            "localappdata",
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
fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn export_policy_strips_metadata_without_touching_library_or_sources() {
    use image::ImageEncoder;
    let fixture = Fixture::new();
    let source = fixture.path("metadata.png");
    let pixels = RgbaImage::from_pixel(4, 3, Rgba([12, 34, 56, 78]));
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut encoded);
    encoder.set_exif_metadata(vec![b'M'; 64]).unwrap();
    encoder
        .write_image(pixels.as_raw(), 4, 3, image::ExtendedColorType::Rgba8)
        .unwrap();
    std::fs::write(&source, &encoded).unwrap();
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, std::slice::from_ref(&source)).unwrap();
    let entry = app_core::capture_history_page(&database, None)
        .unwrap()
        .remove(0);
    let before = std::fs::read(&database).unwrap();
    let directory = fixture.path("out");
    std::fs::create_dir(&directory).unwrap();
    let result = success(fixture.run(
        "export",
        &database,
        &[
            directory.clone().into(),
            "strip-metadata".into(),
            entry.id.to_string().into(),
        ],
    ));
    assert_eq!(result["exported"], 1);
    assert_eq!(result["failed"], 0);
    let bytes = std::fs::read(directory.join("metadata.png")).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"eXIf"));
    assert_eq!(
        image::load_from_memory(&bytes).unwrap().into_rgba8(),
        pixels
    );
    assert_eq!(std::fs::read(&source).unwrap(), encoded);
    assert_eq!(std::fs::read(&database).unwrap(), before);
    let duplicate = success(fixture.run(
        "export",
        &database,
        &[
            directory.into(),
            "original".into(),
            entry.id.to_string().into(),
        ],
    ));
    assert_eq!(duplicate["exported"], 0);
    assert_eq!(duplicate["failed"], 1);
    fixture.no_user_state();
}

#[test]
fn import_reports_added_existing_and_individual_failures_without_changing_files() {
    let fixture = Fixture::new();
    let image = fixture.png("Größe $(ignored).png", 3, 2);
    let original = std::fs::read(&image).unwrap();
    let invalid = fixture.path("invalid.png");
    std::fs::write(&invalid, b"not PNG").unwrap();
    let database = fixture.path("new/history.sqlite3");
    let result = success(fixture.run(
        "import",
        &database,
        &[
            OsString::from("Größe $(ignored).png"),
            invalid.clone().into(),
            "missing.png".into(),
        ],
    ));
    assert_eq!(result["added"], 1);
    assert_eq!(result["existing"], 0);
    assert_eq!(result["failed"], 2);
    assert_eq!(result["failures"].as_array().unwrap().len(), 2);
    let entries = app_core::capture_history_page(&database, None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].path.canonicalize().unwrap(),
        image.canonicalize().unwrap()
    );
    let again = success(fixture.run("import", &database, &[image.clone().into()]));
    assert_eq!(again["added"], 0);
    assert_eq!(again["existing"], 1);
    assert_eq!(again["failed"], 0);
    let all_failed = success(fixture.run("import", &database, &[invalid.clone().into()]));
    assert_eq!(all_failed["added"], 0);
    assert_eq!(all_failed["failed"], 1);
    assert_eq!(std::fs::read(image).unwrap(), original);
    assert_eq!(std::fs::read(invalid).unwrap(), b"not PNG");
    fixture.no_user_state();
}

#[test]
fn relink_retains_id_timestamp_and_both_files_then_remove_only_changes_metadata() {
    let fixture = Fixture::new();
    let old = fixture.png("old.png", 2, 3);
    let new = fixture.png("new.png", 4, 5);
    let old_bytes = std::fs::read(&old).unwrap();
    let new_bytes = std::fs::read(&new).unwrap();
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, std::slice::from_ref(&old)).unwrap();
    let original = app_core::capture_history_page(&database, None)
        .unwrap()
        .remove(0);
    let result = success(fixture.run(
        "relink",
        &database,
        &[original.id.to_string().into(), new.clone().into()],
    ));
    assert_eq!(result["id"], original.id);
    assert_eq!(result["width"], 4);
    assert_eq!(result["height"], 5);
    let updated = app_core::lookup_history_entry(&database, original.id)
        .unwrap()
        .unwrap();
    assert_eq!(updated.path, new);
    assert_eq!(updated.created_at, original.created_at);
    let result = success(fixture.run(
        "remove",
        &database,
        &[
            original.id.to_string().into(),
            original.id.to_string().into(),
            "9999".into(),
        ],
    ));
    assert_eq!(result["removed"], 1);
    assert!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(std::fs::read(old).unwrap(), old_bytes);
    assert_eq!(std::fs::read(new).unwrap(), new_bytes);
    fixture.no_user_state();
}

#[test]
fn relink_conflicts_missing_ids_and_bad_images_preserve_existing_metadata() {
    let fixture = Fixture::new();
    let one = fixture.png("one.png", 2, 3);
    let two = fixture.png("two.png", 4, 5);
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, &[one.clone(), two.clone()]).unwrap();
    let entries = app_core::capture_history_page(&database, None).unwrap();
    let canonical_one = std::fs::canonicalize(&one).unwrap();
    let first = entries
        .iter()
        .find(|entry| entry.path == canonical_one)
        .unwrap();
    for values in [
        vec![first.id.to_string().into(), two.into()],
        vec!["9999999".into(), one.clone().into()],
        vec![
            first.id.to_string().into(),
            fixture.path("missing.png").into(),
        ],
    ] {
        failure(fixture.run("relink", &database, &values));
    }
    let unchanged = app_core::lookup_history_entry(&database, first.id)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.path, one);
    assert_eq!(unchanged.created_at, first.created_at);
    assert_eq!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .len(),
        2
    );
    fixture.no_user_state();
}

#[test]
fn invalid_ids_counts_and_missing_mutation_database_never_create_storage() {
    let fixture = Fixture::new();
    let database = fixture.path("missing/history.sqlite3");
    for id in ["0", "-1", "+1", "1.5", "9223372036854775808", "no"] {
        failure(fixture.run("remove", &database, &[id.into()]));
        failure(fixture.run("relink", &database, &[id.into(), "image.png".into()]));
    }
    failure(fixture.run("remove", &database, &[]));
    failure(fixture.run("remove", &database, &vec![OsString::from("1"); 101]));
    failure(fixture.run("import", &database, &[]));
    failure(fixture.run("import", &database, &vec![OsString::from("image.png"); 101]));
    failure(fixture.run("remove", &database, &["1".into()]));
    failure(fixture.run("relink", &database, &["1".into(), "image.png".into()]));
    assert!(!database.exists());
    assert!(!fixture.path("missing").exists());
    fixture.no_user_state();
}

#[test]
fn remove_twenty_rows_is_bounded_and_keeps_every_image() {
    let fixture = Fixture::new();
    let database = fixture.path("history.sqlite3");
    let files: Vec<_> = (0..21)
        .map(|index| fixture.png(&format!("{index}.png"), 1, 1))
        .collect();
    app_core::import_capture_files(&database, &files).unwrap();
    let entries = app_core::capture_history_page(&database, None).unwrap();
    let ids: Vec<_> = entries[..20]
        .iter()
        .map(|entry| entry.id.to_string().into())
        .collect();
    assert_eq!(
        success(fixture.run("remove", &database, &ids))["removed"],
        20
    );
    assert_eq!(
        app_core::capture_history_page(&database, None)
            .unwrap()
            .len(),
        1
    );
    assert!(files.iter().all(|file| file.exists()));
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn mutation_database_symlinks_are_rejected_without_changing_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let image = fixture.png("one.png", 1, 1);
    let database = fixture.path("history.sqlite3");
    app_core::import_capture_files(&database, std::slice::from_ref(&image)).unwrap();
    let entry = app_core::capture_history_page(&database, None)
        .unwrap()
        .remove(0);
    let link = fixture.path("linked.sqlite3");
    symlink(&database, &link).unwrap();
    failure(fixture.run("remove", &link, &[entry.id.to_string().into()]));
    failure(fixture.run(
        "relink",
        &link,
        &[entry.id.to_string().into(), image.into()],
    ));
    assert!(
        app_core::lookup_history_entry(&database, entry.id)
            .unwrap()
            .is_some()
    );
    fixture.no_user_state();
}
