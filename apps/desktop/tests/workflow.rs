use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-workflow-{}-{}",
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

    fn recipe(&self, value: Value) -> PathBuf {
        let path = self.path("recipe.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    fn source(&self) -> PathBuf {
        let path = self.path("source $(untouched).png");
        RgbaImage::from_fn(4, 3, |x, y| {
            Rgba([10 + x as u8 * 40, 20 + y as u8 * 60, (x + y) as u8, 255])
        })
        .save(&path)
        .unwrap();
        path
    }

    fn run(&self, recipe: &Path, source: &Path, destination: &Path) -> Output {
        let mut child = self
            .command()
            .arg(recipe)
            .arg(source)
            .arg(destination)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "Workflow fixture timed out: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("workflow")
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
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        command
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
        assert!(std::fs::read_dir(&self.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }

    fn fails(&self, recipe: &Path, source: &Path, output: &Path) {
        let result = self.run(recipe, source, output);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        self.no_user_state();
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

fn effect_oracle(source: &RgbaImage, area: [u32; 4], blur: bool) -> RgbaImage {
    let mut output = source.clone();
    for y in area[1]..area[1] + area[3] {
        for x in area[0]..area[0] + area[2] {
            let (left, top, right, bottom) = if blur {
                (
                    x.saturating_sub(8),
                    y.saturating_sub(8),
                    (x + 9).min(source.width()),
                    (y + 9).min(source.height()),
                )
            } else {
                let left = x / 16 * 16;
                let top = y / 16 * 16;
                (
                    left,
                    top,
                    (left + 16).min(source.width()),
                    (top + 16).min(source.height()),
                )
            };
            let mut sums = [0u64; 4];
            for row in top..bottom {
                for column in left..right {
                    let color = source.get_pixel(column, row).0;
                    for channel in 0..3 {
                        sums[channel] += u64::from(color[channel]) * u64::from(color[3]);
                    }
                    sums[3] += u64::from(color[3]);
                }
            }
            output.put_pixel(
                x,
                y,
                Rgba([
                    if sums[3] == 0 {
                        0
                    } else {
                        (sums[0] / sums[3]) as u8
                    },
                    if sums[3] == 0 {
                        0
                    } else {
                        (sums[1] / sums[3]) as u8
                    },
                    if sums[3] == 0 {
                        0
                    } else {
                        (sums[2] / sums[3]) as u8
                    },
                    (sums[3] / u64::from((right - left) * (bottom - top))) as u8,
                ]),
            );
        }
    }
    output
}

#[test]
fn effects_flatten_in_recipe_order_match_independent_alpha_oracles_and_preserve_source() {
    let fixture = Fixture::new();
    let source = fixture.path("pattern.png");
    let input = RgbaImage::from_fn(20, 19, |x, y| {
        Rgba([
            ((x * 47 + y * 19) % 256) as u8,
            ((x * 17 + y * 71) % 256) as u8,
            ((x * 61 + y * 13) % 256) as u8,
            ((x * 31 + y * 43) % 256) as u8,
        ])
    });
    input.save(&source).unwrap();
    let original = std::fs::read(&source).unwrap();
    let recipe = fixture.recipe(json!({"version":1,"steps":[
        {"op":"crop","x":1,"y":1,"width":18,"height":17},
        {"op":"blur","x":1,"y":0,"width":9,"height":12},
        {"op":"redact","x":4,"y":4,"width":3,"height":4},
        {"op":"pixelate","x":2,"y":1,"width":14,"height":15},
        {"op":"flip","axis":"horizontal"},
        {"op":"blur","x":0,"y":0,"width":7,"height":8},
        {"op":"pixelate","x":11,"y":9,"width":7,"height":8}
    ]}));
    let destination = fixture.path("effects.webp");
    let summary = success(fixture.run(&recipe, &source, &destination));
    assert_eq!(summary["steps"], 7);
    assert_eq!(summary["width"], 18);
    assert_eq!(summary["height"], 17);
    let mut expected = image::imageops::crop_imm(&input, 1, 1, 18, 17).to_image();
    expected = effect_oracle(&expected, [1, 0, 9, 12], true);
    for y in 4..8 {
        for x in 4..7 {
            expected.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    expected = effect_oracle(&expected, [2, 1, 14, 15], false);
    image::imageops::flip_horizontal_in_place(&mut expected);
    expected = effect_oracle(&expected, [0, 0, 7, 8], true);
    expected = effect_oracle(&expected, [11, 9, 7, 8], false);
    assert_eq!(image::open(&destination).unwrap().into_rgba8(), expected);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    let saved = std::fs::read(&destination).unwrap();
    assert!(!fixture.run(&recipe, &source, &destination).status.success());
    assert_eq!(std::fs::read(&destination).unwrap(), saved);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 3);
    fixture.no_user_state();
}

#[test]
fn effects_after_redaction_cannot_restore_original_secret_pixels() {
    let fixture = Fixture::new();
    let recipe = fixture.recipe(json!({"version":1,"steps":[
        {"op":"redact","x":3,"y":2,"width":7,"height":6},
        {"op":"blur","x":0,"y":0,"width":20,"height":18},
        {"op":"pixelate","x":0,"y":0,"width":20,"height":18},
        {"op":"rotate","degrees":180}
    ]}));
    let mut results = Vec::new();
    for secret in [40, 230] {
        let source = fixture.path(&format!("secret-{secret}.png"));
        let input = RgbaImage::from_fn(20, 18, |x, y| {
            if (3..10).contains(&x) && (2..8).contains(&y) {
                Rgba([secret, 255 - secret, 92, 110])
            } else {
                Rgba([12, 74, 160, 255])
            }
        });
        input.save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        let destination = fixture.path(&format!("redacted-{secret}.png"));
        success(fixture.run(&recipe, &source, &destination));
        results.push(image::open(destination).unwrap().into_rgba8());
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
    assert_eq!(results[0], results[1]);
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn fifo_recipe_is_rejected_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let source = fixture.source();
    let recipe = fixture.path("recipe.fifo");
    let native = std::ffi::CString::new(recipe.as_os_str().as_bytes()).unwrap();
    // mkfifo borrows a valid NUL-terminated fixture path only for this call.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let output = fixture.run(&recipe, &source, &fixture.path("absent.png"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("regular file"));
    assert!(output.stdout.is_empty());
    assert!(!fixture.path("absent.png").exists());
    fixture.no_user_state();
}

#[test]
fn executable_runs_every_step_in_order_and_saves_only_the_final_pixels() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let original = std::fs::read(&source).unwrap();
    let recipe = fixture.recipe(json!({"version": 1, "steps": [
        {"op": "crop", "x": 1, "y": 0, "width": 3, "height": 2},
        {"op": "rotate", "degrees": 90},
        {"op": "flip", "axis": "horizontal"},
        {"op": "flip", "axis": "vertical"},
        {"op": "redact", "x": 0, "y": 1, "width": 1, "height": 2},
        {"op": "resize", "width": 4, "height": 6},
        {"op": "canvas", "padding": 1, "background": "112233"}
    ]}));
    let destination = fixture.path("final.webp");
    let result = success(fixture.run(&recipe, &source, &destination));
    assert_eq!(
        result,
        json!({
            "version": 1, "steps": 7, "format": "webp", "width": 6, "height": 8
        })
    );
    let input = image::open(&source).unwrap().into_rgba8();
    let expected = RgbaImage::from_fn(6, 8, |x, y| {
        if x == 0 || x == 5 || y == 0 || y == 7 {
            return Rgba([17, 34, 51, 255]);
        }
        let column = (x - 1) / 2;
        let row = (y - 1) / 2;
        match (column, row) {
            (0, 0) => *input.get_pixel(3, 0),
            (0, _) => Rgba([0, 0, 0, 255]),
            (1, 0) => *input.get_pixel(3, 1),
            (1, 1) => *input.get_pixel(2, 1),
            (1, 2) => *input.get_pixel(1, 1),
            _ => unreachable!(),
        }
    });
    assert_eq!(image::open(&destination).unwrap().into_rgba8(), expected);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 3);
    fixture.no_user_state();
}

#[test]
fn relative_paths_use_the_invoking_directory_not_the_recipe_directory() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let directory = fixture.path("recipes");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("rotate.json"),
        br#"{"version":1,"steps":[{"op":"rotate","degrees":180}]}"#,
    )
    .unwrap();
    success(fixture.run(
        Path::new("recipes/rotate.json"),
        Path::new(source.file_name().unwrap()),
        Path::new("relative.png"),
    ));
    let input = image::open(source).unwrap().into_rgba8();
    let expected = RgbaImage::from_fn(4, 3, |x, y| *input.get_pixel(3 - x, 2 - y));
    assert_eq!(
        image::open(fixture.path("relative.png"))
            .unwrap()
            .into_rgba8(),
        expected
    );
    assert!(!directory.join("relative.png").exists());
    fixture.no_user_state();
}

#[test]
fn rotation270_resize_downsample_and_transparent_canvas_keep_exact_pixels() {
    let fixture = Fixture::new();
    let source = fixture.path("alpha.png");
    let input = RgbaImage::from_fn(2, 2, |x, y| {
        Rgba([30 + x as u8 * 60, 10 + y as u8 * 40, 5, 128])
    });
    input.save(&source).unwrap();
    let recipe = fixture.recipe(json!({"version": 1, "steps": [
        {"op": "rotate", "degrees": 270},
        {"op": "resize", "width": 1, "height": 1},
        {"op": "canvas", "padding": 1, "background": "00000000"}
    ]}));
    let destination = fixture.path("transparent.png");
    success(fixture.run(&recipe, &source, &destination));
    let expected = RgbaImage::from_fn(3, 3, |x, y| {
        if x == 1 && y == 1 {
            *input.get_pixel(0, 1)
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    assert_eq!(image::open(destination).unwrap().into_rgba8(), expected);
    fixture.no_user_state();
}

#[test]
fn strict_schema_rejects_malformed_unknown_duplicate_and_unsupported_fields() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let recipe = fixture.path("invalid.json");
    let output = fixture.path("absent.png");
    for invalid in [
        r#"{"version":1,"steps":[]}"#,
        r#"{"version":2,"steps":[{"op":"rotate","degrees":90}]}"#,
        r#"{"version":1,"version":1,"steps":[{"op":"rotate","degrees":90}]}"#,
        r#"{"version":1,"steps":[{"op":"rotate","degrees":90}],"output":"secret.png"}"#,
        r#"{"version":1,"steps":[{"op":"rotate","degrees":90,"degrees":180}]}"#,
        r#"{"version":1,"steps":[{"op":"rotate","degrees":90,"shell":"true"}]}"#,
        r#"{"version":1,"steps":[{"op":"upload","url":"https://invalid.example"}]}"#,
        r#"{"version":1,"steps":[{"op":"flip","axis":"diagonal"}]}"#,
        r#"{"version":1,"steps":[{"op":"rotate","degrees":45}]}"#,
        r#"{"version":1,"steps":[{"op":"crop","x":-1,"y":0,"width":1,"height":1}]}"#,
        r#"{"version":1,"steps":[{"op":"blur","x":0,"y":0,"width":1,"height":1,"radius":3}]}"#,
        r#"{"version":1,"steps":[{"op":"pixelate","x":0,"y":0,"width":1,"height":1,"block_size":8}]}"#,
        r#"{"version":1,"steps":[{"op":"blur","x":0,"x":1,"y":0,"width":1,"height":1}]}"#,
        r#"{"version":1,"steps":[{"op":"pixelate","x":0,"y":0,"width":1}]}"#,
        r#"{"version":1,"steps":[{"op":"blur","x":0.5,"y":0,"width":1,"height":1}]}"#,
        r#"{"version":1,"steps":[{"op":"canvas","padding":0,"background":"GG0000"}]}"#,
        r#"{"version":1,"steps":[{"op":"canvas","padding":1}]}"#,
        r#"{"version":1,"steps":[{"op":"rotate","degrees":90}]} trailing"#,
        "{",
    ] {
        std::fs::write(&recipe, invalid).unwrap();
        fixture.fails(&recipe, &source, &output);
    }
    assert!(!fixture.command().output().unwrap().status.success());
    assert!(
        !fixture
            .command()
            .args([
                OsStr::new("a"),
                OsStr::new("b"),
                OsStr::new("c"),
                OsStr::new("d")
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn bounds_and_later_step_failure_leave_the_source_and_output_unchanged() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let original = std::fs::read(&source).unwrap();
    let output = fixture.path("absent.png");
    for step in [
        json!({"op":"resize","width":0,"height":1}),
        json!({"op":"resize","width":65536,"height":65536}),
        json!({"op":"canvas","padding":4294967295_u32,"background":"000000"}),
        json!({"op":"canvas","padding":65536,"background":"000000"}),
        json!({"op":"crop","x":4294967295_u32,"y":0,"width":2,"height":1}),
        json!({"op":"crop","x":2,"y":0,"width":1,"height":1}),
        json!({"op":"redact","x":0,"y":1,"width":1,"height":1}),
        json!({"op":"blur","x":0,"y":1,"width":1,"height":1}),
        json!({"op":"pixelate","x":2,"y":0,"width":1,"height":1}),
        json!({"op":"blur","x":0,"y":0,"width":0,"height":1}),
        json!({"op":"pixelate","x":0,"y":0,"width":65536,"height":65536}),
        json!({"op":"blur","x":4294967295_u32,"y":0,"width":2,"height":1}),
        json!({"op":"pixelate","x":0,"y":4294967295_u32,"width":1,"height":2}),
    ] {
        let recipe = fixture.recipe(json!({"version":1,"steps":[
            {"op":"resize","width":2,"height":1}, step
        ]}));
        fixture.fails(&recipe, &source, &output);
        assert_eq!(std::fs::read(&source).unwrap(), original);
    }
    let recipe = fixture.recipe(json!({"version":1,"steps":[{"op":"rotate","degrees":90}]}));
    std::fs::write(&output, b"existing destination").unwrap();
    assert!(!fixture.run(&recipe, &source, &output).status.success());
    assert_eq!(std::fs::read(&output).unwrap(), b"existing destination");
    assert!(!fixture.run(&recipe, &source, &source).status.success());
    assert_eq!(std::fs::read(source).unwrap(), original);
    fixture.no_user_state();
}

#[test]
fn recipe_size_and_step_limits_are_enforced_at_their_boundaries() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let recipe = fixture.recipe(json!({"version":1,"steps":
        vec![json!({"op":"flip","axis":"horizontal"}); 16]
    }));
    let mut bytes = std::fs::read(&recipe).unwrap();
    bytes.resize(64 * 1024, b' ');
    std::fs::write(&recipe, &bytes).unwrap();
    let output = fixture.path("boundary.png");
    success(fixture.run(&recipe, &source, &output));
    assert_eq!(
        image::open(&output).unwrap().into_rgba8(),
        image::open(&source).unwrap().into_rgba8()
    );
    bytes.push(b' ');
    std::fs::write(&recipe, bytes).unwrap();
    fixture.fails(&recipe, &source, &fixture.path("oversized.png"));
    let recipe = fixture.recipe(json!({"version":1,"steps":
        vec![json!({"op":"flip","axis":"horizontal"}); 17]
    }));
    fixture.fails(&recipe, &source, &fixture.path("too-many.png"));
    fixture.fails(&fixture.0, &source, &fixture.path("directory.png"));
    fixture.no_user_state();
}

#[test]
fn oversized_or_malformed_images_and_transparent_jpeg_never_publish() {
    let fixture = Fixture::new();
    let recipe = fixture.recipe(json!({"version":1,"steps":[{"op":"rotate","degrees":90}]}));
    let source = fixture.path("invalid.png");
    std::fs::write(&source, b"not an image").unwrap();
    fixture.fails(&recipe, &source, &fixture.path("invalid-output.png"));
    std::fs::File::create(&source)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    fixture.fails(&recipe, &source, &fixture.path("large-output.png"));
    RgbaImage::from_pixel(1, 1, Rgba([255, 20, 30, 0]))
        .save(&source)
        .unwrap();
    fixture.fails(&recipe, &source, &fixture.path("transparent.jpg"));
    fixture.fails(&recipe, &source, &fixture.path("unsupported.gif"));
    fixture.no_user_state();
}
