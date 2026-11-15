use image::{Rgba, RgbaImage};
use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-collection-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn run(&self, args: &[OsString]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("image")
            .args(args)
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

fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn combine_preserves_rgba_and_pads_unequal_images_in_both_directions() {
    let fixture = Fixture::new();
    let first = fixture.path("first $(touch never).png");
    let second = fixture.path("second.webp");
    let left = RgbaImage::from_fn(2, 1, |x, _| {
        if x == 0 {
            Rgba([12, 34, 56, 0])
        } else {
            Rgba([70, 80, 90, 128])
        }
    });
    let right = RgbaImage::from_pixel(1, 2, Rgba([1, 2, 3, 255]));
    left.save(&first).unwrap();
    right.save(&second).unwrap();
    let first_bytes = std::fs::read(&first).unwrap();
    let second_bytes = std::fs::read(&second).unwrap();
    let output = fixture.path("combined.png");
    let result = success(fixture.run(&[
        "combine".into(),
        "horizontal".into(),
        output.clone().into(),
        first.clone().into(),
        second.clone().into(),
    ]));
    assert_eq!(result["width"], 3);
    assert_eq!(result["height"], 2);
    assert_eq!(result["inputs"], 2);
    assert_eq!(
        image::open(&output).unwrap().into_rgba8(),
        RgbaImage::from_fn(3, 2, |x, y| {
            if x < 2 && y == 0 {
                *left.get_pixel(x, y)
            } else if x == 2 {
                *right.get_pixel(0, y)
            } else {
                Rgba([0, 0, 0, 0])
            }
        })
    );
    let output = fixture.path("vertical.png");
    success(fixture.run(&[
        "combine".into(),
        "vertical".into(),
        output.clone().into(),
        first.clone().into(),
        second.clone().into(),
    ]));
    assert_eq!(
        image::open(output).unwrap().into_rgba8(),
        RgbaImage::from_fn(2, 3, |x, y| {
            if y == 0 {
                *left.get_pixel(x, 0)
            } else if x == 0 {
                *right.get_pixel(0, y - 1)
            } else {
                Rgba([0, 0, 0, 0])
            }
        })
    );
    assert_eq!(std::fs::read(&first).unwrap(), first_bytes);
    assert_eq!(std::fs::read(&second).unwrap(), second_bytes);
    assert!(!fixture.path("never").exists());
    fixture.no_user_state();
}

#[test]
fn compare_reports_exact_raw_pixel_changes_including_alpha_and_hidden_rgb() {
    let fixture = Fixture::new();
    let first = fixture.path("one.png");
    let second = fixture.path("two.png");
    let before = RgbaImage::from_fn(4, 1, |x, _| match x {
        0 => Rgba([10, 20, 30, 255]),
        1 => Rgba([10, 20, 30, 10]),
        2 => Rgba([10, 20, 30, 0]),
        _ => Rgba([50, 60, 70, 80]),
    });
    let after = RgbaImage::from_fn(4, 1, |x, _| match x {
        0 => Rgba([15, 10, 60, 255]),
        1 => Rgba([10, 20, 30, 60]),
        2 => Rgba([20, 20, 30, 0]),
        _ => Rgba([50, 60, 70, 80]),
    });
    before.save(&first).unwrap();
    after.save(&second).unwrap();
    let original = std::fs::read(&first).unwrap();
    let output = fixture.path("difference.png");
    let result = success(fixture.run(&[
        "compare".into(),
        first.clone().into(),
        second.into(),
        output.clone().into(),
    ]));
    assert_eq!(result["changed_pixels"], 3);
    assert_eq!(result["total_pixels"], 4);
    assert_eq!(
        image::open(output).unwrap().into_rgba8().into_raw(),
        vec![5, 10, 30, 255, 50, 50, 50, 255, 10, 0, 0, 255, 0, 0, 0, 255]
    );
    let equal = fixture.path("equal.png");
    let result = success(fixture.run(&[
        "compare".into(),
        first.clone().into(),
        first.clone().into(),
        equal.clone().into(),
    ]));
    assert_eq!(result["changed_pixels"], 0);
    assert!(
        image::open(equal)
            .unwrap()
            .into_rgba8()
            .pixels()
            .all(|pixel| *pixel == Rgba([0, 0, 0, 255]))
    );
    assert_eq!(std::fs::read(first).unwrap(), original);
    fixture.no_user_state();
}

#[test]
fn split_row_major_tiles_reassemble_exactly_and_maximum_grid_is_supported() {
    let fixture = Fixture::new();
    let input = fixture.path("grid.png");
    let source = RgbaImage::from_fn(12, 8, |x, y| {
        Rgba([x as u8, y as u8, (x + y) as u8, (x * y) as u8])
    });
    source.save(&input).unwrap();
    let original = std::fs::read(&input).unwrap();
    let directory = fixture.path("tiles");
    std::fs::create_dir(&directory).unwrap();
    let result = success(fixture.run(&[
        "split".into(),
        input.clone().into(),
        directory.clone().into(),
        "--rows".into(),
        "2".into(),
        "--columns".into(),
        "3".into(),
    ]));
    assert_eq!(result["completed"], 6);
    assert_eq!(result["tile_width"], 4);
    assert_eq!(result["tile_height"], 4);
    let mut rebuilt = RgbaImage::new(12, 8);
    for (index, file) in result["files"].as_array().unwrap().iter().enumerate() {
        let name = file.as_str().unwrap();
        assert_eq!(name, format!("tile-{:03}.png", index + 1));
        let tile = image::open(directory.join(name)).unwrap().into_rgba8();
        for (x, y, pixel) in tile.enumerate_pixels() {
            rebuilt.put_pixel(x + index as u32 % 3 * 4, y + index as u32 / 3 * 4, *pixel);
        }
    }
    assert_eq!(rebuilt, source);
    let directory = fixture.path("sixty-four");
    std::fs::create_dir(&directory).unwrap();
    let input64 = fixture.path("grid64.png");
    RgbaImage::from_pixel(16, 4, Rgba([1, 2, 3, 4]))
        .save(&input64)
        .unwrap();
    let result = success(fixture.run(&[
        "split".into(),
        input64.into(),
        directory.clone().into(),
        "--columns".into(),
        "16".into(),
        "--rows".into(),
        "4".into(),
    ]));
    assert_eq!(result["completed"], 64);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 64);
    assert_eq!(std::fs::read(input).unwrap(), original);
    fixture.no_user_state();
}

