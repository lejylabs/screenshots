use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
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
        self.command()
            .arg(recipe)
            .arg(source)
            .arg(destination)
            .output()
            .unwrap()
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
            .env_remove("WAYLAND_DISPLAY");
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
