use image::{Rgba, RgbaImage};
use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-lossy-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(
        &self,
        input: &std::path::Path,
        output: &std::path::Path,
        flags: &[&str],
    ) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "convert"])
            .arg(input)
            .arg(output)
            .args(flags);
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(name, self.0.join(name));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap()
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
            assert!(!self.0.join(name).exists());
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn explicit_lossy_quality_preserves_alpha_and_leaves_lossless_default_unchanged() {
    let f = Fixture::new();
    let source = f.0.join("source.png");
    let pixels = RgbaImage::from_fn(32, 24, |x, y| {
        Rgba([
            (x * 7) as u8,
            (y * 9) as u8,
            ((x + y) * 4) as u8,
            if x % 4 == 0 { 0 } else { 173 },
        ])
    });
    pixels.save(&source).unwrap();
    let before = std::fs::read(&source).unwrap();
    let lossless = f.0.join("lossless.webp");
    assert!(f.run(&source, &lossless, &[]).status.success());
    assert_eq!(image::open(&lossless).unwrap().into_rgba8(), pixels);
    for quality in ["10", "90"] {
        let output = f.0.join(format!("lossy-{quality}.webp"));
        let result = f.run(&source, &output, &["--webp-quality", quality]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["format"], "webp");
        let decoded = image::open(&output).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), pixels.dimensions());
        assert!(
            decoded
                .pixels()
                .zip(pixels.pixels())
                .all(|(a, b)| a[3] == b[3])
        );
        assert_ne!(decoded, pixels);
        let bytes = std::fs::read(&output).unwrap();
        assert!(bytes.windows(4).any(|p| p == b"VP8 "));
        assert!(
            !f.run(&source, &output, &["--webp-quality", quality])
                .status
                .success()
        );
        assert_eq!(std::fs::read(output).unwrap(), bytes);
    }
    assert_eq!(std::fs::read(source).unwrap(), before);
    f.no_state();
}

#[test]
fn invalid_lossy_requests_fail_without_publication_or_source_access() {
    let f = Fixture::new();
    let source = f.0.join("missing.png");
    for (name, flags) in [
        ("out.webp", vec!["--webp-quality", "0"]),
        ("out.webp", vec!["--webp-quality", "101"]),
        ("out.png", vec!["--webp-quality", "80"]),
        ("out.jpg", vec!["--webp-quality", "80"]),
        (
            "out.webp",
            vec!["--webp-quality", "80", "--jpeg-quality", "80"],
        ),
        (
            "out.webp",
            vec!["--webp-quality", "80", "--webp-quality", "90"],
        ),
    ] {
        let output = f.0.join(name);
        let result = f.run(&source, &output, &flags);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
    }
    assert!(!source.exists());
    f.no_state();
}

#[test]
fn lossy_downscale_keeps_existing_alpha_resampling_semantics() {
    let f = Fixture::new();
    let source = f.0.join("alpha.png");
    let pixels = RgbaImage::from_fn(8, 4, |x, y| {
        Rgba([(x * 31) as u8, (y * 71) as u8, 90, ((x + y) * 17) as u8])
    });
    pixels.save(&source).unwrap();
    let expected = app_core::downscale_export(&pixels, app_core::ExportSize::Exact(4, 2)).unwrap();
    let output = f.0.join("small.webp");
    let result = f.run(
        &source,
        &output,
        &["--width", "4", "--height", "2", "--webp-quality", "75"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let decoded = image::open(output).unwrap().into_rgba8();
    assert_eq!(decoded.dimensions(), (4, 2));
    assert!(
        decoded
            .pixels()
            .zip(expected.pixels())
            .all(|(a, b)| a[3] == b[3])
    );
    assert_eq!(image::open(source).unwrap().into_rgba8(), pixels);
    f.no_state();
}
