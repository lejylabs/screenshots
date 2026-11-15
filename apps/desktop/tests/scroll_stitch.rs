use image::{Rgba, RgbaImage};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-scroll-stitch-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, destination: &Path, minimum: &str, inputs: &[&Path]) -> Output {
        let mut arguments = vec![destination.as_os_str().to_owned(), minimum.into()];
        arguments.extend(inputs.iter().map(|path| path.as_os_str().to_owned()));
        self.run_args(&arguments)
    }
    fn run_args(&self, arguments: &[OsString]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("stitch-scroll")
            .args(arguments)
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

fn failure(output: Output) -> String {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    String::from_utf8(output.stderr).unwrap()
}

fn save_rows(path: &Path, rows: &[u8]) {
    RgbaImage::from_fn(3, rows.len() as u32, |x, y| {
        Rgba([rows[y as usize], x as u8, 99, if x == 0 { 0 } else { 128 }])
    })
    .save(path)
    .unwrap();
}

#[test]
fn exact_multiple_overlaps_keep_original_pixels_and_source_files() {
    let fixture = Fixture::new();
    let one = fixture.path("one 東京 $(ignored).png");
    let two = fixture.path("two.webp");
    let three = fixture.path("three.png");
    save_rows(&one, &[0, 1, 2, 3, 4]);
    save_rows(&two, &[3, 4, 5, 6, 7]);
    save_rows(&three, &[6, 7, 8, 9]);
    let before: Vec<_> = [&one, &two, &three]
        .map(|path| std::fs::read(path).unwrap())
        .into();
    let destination = fixture.path("joined.png");
    let result = success(fixture.run(&destination, "2", &[&one, &two, &three]));
    assert_eq!(result["overlaps"], serde_json::json!([2, 2]));
    assert_eq!(result["width"], 3);
    assert_eq!(result["height"], 10);
    assert_eq!(result["inputs"], 3);
    let expected = RgbaImage::from_fn(3, 10, |x, y| {
        Rgba([y as u8, x as u8, 99, if x == 0 { 0 } else { 128 }])
    });
    assert_eq!(image::open(&destination).unwrap().into_rgba8(), expected);
    for (index, path) in [&one, &two, &three].into_iter().enumerate() {
        assert_eq!(std::fs::read(path).unwrap(), before[index]);
    }
    let lossless = fixture.path("joined.webp");
    success(fixture.run(&lossless, "2", &[&one, &two, &three]));
    assert_eq!(image::open(lossless).unwrap().into_rgba8(), expected);
    fixture.no_user_state();
}

#[test]
fn absence_ambiguity_identical_frames_and_transparent_rgb_mismatch_fail_without_output() {
    let fixture = Fixture::new();
    let one = fixture.path("one.png");
    let two = fixture.path("two.png");
    let output = fixture.path("absent.png");
    for (a, b, message) in [
        (vec![0, 1, 2], vec![3, 4, 5], "no exact overlap"),
        (vec![0, 1, 1], vec![1, 1, 2], "ambiguous"),
        (vec![0, 1, 2], vec![0, 1, 2], "identical"),
    ] {
        save_rows(&one, &a);
        save_rows(&two, &b);
        assert!(failure(fixture.run(&output, "1", &[&one, &two])).contains(message));
        assert!(!output.exists());
    }
    save_rows(&one, &[0, 1, 2]);
    save_rows(&two, &[1, 2, 3]);
    let mut changed = image::open(&two).unwrap().into_rgba8();
    changed.get_pixel_mut(0, 0)[0] = 77;
    changed.save(&two).unwrap();
    assert!(failure(fixture.run(&output, "2", &[&one, &two])).contains("no exact overlap"));
    assert!(!output.exists());
    fixture.no_user_state();
}

#[test]
fn minimum_and_image_count_bounds_width_height_and_no_overwrite_are_enforced() {
    let fixture = Fixture::new();
    let one = fixture.path("one.png");
    let two = fixture.path("two.png");
    save_rows(&one, &[0, 1, 2]);
    save_rows(&two, &[1, 2, 3]);
    let destination = fixture.path("absent.png");
    for minimum in ["0", "4097", "-1", "three", "3", "4294967296"] {
        failure(fixture.run(&destination, minimum, &[&one, &two]));
        assert!(!destination.exists());
    }
    failure(fixture.run(&destination, "1", &[&one]));
    let inputs = vec![one.as_path(); 17];
    failure(fixture.run(&destination, "1", &inputs));
    RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]))
        .save(&two)
        .unwrap();
    assert!(failure(fixture.run(&destination, "1", &[&one, &two])).contains("widths"));
    RgbaImage::from_pixel(3, 1, Rgba([1, 2, 3, 255]))
        .save(&two)
        .unwrap();
    assert!(failure(fixture.run(&destination, "1", &[&one, &two])).contains("height"));
    RgbaImage::from_pixel(3, 16_385, Rgba([1, 2, 3, 255]))
        .save(&two)
        .unwrap();
    assert!(failure(fixture.run(&destination, "1", &[&one, &two])).contains("height"));
    save_rows(&two, &[1, 2, 3]);
    let original = std::fs::read(&one).unwrap();
    failure(fixture.run(&one, "1", &[&one, &two]));
    assert_eq!(std::fs::read(&one).unwrap(), original);
    std::fs::write(&destination, b"keep").unwrap();
    failure(fixture.run(&destination, "1", &[&one, &two]));
    assert_eq!(std::fs::read(destination).unwrap(), b"keep");
    fixture.no_user_state();
}

#[test]
fn minimum_disambiguates_short_repeating_rows_and_maximum_inputs_work() {
    let fixture = Fixture::new();
    let one = fixture.path("one.png");
    let two = fixture.path("two.png");
    save_rows(&one, &[0, 1, 1]);
    save_rows(&two, &[1, 1, 2]);
    let destination = fixture.path("unique.png");
    assert_eq!(
        success(fixture.run(&destination, "2", &[&one, &two]))["overlaps"],
        serde_json::json!([2])
    );
    let files: Vec<_> = (0..16)
        .map(|index| {
            let file = fixture.path(&format!("{index}.png"));
            save_rows(&file, &[index, index + 1]);
            file
        })
        .collect();
    let paths: Vec<_> = files.iter().map(PathBuf::as_path).collect();
    let result = success(fixture.run(&fixture.path("sixteen.png"), "1", &paths));
    assert_eq!(result["inputs"], 16);
    assert_eq!(result["height"], 17);
    fixture.no_user_state();
}

#[test]
fn oversized_encoded_input_is_rejected_without_output() {
    let fixture = Fixture::new();
    let one = fixture.path("one.png");
    save_rows(&one, &[0, 1, 2]);
    let huge = fixture.path("huge.png");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let output = fixture.path("absent.png");
    assert!(failure(fixture.run(&output, "1", &[&one, &huge])).contains("32 MiB"));
    assert!(!output.exists());
    fixture.no_user_state();
}

#[cfg(unix)]
#[test]
fn broken_symlink_output_is_preserved() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let one = fixture.path("one.png");
    let two = fixture.path("two.png");
    save_rows(&one, &[0, 1, 2]);
    save_rows(&two, &[1, 2, 3]);
    let destination = fixture.path("broken.png");
    symlink(fixture.path("missing.png"), &destination).unwrap();
    failure(fixture.run(&destination, "1", &[&one, &two]));
    assert!(
        std::fs::symlink_metadata(destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!fixture.path("missing.png").exists());
    fixture.no_user_state();
}
