use image::{ImageEncoder, Rgba, RgbaImage};
use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-image-metadata-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn run(&self, bytes: &[u8]) -> std::process::Output {
        let path = self.0.join("image.wrong-extension");
        std::fs::write(&path, bytes).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(["image", "metadata"])
            .arg(&path)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(name, self.0.join(name));
        }
        let output = command.output().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&self.0).unwrap().count(), 1);
        output
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn jpeg_and_webp_inventory_detect_content_without_exposing_metadata() {
    let f = Fixture::new();
    let secret = b"PRIVATE GPS AND CAMERA PAYLOAD";
    let image = RgbaImage::from_pixel(3, 2, Rgba([11, 22, 33, 255]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&image)
        .unwrap();
    let mut app = vec![0xff, 0xe1];
    app.extend_from_slice(&((secret.len() + 2) as u16).to_be_bytes());
    app.extend_from_slice(secret);
    jpeg.splice(2..2, app);
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .write_image(image.as_raw(), 3, 2, image::ExtendedColorType::Rgba8)
        .unwrap();
    let mut chunks = Vec::new();
    chunks.extend_from_slice(b"VP8X");
    chunks.extend_from_slice(&10u32.to_le_bytes());
    chunks.extend_from_slice(&[0x08, 0, 0, 0, 2, 0, 0, 1, 0, 0]);
    chunks.extend_from_slice(&webp[12..]);
    chunks.extend_from_slice(b"EXIF");
    chunks.extend_from_slice(&(secret.len() as u32).to_le_bytes());
    chunks.extend_from_slice(secret);
    if secret.len() % 2 == 1 {
        chunks.push(0);
    }
    webp = b"RIFF".to_vec();
    webp.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    webp.extend_from_slice(b"WEBP");
    webp.extend(chunks);
    for (bytes, format) in [(&jpeg, "jpeg"), (&webp, "webp")] {
        let result = f.run(bytes);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["format"], format);
        assert_eq!(json["width"], 3);
        assert_eq!(json["height"], 2);
        assert_eq!(json["metadata_present"], true);
        assert!(
            !result
                .stdout
                .windows(secret.len())
                .any(|window| window == secret)
        );
        let truncated = f.run(&bytes[..bytes.len() - 1]);
        assert!(!truncated.status.success());
        assert!(truncated.stdout.is_empty());
        assert!(
            !truncated
                .stderr
                .windows(secret.len())
                .any(|window| window == secret)
        );
    }
}
