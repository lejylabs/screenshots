use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("screenshots-strip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn source(&self) -> (PathBuf, RgbaImage) {
        let pixels = RgbaImage::from_fn(3, 4, |x, y| {
            Rgba([
                10 + x as u8,
                30 + y as u8,
                (x + y * 3) as u8,
                [0, 128, 255][x as usize],
            ])
        });
        let path = self.path("source.png");
        pixels.save(&path).unwrap();
        (path, pixels)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        command
    }
    fn run(&self, command: &mut Command) -> Output {
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Strip fixture exceeded deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn strip(
        &self,
        source: &Path,
        destination: &Path,
        axis: &str,
        start: &str,
        length: &str,
    ) -> Output {
        self.run(
            self.command()
                .args(["image", "remove-strip"])
                .arg(source)
                .arg(destination)
                .args([axis, start, length]),
        )
    }
    fn workflow(&self, recipe: &Path, source: &Path, destination: &Path) -> Output {
        self.run(
            self.command()
                .arg("workflow")
                .arg(recipe)
                .arg(source)
                .arg(destination),
        )
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

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn oracle(source: &RgbaImage, vertical: bool, start: u32, length: u32) -> RgbaImage {
    let width = source.width() - if vertical { length } else { 0 };
    let height = source.height() - if vertical { 0 } else { length };
    RgbaImage::from_fn(width, height, |x, y| {
        *source.get_pixel(
            if vertical && x >= start {
                x + length
            } else {
                x
            },
            if !vertical && y >= start {
                y + length
            } else {
                y
            },
        )
    })
}

#[test]
fn strip_removal_preserves_exact_retained_rgba_at_edges_and_middle() {
    let f = Fixture::new();
    let (source, pixels) = f.source();
    let original = std::fs::read(&source).unwrap();
    for (axis, vertical, start, length) in [
        ("horizontal", false, 0, 1),
        ("horizontal", false, 1, 2),
        ("horizontal", false, 3, 1),
        ("vertical", true, 0, 1),
        ("vertical", true, 1, 1),
        ("vertical", true, 2, 1),
    ] {
        let output = f.path(&format!("{axis}-{start}-{length}.png"));
        let result = success(f.strip(
            &source,
            &output,
            axis,
            &start.to_string(),
            &length.to_string(),
        ));
        let expected = oracle(&pixels, vertical, start, length);
        assert_eq!(image::open(output).unwrap().into_rgba8(), expected);
        assert_eq!(result["width"], expected.width());
        assert_eq!(result["height"], expected.height());
        assert_eq!(result["format"], "png");
    }
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.no_state();
}

#[test]
fn invalid_strips_and_existing_destination_do_not_change_files() {
    let f = Fixture::new();
    let (source, _) = f.source();
    let original = std::fs::read(&source).unwrap();
    let output = f.path("output.png");
    for (axis, start, length) in [
        ("diagonal", "0", "1"),
        ("horizontal", "-1", "1"),
        ("vertical", "0", "0"),
        ("vertical", "0", "3"),
        ("horizontal", "0", "4"),
        ("vertical", "3", "1"),
        ("horizontal", "3", "2"),
        ("horizontal", "4294967295", "2"),
    ] {
        let result = f.strip(&source, &output, axis, start, length);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
    }
    std::fs::write(&output, b"existing").unwrap();
    assert!(
        !f.strip(&source, &output, "horizontal", "0", "1")
            .status
            .success()
    );
    assert_eq!(std::fs::read(output).unwrap(), b"existing");
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.no_state();
}

#[test]
fn workflow_applies_redaction_before_strip_and_publishes_only_the_final_image() {
    let f = Fixture::new();
    let (source, mut pixels) = f.source();
    let original = std::fs::read(&source).unwrap();
    let recipe = f.path("recipe.json");
    let steps = json!({"version":1,"steps":[{"op":"redact","x":1,"y":2,"width":1,"height":2},{"op":"remove_strip","axis":"horizontal","start":1,"length":1}]});
    let recipe_bytes = serde_json::to_vec(&steps).unwrap();
    std::fs::write(&recipe, &recipe_bytes).unwrap();
    let output = f.path("workflow.png");
    let result = success(f.workflow(&recipe, &source, &output));
    for y in 2..4 {
        pixels.put_pixel(1, y, Rgba([0, 0, 0, 255]));
    }
    assert_eq!(
        image::open(output).unwrap().into_rgba8(),
        oracle(&pixels, false, 1, 1)
    );
    assert_eq!(result["steps"], 2);
    assert_eq!(std::fs::read(&recipe).unwrap(), recipe_bytes);
    let later = f.path("later-failure.json");
    std::fs::write(&later,serde_json::to_vec(&json!({"version":1,"steps":[{"op":"remove_strip","axis":"vertical","start":1,"length":1},{"op":"remove_strip","axis":"vertical","start":0,"length":2}]})).unwrap()).unwrap();
    let failed = f.path("failed.png");
    let result = f.workflow(&later, &source, &failed);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!failed.exists());
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.no_state();
}
