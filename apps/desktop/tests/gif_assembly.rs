use image::{AnimationDecoder, Rgba, RgbaImage};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("screenshots-gif-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn image(&self, name: &str, pixels: &RgbaImage) -> PathBuf {
        let path = self.path(name);
        pixels.save(&path).unwrap();
        path
    }
    fn run(&self, output: &Path, delay: &str, inputs: &[PathBuf]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("gif")
            .arg(output)
            .args(["--delay-ms", delay])
            .args(inputs)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
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
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("GIF fixture exceeded deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
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
        assert!(std::fs::read_dir(&self.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_pixels(actual: &RgbaImage, expected: &RgbaImage) {
    assert_eq!(actual.dimensions(), expected.dimensions());
    for (a, b) in actual.pixels().zip(expected.pixels()) {
        assert_eq!(a[3], b[3]);
        if b[3] != 0 {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn file_frames_keep_delay_binary_alpha_and_clear_previous_pixels() {
    let f = Fixture::new();
    let first = RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255]));
    let mut second = RgbaImage::from_pixel(3, 2, Rgba([17, 39, 85, 0]));
    second.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
    let third = RgbaImage::from_pixel(3, 2, Rgba([0, 255, 0, 255]));
    let inputs = [
        f.image("first.png", &first),
        f.image("second.png", &second),
        f.image("third.png", &third),
    ];
    let originals = inputs
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let output = f.path("animation.GIF");
    let result = f.run(&output, "120", &inputs);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(summary["frames"], 3);
    assert_eq!(summary["format"], "gif");
    assert_eq!(summary["delay_ms"], 120);
    let frames = image::codecs::gif::GifDecoder::new(BufReader::new(File::open(output).unwrap()))
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap();
    assert_eq!(frames.len(), 3);
    for (frame, expected) in frames.iter().zip([&first, &second, &third]) {
        assert_eq!(frame.delay().numer_denom_ms(), (120, 1));
        assert_pixels(frame.buffer(), expected);
    }
    for (input, original) in inputs.iter().zip(originals) {
        assert_eq!(std::fs::read(input).unwrap(), original);
    }
    f.no_state();
}

#[test]
fn invalid_count_timing_pixels_and_collisions_publish_nothing() {
    let f = Fixture::new();
    let good = f.image(
        "good.png",
        &RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255])),
    );
    let partial = f.image(
        "partial.png",
        &RgbaImage::from_pixel(3, 2, Rgba([0, 0, 255, 128])),
    );
    let mismatch = f.image(
        "mismatch.png",
        &RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 255])),
    );
    let sources = [good.clone(), partial.clone(), mismatch.clone()];
    let originals = sources
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let output = f.path("animation.gif");
    for (delay, inputs) in [
        ("120", vec![good.clone()]),
        ("120", vec![good.clone(); 17]),
        ("0", vec![good.clone(); 2]),
        ("15", vec![good.clone(); 2]),
        ("60010", vec![good.clone(); 2]),
        ("120", vec![good.clone(), partial]),
        ("120", vec![good.clone(), mismatch]),
    ] {
        let result = f.run(&output, delay, &inputs);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        f.no_state();
    }
    std::fs::write(&output, b"unchanged").unwrap();
    let result = f.run(&output, "120", &[good.clone(), good]);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(output).unwrap(), b"unchanged");
    for (source, original) in sources.iter().zip(originals) {
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
    f.no_state();
}