#[test]
fn invalid_counts_grids_shapes_and_destinations_publish_nothing() {
    let fixture = Fixture::new();
    let input = fixture.path("source.png");
    RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 4]))
        .save(&input)
        .unwrap();
    let second = fixture.path("other.png");
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&second)
        .unwrap();
    let output = fixture.path("absent.png");
    for direction in ["diagonal", "horizontal"] {
        failure(fixture.run(&[
            "combine".into(),
            direction.into(),
            output.clone().into(),
            input.clone().into(),
        ]));
    }
    let mut too_many = vec!["combine".into(), "horizontal".into(), output.clone().into()];
    too_many.extend(std::iter::repeat_n(input.clone().into_os_string(), 17));
    failure(fixture.run(&too_many));
    failure(fixture.run(&[
        "compare".into(),
        input.clone().into(),
        second.into(),
        output.clone().into(),
    ]));
    failure(fixture.run(&[
        "compare".into(),
        input.clone().into(),
        input.clone().into(),
        fixture.path("lossy.jpg").into(),
    ]));
    let original = std::fs::read(&input).unwrap();
    for command in ["compare", "combine"] {
        let arguments = if command == "compare" {
            vec![
                command.into(),
                input.clone().into(),
                input.clone().into(),
                input.clone().into(),
            ]
        } else {
            vec![
                command.into(),
                "horizontal".into(),
                input.clone().into(),
                input.clone().into(),
                input.clone().into(),
            ]
        };
        failure(fixture.run(&arguments));
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }
    let directory = fixture.path("tiles");
    std::fs::create_dir(&directory).unwrap();
    for options in [
        args(&["--columns", "0", "--rows", "1"]),
        args(&["--columns", "17", "--rows", "1"]),
        args(&["--columns", "16", "--rows", "16"]),
        args(&["--columns", "2", "--rows", "1"]),
        args(&["--columns", "1", "--columns", "1"]),
        args(&["--columns", "1", "--unknown", "1"]),
        args(&["--columns", "-1", "--rows", "1"]),
        args(&["--columns", "1"]),
    ] {
        let mut arguments = vec![
            "split".into(),
            input.clone().into(),
            directory.clone().into(),
        ];
        arguments.extend(options);
        failure(fixture.run(&arguments));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
    }
    let existing = directory.join("tile-001.png");
    std::fs::write(&existing, b"keep existing").unwrap();
    failure(fixture.run(&[
        "split".into(),
        input.clone().into(),
        directory.into(),
        "--columns".into(),
        "1".into(),
        "--rows".into(),
        "1".into(),
    ]));
    assert_eq!(std::fs::read(existing).unwrap(), b"keep existing");
    let missing = fixture.path("missing-directory");
    failure(fixture.run(&[
        "split".into(),
        input.into(),
        missing.clone().into(),
        "--columns".into(),
        "1".into(),
        "--rows".into(),
        "1".into(),
    ]));
    assert!(!missing.exists());
    assert!(!output.exists());
    fixture.no_user_state();
}

