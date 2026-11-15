use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-bmp-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn convert(&self, input: &Path, output: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "convert"])
            .arg(input)
            .arg(output)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, self.path(key));
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("BMP input conversion stalled");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn no_state(&self) {
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.path(key).exists());
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

fn bmp_info(pixels: &RgbaImage, bits: u16, top_down: bool) -> Vec<u8> {
    let stride = (pixels.width() * u32::from(bits)).div_ceil(32) * 4;
    let size = 54 + stride * pixels.height();
    let mut bytes = Vec::with_capacity(size as usize);
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(pixels.width() as i32).to_le_bytes());
    bytes.extend_from_slice(
        &(if top_down {
            -(pixels.height() as i32)
        } else {
            pixels.height() as i32
        })
        .to_le_bytes(),
    );
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(stride * pixels.height()).to_le_bytes());
    bytes.extend_from_slice(&[0; 16]);
    for stored_y in 0..pixels.height() {
        let y = if top_down {
            stored_y
        } else {
            pixels.height() - 1 - stored_y
        };
        let start = bytes.len();
        for x in 0..pixels.width() {
            let p = pixels.get_pixel(x, y);
            bytes.extend_from_slice(&[p[2], p[1], p[0]]);
            if bits == 32 {
                bytes.push(p[3]);
            }
        }
        bytes.resize(start + stride as usize, 0);
    }
    bytes
}

fn success(result: Output) {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["format"], "png");
}

#[test]
fn exported_bgra_bmp_reenters_loader_with_exact_hidden_rgb_and_alpha() {
    let f = Fixture::new();
    let expected = pixels();
    let mut bytes = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bytes)
        .encode(expected.as_raw(), 3, 5, image::ExtendedColorType::Rgba8)
        .unwrap();
    let source = f.path("looks-like.png");
    std::fs::write(&source, &bytes).unwrap();
    let output = f.path("decoded.png");
    success(f.convert(&source, &output));
    assert_eq!(image::open(output).unwrap().into_rgba8(), expected);
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    f.no_state();
}

#[test]
fn padded_bgr_and_bgrx_decode_both_orientations_as_opaque() {
    let f = Fixture::new();
    let mut expected = pixels();
    for pixel in expected.pixels_mut() {
        pixel[3] = 255;
    }
    for bits in [24, 32] {
        for top_down in [false, true] {
            let bytes = bmp_info(&pixels(), bits, top_down);
            let source = f.path(&format!("input-{bits}-{top_down}.bmp"));
            let output = source.with_extension("png");
            std::fs::write(&source, &bytes).unwrap();
            success(f.convert(&source, &output));
            assert_eq!(image::open(output).unwrap().into_rgba8(), expected);
            assert_eq!(std::fs::read(source).unwrap(), bytes);
        }
    }
    f.no_state();
}

#[test]
fn malformed_bmp_extent_masks_compression_and_palette_never_publish() {
    let f = Fixture::new();
    let valid = bmp_info(&pixels(), 24, false);
    let mut invalid = Vec::new();
    let mut size = valid.clone();
    size[2..6].copy_from_slice(&12u32.to_le_bytes());
    invalid.push(size);
    let mut compressed = valid.clone();
    compressed[30..34].copy_from_slice(&1u32.to_le_bytes());
    invalid.push(compressed);
    let mut palette = valid.clone();
    palette[46..50].copy_from_slice(&2u32.to_le_bytes());
    invalid.push(palette);
    let mut offset = valid.clone();
    offset[10..14].copy_from_slice(&55u32.to_le_bytes());
    invalid.push(offset);
    let mut truncated = valid;
    truncated.pop();
    invalid.push(truncated);
    let mut alpha = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut alpha)
        .encode(pixels().as_raw(), 3, 5, image::ExtendedColorType::Rgba8)
        .unwrap();
    alpha[54..58].copy_from_slice(&0x000000ffu32.to_le_bytes());
    invalid.push(alpha);
    for (index, bytes) in invalid.iter().enumerate() {
        let source = f.path(&format!("invalid-{index}.bmp"));
        let output = source.with_extension("png");
        std::fs::write(&source, bytes).unwrap();
        let result = f.convert(&source, &output);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        assert_eq!(std::fs::read(source).unwrap(), *bytes);
    }
    f.no_state();
}
