use image::{GenericImageView, Rgba, RgbaImage};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-image-tools-{}-{}",
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

    fn run(&self, mode: &str, source: &Path, destination: &Path, options: &[&OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("image")
            .arg(mode)
            .arg(source)
            .arg(destination)
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

#[test]
fn executable_composes_padding_and_clipped_alpha_watermark() {
    let fixture = Fixture::new();
    let input = fixture.path("source $(ignored).png");
    let watermark = fixture.path("logo.png");
    let source = RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255]));
    source.save(&input).unwrap();
    RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 128]))
        .save(&watermark)
        .unwrap();
    let input_bytes = std::fs::read(&input).unwrap();
    let watermark_bytes = std::fs::read(&watermark).unwrap();
    let output = fixture.path("canvas.webp");
    let result = success(fixture.run(
        "compose",
        &input,
        &output,
        &[
            "--padding".as_ref(),
            "1".as_ref(),
            "--background".as_ref(),
            "000000".as_ref(),
            "--watermark".as_ref(),
            watermark.as_os_str(),
            "--at".as_ref(),
            "3".as_ref(),
            "3".as_ref(),
        ],
    ));
    assert_eq!(result["width"], 4);
    assert_eq!(result["height"], 4);
    let actual = image::open(&output).unwrap().into_rgba8();
    let expected = RgbaImage::from_fn(4, 4, |x, y| {
        if x == 3 && y == 3 {
            Rgba([0, 0, 128, 255])
        } else if (1..=2).contains(&x) && (1..=2).contains(&y) {
            Rgba([20, 40, 60, 255])
        } else {
            Rgba([0, 0, 0, 255])
        }
    });
    assert_eq!(actual, expected);
    assert_eq!(std::fs::read(input).unwrap(), input_bytes);
    assert_eq!(std::fs::read(watermark).unwrap(), watermark_bytes);
    fixture.no_user_state();
}

#[test]
fn transparent_composition_and_explicit_background_preserve_alpha_semantics() {
    let fixture = Fixture::new();
    let input = fixture.path("alpha.png");
    let source = RgbaImage::from_fn(2, 1, |x, _| {
        if x == 0 {
            Rgba([255, 0, 0, 128])
        } else {
            Rgba([0, 255, 0, 0])
        }
    });
    source.save(&input).unwrap();
    let output = fixture.path("transparent.png");
    success(fixture.run(
        "compose",
        &input,
        &output,
        &["--padding".as_ref(), "1".as_ref()],
    ));
    let actual = image::open(output).unwrap().into_rgba8();
    assert_eq!(actual.dimensions(), (4, 3));
    assert_eq!(*actual.get_pixel(0, 0), Rgba([0, 0, 0, 0]));
    assert_eq!(*actual.get_pixel(1, 1), Rgba([255, 0, 0, 128]));
    assert_eq!(*actual.get_pixel(2, 1), Rgba([0, 0, 0, 0]));
    let translucent = fixture.path("background.png");
    success(fixture.run(
        "compose",
        &input,
        &translucent,
        &["--background".as_ref(), "01020380".as_ref()],
    ));
    let actual = image::open(translucent).unwrap().into_rgba8();
    assert_eq!(*actual.get_pixel(0, 0), Rgba([171, 1, 1, 192]));
    assert_eq!(*actual.get_pixel(1, 0), Rgba([1, 2, 3, 128]));
    let jpeg = fixture.path("opaque.jpg");
    success(fixture.run(
        "compose",
        &input,
        &jpeg,
        &["--background".as_ref(), "FFFFFF".as_ref()],
    ));
    assert_eq!(image::open(jpeg).unwrap().dimensions(), (2, 1));
    fixture.no_user_state();
}