#[test]
fn encoded_and_declared_decoded_oversize_inputs_fail_before_output() {
    let fixture = Fixture::new();
    let valid = fixture.path("valid.png");
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]))
        .save(&valid)
        .unwrap();
    let huge_encoded = fixture.path("encoded.png");
    std::fs::File::create(&huge_encoded)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let huge_decoded = fixture.path("decoded.png");
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(100_000_u32.to_be_bytes());
    ihdr.extend(100_000_u32.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &[]);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(&huge_decoded, png).unwrap();
    for source in [huge_encoded, huge_decoded] {
        let output = fixture.path("no.png");
        failure(fixture.run(&[
            "combine".into(),
            "horizontal".into(),
            output.clone().into(),
            valid.clone().into(),
            source.clone().into(),
        ]));
        assert!(!output.exists());
        failure(fixture.run(&[
            "compare".into(),
            valid.clone().into(),
            source.into(),
            output.clone().into(),
        ]));
        assert!(!output.exists());
    }
    fixture.no_user_state();
}

#[test]
fn small_sources_cannot_expand_into_an_oversized_combined_canvas() {
    let fixture = Fixture::new();
    let tall = fixture.path("tall.png");
    let wide = fixture.path("wide.png");
    RgbaImage::from_pixel(1, 8192, Rgba([1, 2, 3, 255]))
        .save(&tall)
        .unwrap();
    RgbaImage::from_pixel(8192, 1, Rgba([4, 5, 6, 255]))
        .save(&wide)
        .unwrap();
    for direction in ["horizontal", "vertical"] {
        let destination = fixture.path("huge.png");
        failure(fixture.run(&[
            "combine".into(),
            direction.into(),
            destination.clone().into(),
            tall.clone().into(),
            wide.clone().into(),
        ]));
        assert!(!destination.exists());
    }
    fixture.no_user_state();
}

#[test]
fn combine_supports_jpeg_input_and_lossless_webp_output_for_opaque_pixels() {
    let fixture = Fixture::new();
    let first = fixture.path("one.jpg");
    let second = fixture.path("two.png");
    image::RgbImage::from_pixel(2, 1, image::Rgb([80, 100, 120]))
        .save(&first)
        .unwrap();
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&second)
        .unwrap();
    let jpeg_pixels = image::open(&first).unwrap().into_rgba8();
    let destination = fixture.path("combined.webp");
    success(fixture.run(&[
        "combine".into(),
        "horizontal".into(),
        destination.clone().into(),
        first.into(),
        second.into(),
    ]));
    let actual = image::open(destination).unwrap().into_rgba8();
    assert_eq!(actual.dimensions(), (3, 1));
    assert_eq!(*actual.get_pixel(0, 0), *jpeg_pixels.get_pixel(0, 0));
    assert_eq!(*actual.get_pixel(1, 0), *jpeg_pixels.get_pixel(1, 0));
    assert_eq!(*actual.get_pixel(2, 0), Rgba([1, 2, 3, 255]));
    fixture.no_user_state();
}

fn chunk(output: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    output.extend((data.len() as u32).to_be_bytes());
    output.extend(name);
    output.extend(data);
    let mut crc = !0_u32;
    for byte in name.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = crc >> 1 ^ (0xedb88320_u32 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    output.extend((!crc).to_be_bytes());
}

#[cfg(unix)]
#[test]
fn native_non_utf8_filenames_and_broken_symlink_outputs_are_handled() {
    use std::os::unix::{ffi::OsStringExt as _, fs::symlink};
    let fixture = Fixture::new();
    let input = fixture
        .0
        .join(OsString::from_vec(b"source-\xff.png".to_vec()));
    RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 255]))
        .save(&input)
        .unwrap();
    let output = fixture
        .0
        .join(OsString::from_vec(b"output-\xfe.png".to_vec()));
    success(fixture.run(&[
        "combine".into(),
        "horizontal".into(),
        output.clone().into(),
        input.clone().into(),
        input.clone().into(),
    ]));
    assert_eq!(
        image::open(&output).unwrap().into_rgba8().dimensions(),
        (2, 1)
    );
    let broken = fixture.path("broken.png");
    symlink(fixture.path("missing.png"), &broken).unwrap();
    failure(fixture.run(&[
        "compare".into(),
        input.clone().into(),
        input.into(),
        broken.clone().into(),
    ]));
    assert!(
        std::fs::symlink_metadata(broken)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!fixture.path("missing.png").exists());
    fixture.no_user_state();
}
