use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const STATE: [&str; 7] = [
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("screenshots-tiff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn convert(&self, source: &Path, destination: &Path, extra: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "convert"])
            .arg(source)
            .arg(destination)
            .args(extra)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in STATE {
            command.env(name, self.path(name));
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("TIFF conversion exceeded test deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn no_state(&self) {
        for name in STATE {
            assert!(!self.path(name).exists());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pixels() -> RgbaImage {
    RgbaImage::from_fn(3, 5, |x, y| {
        Rgba([
            (x * 71 + y * 19) as u8,
            (x * 29 + y * 61) as u8,
            (x * 47 + y * 37 + 13) as u8,
            [0, 128, 255][((x + y) % 3) as usize],
        ])
    })
}

fn success(output: Output, width: u32, height: u32) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"format":"tiff","width":width,"height":height})
    );
}

#[test]
fn tiff_conversion_preserves_rgba_and_existing_transform_semantics() {
    let f = Fixture::new();
    let source = f.path("source.png");
    let pixels = pixels();
    pixels.save(&source).unwrap();
    let original = std::fs::read(&source).unwrap();
    for extension in ["tif", "TIFF"] {
        let output = f.path(&format!("plain.{extension}"));
        success(f.convert(&source, &output, &[]), 3, 5);
        assert_eq!(image::open(output).unwrap().into_rgba8(), pixels);
    }
    let transformed = f.path("transformed.tiff");
    success(
        f.convert(
            &source,
            &transformed,
            &["--rotate", "90", "--flip-horizontal"],
        ),
        5,
        3,
    );
    assert_eq!(
        image::open(transformed).unwrap().into_rgba8(),
        image::imageops::flip_horizontal(&image::imageops::rotate90(&pixels))
    );
    let smaller = f.path("smaller.TIF");
    let expected = app_core::downscale_export(&pixels, app_core::ExportSize::Width(1)).unwrap();
    success(
        f.convert(&source, &smaller, &["--width", "1"]),
        expected.width(),
        expected.height(),
    );
    assert_eq!(image::open(smaller).unwrap().into_rgba8(), expected);
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.no_state();
}

#[test]
fn tiff_quality_rejection_precedes_input_access_and_never_overwrites() {
    let f = Fixture::new();
    let missing = f.path("missing.png");
    let destination = f.path("output.tiff");
    for extra in [["--jpeg-quality", "90"], ["--webp-quality", "75"]] {
        let result = f.convert(&missing, &destination, &extra);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("quality"));
        assert!(!destination.exists());
    }
    let source = f.path("source.png");
    pixels().save(&source).unwrap();
    std::fs::write(&destination, b"unchanged").unwrap();
    let result = f.convert(&source, &destination, &[]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(&destination).unwrap(), b"unchanged");
    let malformed = f.path("malformed.png");
    std::fs::write(&malformed, b"invalid").unwrap();
    let new = f.path("new.tif");
    assert!(!f.convert(&malformed, &new, &[]).status.success());
    assert!(!new.exists());
    #[cfg(unix)]
    {
        let target = f.path("absent");
        let link = f.path("link.tiff");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!f.convert(&source, &link, &[]).status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!target.exists());
    }
    f.no_state();
}