#[test]
fn nearest_resize_supports_exact_upscale_downscale_and_identity_without_alpha_bleed() {
    let fixture = Fixture::new();
    let input = fixture.path("source.png");
    let source = RgbaImage::from_fn(2, 2, |x, y| match (x, y) {
        (0, 0) => Rgba([255, 0, 0, 0]),
        (1, 0) => Rgba([0, 255, 0, 255]),
        (0, 1) => Rgba([0, 0, 255, 128]),
        _ => Rgba([20, 40, 60, 255]),
    });
    source.save(&input).unwrap();
    let bytes = std::fs::read(&input).unwrap();
    let enlarged = fixture.path("enlarged.png");
    success(fixture.run("resize", &input, &enlarged, &["4".as_ref(), "4".as_ref()]));
    assert_eq!(
        image::open(&enlarged).unwrap().into_rgba8(),
        RgbaImage::from_fn(4, 4, |x, y| *source.get_pixel(x / 2, y / 2))
    );
    let reduced = fixture.path("reduced.webp");
    success(fixture.run("resize", &enlarged, &reduced, &["2".as_ref(), "2".as_ref()]));
    assert_eq!(image::open(reduced).unwrap().into_rgba8(), source);
    let single = fixture.path("single.png");
    success(fixture.run("resize", &input, &single, &["1".as_ref(), "1".as_ref()]));
    assert_eq!(
        *image::open(single).unwrap().into_rgba8().get_pixel(0, 0),
        Rgba([20, 40, 60, 255])
    );
    let identity = fixture.path("identity.png");
    success(fixture.run("resize", &input, &identity, &["2".as_ref(), "2".as_ref()]));
    assert_eq!(image::open(identity).unwrap().into_rgba8(), source);
    assert_eq!(std::fs::read(input).unwrap(), bytes);
    fixture.no_user_state();
}

#[test]
fn invalid_compose_resize_and_existing_outputs_never_publish_files() {
    let fixture = Fixture::new();
    let input = fixture.path("alpha.png");
    RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 128]))
        .save(&input)
        .unwrap();
    let output = fixture.path("absent.png");
    for options in [
        vec!["--padding", "4294967295"],
        vec!["--padding", "65536"],
        vec!["--padding", "-1"],
        vec!["--background", "not-rgb"],
        vec!["--background", "FFF"],
        vec!["--padding", "0", "--padding", "1"],
        vec!["--at", "0", "0"],
        vec!["--watermark", "missing.png"],
        vec!["--watermark", "missing.png", "--at", "2", "0"],
        vec!["--watermark", "missing.png", "--at", "0", "0"],
        vec!["--at", "0"],
        vec!["--unknown", "0"],
    ] {
        let options: Vec<_> = options.iter().map(OsStr::new).collect();
        assert!(
            !fixture
                .run("compose", &input, &output, &options)
                .status
                .success()
        );
        assert!(!output.exists());
    }
    for options in [
        vec!["0", "1"],
        vec!["1", "0"],
        vec!["65536", "65536"],
        vec!["-1", "2"],
        vec!["1"],
        vec!["1", "2", "3"],
    ] {
        let options: Vec<_> = options.iter().map(OsStr::new).collect();
        assert!(
            !fixture
                .run("resize", &input, &output, &options)
                .status
                .success()
        );
        assert!(!output.exists());
    }
    for mode in ["compose", "resize"] {
        let options: &[&OsStr] = if mode == "resize" {
            &["3".as_ref(), "3".as_ref()]
        } else {
            &[]
        };
        let original = std::fs::read(&input).unwrap();
        assert!(!fixture.run(mode, &input, &input, options).status.success());
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let jpeg = fixture.path("alpha.jpg");
        assert!(!fixture.run(mode, &input, &jpeg, options).status.success());
        assert!(!jpeg.exists());
    }
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn bounded_decoder_checks_exact_remaining_composition_budget() {
    let fixture = Fixture::new();
    let source = fixture.path("watermark.png");
    let pixels = RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 4]));
    pixels.save(&source).unwrap();
    assert!(app_core::load_editor_image_with_limit(&source, 63).is_err());
    assert!(app_core::load_editor_image_with_limit(&source, 0).is_err());
    assert_eq!(
        app_core::load_editor_image_with_limit(&source, 64).unwrap(),
        pixels
    );
    let oversized = fixture.path("oversized.png");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let output = fixture.path("absent.png");
    assert!(
        !fixture
            .run("compose", &oversized, &output, &[])
            .status
            .success()
    );
    assert!(!output.exists());
    fixture.no_user_state();
}
