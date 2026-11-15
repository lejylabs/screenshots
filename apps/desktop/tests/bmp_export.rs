use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("screenshots-bmp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn convert(&self, input: &Path, output: &Path, extra: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "convert"])
            .arg(input)
            .arg(output)
            .args(extra)
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
            command.env(name, self.path(name));
        }
        command.output().unwrap()
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

#[test]
fn bmp_conversion_preserves_rgba_source_and_composes_transforms() {
    let f = Fixture::new();
    let source = f.path("source.png");
    let pixels = RgbaImage::from_fn(3, 2, |x, y| {
        Rgba([
            (x * 41 + 3) as u8,
            (y * 53 + 5) as u8,
            17,
            if x == 0 {
                0
            } else if x == 1 {
                128
            } else {
                255
            },
        ])
    });
    pixels.save(&source).unwrap();
    let original = std::fs::read(&source).unwrap();
    let output = f.path("output.BMP");
    let result = f.convert(&source, &output, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(summary["format"], "bmp");
    assert_eq!(image::open(&output).unwrap().into_rgba8(), pixels);
    let transformed = f.path("transformed.bmp");
    let result = f.convert(
        &source,
        &transformed,
        &["--rotate", "90", "--flip-horizontal"],
    );
    assert!(result.status.success());
    assert_eq!(
        image::open(&transformed).unwrap().into_rgba8(),
        image::imageops::flip_horizontal(&image::imageops::rotate90(&pixels))
    );
    let resized = f.path("resized.bmp");
    assert!(
        f.convert(&source, &resized, &["--width", "1"])
            .status
            .success()
    );
    let expected = app_core::downscale_export(&pixels, app_core::ExportSize::Width(1)).unwrap();
    assert_eq!(image::open(resized).unwrap().into_rgba8(), expected);
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.no_state();
}

#[test]
fn bmp_output_rejects_quality_and_preserves_existing_destinations() {
    let f = Fixture::new();
    let input = f.path("missing.png");
    let output = f.path("output.bmp");
    for options in [["--jpeg-quality", "90"], ["--webp-quality", "75"]] {
        let result = f.convert(&input, &output, &options);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
    }
    let malformed = f.path("malformed.png");
    std::fs::write(&malformed, b"not an image").unwrap();
    assert!(!f.convert(&malformed, &output, &[]).status.success());
    assert!(!output.exists());
    let source = f.path("source.png");
    RgbaImage::from_pixel(3, 3, Rgba([5, 7, 9, 128]))
        .save(&source)
        .unwrap();
    std::fs::write(&output, b"keep").unwrap();
    assert!(!f.convert(&source, &output, &[]).status.success());
    assert_eq!(std::fs::read(output).unwrap(), b"keep");
    #[cfg(unix)]
    {
        let output = f.path("link.bmp");
        std::os::unix::fs::symlink(f.path("absent"), &output).unwrap();
        assert!(!f.convert(&source, &output, &[]).status.success());
        assert!(output.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!f.path("absent").exists());
    }
    f.no_state();
}
