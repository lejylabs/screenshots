#![cfg(target_os = "windows")]

use image::{Rgba, RgbaImage};
use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "screenshots-ocr-windows-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("ocr").current_dir(&self.0);
        for (variable, name) in [
            ("USERPROFILE", "userprofile"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            command.env(variable, self.path(name));
        }
        command
    }

    fn source(&self) -> PathBuf {
        let path = self.path("synthetic.png");
        RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]))
            .save(&path)
            .unwrap();
        path
    }

    fn assert_unchanged(&self, source: &PathBuf, original: &[u8]) {
        assert_eq!(std::fs::read(source).unwrap(), original);
        for name in [
            "userprofile",
            "appdata",
            "localappdata",
            "config",
            "data",
            "cache",
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

#[test]
fn windows_engine_lookup_reports_result_without_ui_or_user_state() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let original = std::fs::read(&source).unwrap();
    let output = fixture
        .command()
        .arg(&source)
        .env("PATH", fixture.path("no-engines"))
        .output()
        .unwrap();
    // Windows also searches beside the app and in system directories, even with
    // an empty PATH. A separately installed engine may run this fixture.
    if output.status.success() {
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["engine"], "tesseract");
        assert!(result["text"].is_string());
    } else {
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("engine is unavailable") || error.contains("OCR engine failed"),
            "{error}"
        );
    }
    fixture.assert_unchanged(&source, &original);
}

#[test]
fn windows_ocr_rejects_language_injection_and_oversized_pixels_before_spawn() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let original = std::fs::read(&source).unwrap();
    for language in ["eng&whoami", "eng|whoami", "eng\n", "../eng", "eng++deu"] {
        let output = fixture
            .command()
            .args([source.as_os_str(), "--language".as_ref(), language.as_ref()])
            .env("PATH", fixture.path("no-engines"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("OCR language"));
        fixture.assert_unchanged(&source, &original);
    }
    let large = fixture.path("oversized.png");
    RgbaImage::from_pixel(2049, 2048, Rgba([255, 255, 255, 255]))
        .save(&large)
        .unwrap();
    let original = std::fs::read(&large).unwrap();
    let output = fixture
        .command()
        .arg(&large)
        .env("PATH", fixture.path("no-engines"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("memory limit"));
    fixture.assert_unchanged(&large, &original);
}
