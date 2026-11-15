use image::{Rgba, RgbaImage};
use std::{
    ffi::OsString,
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
        let root =
            std::env::temp_dir().join(format!("screenshots-tiff-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[OsString]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(args)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in STATE {
            command.env(key, self.path(key));
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("TIFF input command exceeded test deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn convert(&self, input: &Path, output: &Path) -> Output {
        self.run(&[
            "image".into(),
            "convert".into(),
            input.into(),
            output.into(),
        ])
    }
    fn no_state(&self) {
        for key in STATE {
            assert!(!self.path(key).exists());
        }
        assert!(self.0.read_dir().unwrap().all(|entry| {
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

fn u16_bytes(value: u16, big: bool) -> [u8; 2] {
    if big {
        value.to_be_bytes()
    } else {
        value.to_le_bytes()
    }
}

fn u32_bytes(value: u32, big: bool) -> [u8; 4] {
    if big {
        value.to_be_bytes()
    } else {
        value.to_le_bytes()
    }
}

// Independent classic TIFF fixture: three chunky strips, with a partial last strip.
fn tiff_bytes(pixels: &RgbaImage, big: bool, samples: u32) -> Vec<u8> {
    let rows = 2;
    let strip_count = pixels.height().div_ceil(rows);
    let counts = (0..strip_count)
        .map(|strip| pixels.width() * samples * rows.min(pixels.height() - strip * rows))
        .collect::<Vec<_>>();
    let mut tags: Vec<(u16, u16, Vec<u32>)> = vec![
        (256, 4, vec![pixels.width()]),
        (257, 4, vec![pixels.height()]),
        (258, 3, vec![8; samples as usize]),
        (259, 3, vec![1]),
        (262, 3, vec![2]),
        (273, 4, vec![0; strip_count as usize]),
        (277, 3, vec![samples]),
        (278, 4, vec![rows]),
        (279, 4, counts.clone()),
        (284, 3, vec![1]),
        (339, 3, vec![1; samples as usize]),
    ];
    if samples == 4 {
        tags.push((338, 3, vec![2]));
    }
    tags.sort_by_key(|tag| tag.0);
    let directory_end = 8 + 2 + tags.len() * 12 + 4;
    let mut cursor = directory_end;
    let locations = tags
        .iter()
        .map(|(_, kind, values)| {
            let size = values.len() * if *kind == 3 { 2 } else { 4 };
            if size > 4 {
                let offset = cursor;
                cursor += size;
                Some(offset)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut raster_offset = cursor as u32;
    for tag in &mut tags {
        if tag.0 == 273 {
            for (value, count) in tag.2.iter_mut().zip(&counts) {
                *value = raster_offset;
                raster_offset += count;
            }
        }
    }
    let mut bytes = vec![0; cursor];
    bytes[..2].copy_from_slice(if big { b"MM" } else { b"II" });
    bytes[2..4].copy_from_slice(&u16_bytes(42, big));
    bytes[4..8].copy_from_slice(&u32_bytes(8, big));
    bytes[8..10].copy_from_slice(&u16_bytes(tags.len() as u16, big));
    for (index, ((tag, kind, values), location)) in tags.iter().zip(locations).enumerate() {
        let entry = 10 + index * 12;
        bytes[entry..entry + 2].copy_from_slice(&u16_bytes(*tag, big));
        bytes[entry + 2..entry + 4].copy_from_slice(&u16_bytes(*kind, big));
        bytes[entry + 4..entry + 8].copy_from_slice(&u32_bytes(values.len() as u32, big));
        let start = location.unwrap_or(entry + 8);
        if let Some(location) = location {
            bytes[entry + 8..entry + 12].copy_from_slice(&u32_bytes(location as u32, big));
        }
        for (index, value) in values.iter().enumerate() {
            if *kind == 3 {
                bytes[start + index * 2..start + index * 2 + 2]
                    .copy_from_slice(&u16_bytes(*value as u16, big));
            } else {
                bytes[start + index * 4..start + index * 4 + 4]
                    .copy_from_slice(&u32_bytes(*value, big));
            }
        }
    }
    for pixel in pixels.pixels() {
        bytes.extend_from_slice(&pixel.0[..samples as usize]);
    }
    bytes
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn independent_endian_multistrip_rgb_and_rgba_support_conversion_info_and_pixel() {
    let f = Fixture::new();
    for big in [false, true] {
        for samples in [3, 4] {
            let mut expected = pixels();
            let bytes = tiff_bytes(&expected, big, samples);
            if samples == 3 {
                for pixel in expected.pixels_mut() {
                    pixel[3] = 255;
                }
            }
            let source = f.path(&format!("source-{big}-{samples}.png"));
            let destination = f.path(&format!("decoded-{big}-{samples}.png"));
            std::fs::write(&source, &bytes).unwrap();
            assert_eq!(
                success(f.convert(&source, &destination)),
                serde_json::json!({"format":"png", "width":3, "height":5})
            );
            assert_eq!(image::open(destination).unwrap().into_rgba8(), expected);
            assert_eq!(
                success(f.run(&["image".into(), "info".into(), source.clone().into()])),
                serde_json::json!({"width":3,"height":5,"decoded_rgba_bytes":60,
                    "source_file_bytes":bytes.len()})
            );
            let sample = success(f.run(&[
                "image".into(),
                "pixel".into(),
                source.clone().into(),
                "0".into(),
                "0".into(),
            ]));
            assert_eq!(
                sample["rgba"],
                serde_json::json!(expected.get_pixel(0, 0).0)
            );
            assert_eq!(std::fs::read(source).unwrap(), bytes);
        }
    }
    f.no_state();
}

#[test]
fn own_export_roundtrip_and_inert_qr_keep_sources_unchanged() {
    let f = Fixture::new();
    let source = f.path("source.png");
    let expected = pixels();
    expected.save(&source).unwrap();
    let original = std::fs::read(&source).unwrap();
    let tiff = f.path("own.TIFF");
    assert_eq!(success(f.convert(&source, &tiff))["format"], "tiff");
    let tiff_bytes = std::fs::read(&tiff).unwrap();
    let output = f.path("roundtrip.png");
    success(f.convert(&tiff, &output));
    assert_eq!(image::open(output).unwrap().into_rgba8(), expected);
    let payload = "https://example.invalid/inert-tiff-input-test";
    let qr = f.path("qr.png");
    let qr_tiff = f.path("qr.tif");
    success(f.run(&[
        "qr".into(),
        "generate".into(),
        payload.into(),
        qr.clone().into(),
    ]));
    let qr_bytes = std::fs::read(&qr).unwrap();
    success(f.convert(&qr, &qr_tiff));
    let qr_tiff_bytes = std::fs::read(&qr_tiff).unwrap();
    let scanned = success(f.run(&["qr".into(), "scan".into(), qr_tiff.clone().into()]));
    assert_eq!(scanned["codes"].as_array().unwrap().len(), 1);
    assert_eq!(scanned["codes"][0]["text"], payload);
    for (path, bytes) in [
        (source, original),
        (tiff, tiff_bytes),
        (qr, qr_bytes),
        (qr_tiff, qr_tiff_bytes),
    ] {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    f.no_state();
}

fn entry(bytes: &[u8], tag: u16) -> usize {
    let count = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
    (0..count)
        .map(|index| 10 + index * 12)
        .find(|position| {
            u16::from_le_bytes(bytes[*position..*position + 2].try_into().unwrap()) == tag
        })
        .unwrap()
}

#[test]
fn malformed_tiff_never_publishes_and_existing_outputs_are_preserved() {
    let f = Fixture::new();
    let valid = tiff_bytes(&pixels(), false, 4);
    let mut invalid = vec![valid[..7].to_vec(), valid[..valid.len() - 1].to_vec()];
    for (offset, value) in [
        (2, 43u16),
        (entry(&valid, 259) + 8, 5),
        (entry(&valid, 338) + 8, 1),
        (entry(&valid, 284) + 8, 2),
        (entry(&valid, 339), 65000),
        (entry(&valid, 257), 256),
    ] {
        let mut bytes = valid.clone();
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        invalid.push(bytes);
    }
    let strips = u32::from_le_bytes(
        valid[entry(&valid, 273) + 8..entry(&valid, 273) + 12]
            .try_into()
            .unwrap(),
    ) as usize;
    for (offset, value) in [
        (strips, valid.len() as u32 + 1),
        (strips, 8),
        (entry(&valid, 258) + 4, 3),
        (
            10 + usize::from(u16::from_le_bytes(valid[8..10].try_into().unwrap())) * 12,
            8,
        ),
    ] {
        let mut bytes = valid.clone();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        invalid.push(bytes);
    }
    let destination = f.path("result.png");
    for (index, bytes) in invalid.iter().enumerate() {
        let source = f.path(&format!("invalid-{index}.tiff"));
        std::fs::write(&source, bytes).unwrap();
        let result = f.convert(&source, &destination);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!destination.exists());
        assert_eq!(std::fs::read(source).unwrap(), *bytes);
    }
    let source = f.path("valid.tiff");
    std::fs::write(&source, &valid).unwrap();
    std::fs::write(&destination, b"unchanged").unwrap();
    assert!(!f.convert(&source, &destination).status.success());
    assert_eq!(std::fs::read(destination).unwrap(), b"unchanged");
    #[cfg(unix)]
    {
        let target = f.path("absent.png");
        let link = f.path("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!f.convert(&source, &link).status.success());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!target.exists());
    }
    assert_eq!(std::fs::read(source).unwrap(), valid);
    f.no_state();
}
