use image::{ImageEncoder as _, Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-image-info-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn run(&self, command: &str, input: &Path, options: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .args(["image", command])
            .arg(input)
            .args(options)
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
fn info_and_pixel_preserve_alpha_hidden_rgb_and_unicode_paths_without_writes() {
    let fixture = Fixture::new();
    let input = fixture.path("Größe 東京 $(ignored).png");
    RgbaImage::from_fn(3, 2, |x, y| match (x, y) {
        (0, 0) => Rgba([12, 34, 56, 0]),
        (2, 1) => Rgba([170, 187, 204, 221]),
        _ => Rgba([1, 2, 3, 255]),
    })
    .save(&input)
    .unwrap();
    let original = std::fs::read(&input).unwrap();
    let info = success(fixture.run("info", &input, &[]));
    assert_eq!(
        info,
        serde_json::json!({"width":3,"height":2,"decoded_rgba_bytes":24,"source_file_bytes":original.len()})
    );
    let transparent = success(fixture.run("pixel", &input, &["0", "0"]));
    assert_eq!(
        transparent,
        serde_json::json!({"width":3,"height":2,"x":0,"y":0,"rgba":[12,34,56,0],"hex_rgba":"#0C223800"})
    );
    let alpha = success(fixture.run("pixel", &input, &["2", "1"]));
    assert_eq!(alpha["rgba"], serde_json::json!([170, 187, 204, 221]));
    assert_eq!(alpha["hex_rgba"], "#AABBCCDD");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn jpeg_orientation_is_applied_before_info_and_coordinates() {
    let fixture = Fixture::new();
    let input = fixture.path("misleading.data");
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 100);
    encoder
        .set_exif_metadata(vec![
            73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ])
        .unwrap();
    let rgb = image::RgbImage::from_fn(3, 2, |x, y| {
        image::Rgb([(x * 80) as u8, (y * 120) as u8, 40])
    });
    encoder
        .write_image(rgb.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)
        .unwrap();
    std::fs::write(&input, &encoded).unwrap();
    let oriented =
        image::imageops::rotate90(&image::load_from_memory(&encoded).unwrap().into_rgba8());
    let info = success(fixture.run("info", &input, &[]));
    assert_eq!(info["width"], 2);
    assert_eq!(info["height"], 3);
    assert_eq!(info["decoded_rgba_bytes"], 24);
    let pixel = success(fixture.run("pixel", &input, &["1", "2"]));
    assert_eq!(pixel["rgba"], serde_json::json!(oriented.get_pixel(1, 2).0));
    failure(fixture.run("pixel", &input, &["2", "1"]));
    assert_eq!(std::fs::read(input).unwrap(), encoded);
    fixture.no_user_state();
}

#[test]
fn strict_coordinates_and_argument_counts_fail_without_outputs() {
    let fixture = Fixture::new();
    let input = fixture.path("source.webp");
    RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 4]))
        .save(&input)
        .unwrap();
    let original = std::fs::read(&input).unwrap();
    for options in [
        &["2", "0"][..],
        &["0", "1"],
        &["-1", "0"],
        &["1.5", "0"],
        &["+1", "0"],
        &["", "0"],
        &["4294967296", "0"],
        &["0"],
        &["0", "0", "extra"],
    ] {
        failure(fixture.run("pixel", &input, options));
    }
    failure(fixture.run("info", &input, &["extra"]));
    let pixel = success(fixture.run("pixel", &input, &["01", "00"]));
    assert_eq!(pixel["rgba"], serde_json::json!([1, 2, 3, 4]));
    assert_eq!(std::fs::read(input).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn malformed_missing_and_oversized_sources_do_not_create_files() {
    let fixture = Fixture::new();
    let input = fixture.path("bad.png");
    for contents in [b"not an image".as_slice(), b"GIF89a", b"\x89PNG\r\n\x1a\n"] {
        std::fs::write(&input, contents).unwrap();
        failure(fixture.run("info", &input, &[]));
        failure(fixture.run("pixel", &input, &["0", "0"]));
        assert_eq!(std::fs::read(&input).unwrap(), contents);
    }
    let huge = fixture.path("huge.png");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    failure(fixture.run("info", &huge, &[]));
    failure(fixture.run("pixel", &huge, &["0", "0"]));
    let missing = fixture.path("missing.png");
    failure(fixture.run("info", &missing, &[]));
    failure(fixture.run("pixel", &missing, &["0", "0"]));
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    fixture.no_user_state();
}

// APFS rejects invalid UTF-8 names before the application can open them.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_native_input_path_is_supported() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    let fixture = Fixture::new();
    let input = fixture
        .0
        .join(OsString::from_vec(b"image-\xff.png".to_vec()));
    RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 40]))
        .save(&input)
        .unwrap();
    assert_eq!(success(fixture.run("info", &input, &[]))["width"], 1);
    assert_eq!(
        success(fixture.run("pixel", &input, &["0", "0"]))["rgba"],
        serde_json::json!([10, 20, 30, 40])
    );
    fixture.no_user_state();
}
