use image::{Rgba, RgbaImage};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-qr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[&OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("qr")
            .args(args)
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
    fn generate(&self, text: &str, path: &Path) -> Output {
        self.run(&["generate".as_ref(), text.as_ref(), path.as_os_str()])
    }
    fn no_user_state(&self) {
        for name in [
            "config",
            "data",
            "cache",
            "appdata",
            "localappdata",
            "home",
            "userprofile",
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

#[test]
fn executable_qr_round_trip_preserves_unicode_and_never_opens_links() {
    let f = Fixture::new();
    for (index, text) in [
        "Hello QR",
        "Grüße 🦊 https://example.invalid/$(echo nope)",
        &"z".repeat(2048),
    ]
    .into_iter()
    .enumerate()
    {
        let path = f.path(&format!("qr-{index}.png"));
        let metadata = success(f.generate(text, &path));
        let pixels = image::open(&path).unwrap().into_rgba8();
        assert_eq!(metadata["width"], pixels.width());
        assert_eq!(pixels.width(), pixels.height());
        assert!(
            pixels
                .pixels()
                .all(|pixel| matches!(pixel.0, [0, 0, 0, 255] | [255, 255, 255, 255]))
        );
        assert_eq!(pixels.get_pixel(0, 0).0, [255, 255, 255, 255]);
        let original = std::fs::read(&path).unwrap();
        let decoded = success(f.run(&["scan".as_ref(), path.as_os_str()]));
        assert_eq!(decoded["codes"].as_array().unwrap().len(), 1);
        assert_eq!(decoded["codes"][0]["text"], text);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    f.no_user_state();
}

#[test]
fn executable_qr_alpha_is_composited_on_white_and_hidden_codes_are_ignored() {
    let f = Fixture::new();
    let original = f.path("original.png");
    success(f.generate("Visible QR", &original));
    let mut pixels = image::open(&original).unwrap().into_rgba8();
    for pixel in pixels.pixels_mut() {
        if pixel.0[0] == 255 {
            pixel.0 = [3, 8, 12, 0];
        }
    }
    let visible = f.path("transparent-background.webp");
    pixels.save(&visible).unwrap();
    assert_eq!(
        success(f.run(&["scan".as_ref(), visible.as_os_str()]))["codes"][0]["text"],
        "Visible QR"
    );
    for pixel in pixels.pixels_mut() {
        pixel.0[3] = 0;
    }
    let hidden = f.path("hidden.png");
    pixels.save(&hidden).unwrap();
    let result = f.run(&["scan".as_ref(), hidden.as_os_str()]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    f.no_user_state();
}

#[test]
fn executable_qr_rejects_invalid_inputs_and_preserves_existing_outputs() {
    let f = Fixture::new();
    let output = f.path("out.png");
    for text in [String::new(), "x".repeat(2049)] {
        let result = f.generate(&text, &output);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
    }
    assert!(!f.generate("test", &f.path("bad.webp")).status.success());
    assert!(!f.path("bad.webp").exists());
    std::fs::write(&output, b"preserve this file").unwrap();
    assert!(!f.generate("test", &output).status.success());
    assert_eq!(std::fs::read(output).unwrap(), b"preserve this file");
    let blank = f.path("blank.png");
    RgbaImage::from_pixel(80, 80, Rgba([255, 255, 255, 255]))
        .save(&blank)
        .unwrap();
    assert!(
        !f.run(&["scan".as_ref(), blank.as_os_str()])
            .status
            .success()
    );
    let big = f.path("oversized.png");
    RgbaImage::from_pixel(2049, 2048, Rgba([255, 255, 255, 255]))
        .save(&big)
        .unwrap();
    assert!(!f.run(&["scan".as_ref(), big.as_os_str()]).status.success());
    let malformed = f.path("not-image.png");
    std::fs::write(&malformed, b"bad image").unwrap();
    assert!(
        !f.run(&["scan".as_ref(), malformed.as_os_str()])
            .status
            .success()
    );
    assert!(!f.run(&["scan".as_ref()]).status.success());
    f.no_user_state();
}

#[test]
fn executable_qr_candidate_saturation_fails_without_partial_results() {
    let f = Fixture::new();
    let code = f.path("single.png");
    success(f.generate("many codes", &code));
    let source = image::open(&code).unwrap().into_rgba8();
    let mut many = RgbaImage::from_pixel(
        source.width() * 4,
        source.height() * 3,
        Rgba([255, 255, 255, 255]),
    );
    for y in 0..3 {
        for x in 0..4 {
            image::imageops::overlay(
                &mut many,
                &source,
                i64::from(source.width() * x),
                i64::from(source.height() * y),
            );
        }
    }
    let path = f.path("many.png");
    many.save(&path).unwrap();
    let result = f.run(&["scan".as_ref(), path.as_os_str()]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("candidate limit"));
    f.no_user_state();
}

#[test]
fn executable_qr_grid_saturation_never_reports_only_eight_of_nine_codes() {
    let f = Fixture::new();
    let code = f.path("single.png");
    success(f.generate("nine codes", &code));
    let source = image::open(&code).unwrap().into_rgba8();
    let mut many = RgbaImage::from_pixel(
        source.width() * 3,
        source.height() * 3,
        Rgba([255, 255, 255, 255]),
    );
    for y in 0..3 {
        for x in 0..3 {
            image::imageops::overlay(
                &mut many,
                &source,
                i64::from(source.width() * x),
                i64::from(source.height() * y),
            );
        }
    }
    let path = f.path("nine.png");
    many.save(&path).unwrap();
    let result = f.run(&["scan".as_ref(), path.as_os_str()]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    f.no_user_state();
}
