//! In-memory clipboard images. Keep the clipboard alive for clipboard ownership on Linux.

use std::borrow::Cow;

use anyhow::{Context, Result};

const MAX_EDITOR_FILE_BYTES: u64 = 32 * 1024 * 1024;

// JPEG reads its input in full before applying decoder limits. Freeze the readable
// extent so a growing file cannot bypass the encoded-byte admission check.
struct EditorFile {
    file: std::fs::File,
    length: u64,
    position: u64,
}

impl std::io::Read for EditorFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let available = self.length.saturating_sub(self.position).min(buffer.len() as u64) as usize;
        let read = self.file.read(&mut buffer[..available])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl std::io::Seek for EditorFile {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        let from = match from {
            std::io::SeekFrom::End(offset) => {
                let position = i128::from(self.length) + i128::from(offset);
                std::io::SeekFrom::Start(u64::try_from(position).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid image seek")
                })?)
            }
            other => other,
        };
        self.position = self.file.seek(from)?;
        Ok(self.position)
    }
}

/// Explicit editor input only; run decoding and orientation on a worker.
pub fn load_editor_image(path: &std::path::Path) -> Result<image::RgbaImage> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    anyhow::ensure!(length <= MAX_EDITOR_FILE_BYTES, "Image file exceeds the 32 MiB input limit");
    let mut reader = image::ImageReader::new(std::io::BufReader::new(EditorFile { file, length, position: 0 }))
        .with_guessed_format()?;
    anyhow::ensure!(
        matches!(reader.format(), Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)),
        "Choose a PNG, JPEG or WebP image"
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    decode_editor_pixels(reader.into_decoder()?)
}

pub(crate) fn decode_editor_pixels(mut decoder: impl image::ImageDecoder) -> Result<image::RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    decoder.set_limits(limits)?;
    let (width, height) = decoder.dimensions();
    crate::validate_image_size(width, height)?;
    anyhow::ensure!(decoder.total_bytes() <= crate::MAX_CAPTURE_BYTES, "Decoded image exceeds the memory limit");
    let orientation = decoder.orientation()?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    Ok(decoded.into_rgba8())
}

pub fn load_saved_png(path: &std::path::Path) -> Result<image::RgbaImage> {
    let mut reader = image::ImageReader::with_format(
        std::io::BufReader::new(std::fs::File::open(path)?),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode()?;
    crate::validate_image_size(decoded.width(), decoded.height())?;
    Ok(decoded.into_rgba8())
}

/// Explicit history preview only. Decode on a worker, retain only a bounded thumbnail.
pub fn load_png_thumbnail(
    path: &std::path::Path,
    cancelled: impl Fn() -> bool,
) -> Result<image::RgbaImage> {
    anyhow::ensure!(!cancelled(), "Preview canceled");
    let source = load_saved_png(path)?;
    anyhow::ensure!(!cancelled(), "Preview canceled");
    let (width, height) = (source.width().min(400), source.height().min(240));
    let thumbnail = image::DynamicImage::ImageRgba8(source)
        .thumbnail(width, height)
        .into_rgba8();
    anyhow::ensure!(!cancelled(), "Preview canceled");
    Ok(thumbnail)
}

#[derive(Default)]
pub struct ImageClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl ImageClipboard {
    /// Explicit text output; retain ownership using the same native clipboard instance.
    pub fn copy_text(&mut self, text: String) -> Result<()> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("open clipboard")?);
        }
        self.clipboard.as_mut().unwrap().set_text(text).context("copy text to clipboard")
    }

    /// Explicit recopy only; the live capture/selection pipeline stays in memory.
    pub fn copy_saved_png(&mut self, path: &std::path::Path) -> Result<(u32, u32)> {
        self.copy_image(&load_saved_png(path)?)
    }

    pub fn copy_image(&mut self, image: &image::RgbaImage) -> Result<(u32, u32)> {
        let (width, height) = image.dimensions();
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("open clipboard")?);
        }
        let clipboard = self.clipboard.as_mut().unwrap();
        clipboard
            .set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Borrowed(image.as_raw()),
            })
            .context("copy image to clipboard")?;
        Ok((width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "uses the system clipboard and replaces its contents with a synthetic path"]
    fn file_path_text_round_trip() -> Result<()> {
        let mut clipboard = ImageClipboard::default();
        let text = "C:\\Saved captures\\Grüße image.png".to_string();
        clipboard.copy_text(text.clone())?;
        assert_eq!(clipboard.clipboard.as_mut().unwrap().get_text()?, text);
        Ok(())
    }

    #[test]
    fn editor_input_extent_is_bounded_even_when_the_file_grows() -> Result<()> {
        use std::io::{Read as _, Seek as _, Write as _};
        let path = std::env::temp_dir().join(format!("screenshots-editor-extent-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"abcd")?;
        let mut input = EditorFile { file: std::fs::File::open(&path)?, length: 4, position: 0 };
        std::fs::OpenOptions::new().append(true).open(&path)?.write_all(b"private trailing data")?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"abcd");
        assert_eq!(input.seek(std::io::SeekFrom::End(-2))?, 2);
        bytes.clear();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"cd");
        assert!(input.seek(std::io::SeekFrom::End(-5)).is_err());
        input.seek(std::io::SeekFrom::Start(1))?;
        assert_eq!(input.seek(std::io::SeekFrom::Current(1))?, 2);
        input.seek(std::io::SeekFrom::Start(100))?;
        assert_eq!(input.read(&mut [0; 1])?, 0);
        drop(input);
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        file.set_len(MAX_EDITOR_FILE_BYTES + 1)?;
        drop(file);
        assert!(load_editor_image(&path).unwrap_err().to_string().contains("32 MiB"));
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn editor_images_detect_content_preserve_files_and_apply_orientation() -> Result<()> {
        use image::ImageEncoder as _;
        let root = std::env::temp_dir().join(format!("screenshots-editor-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("misleading.extension");
        let source = image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78]));
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::WebP] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            let image = image::DynamicImage::ImageRgba8(source.clone());
            if format == image::ImageFormat::Jpeg {
                image.to_rgb8().write_to(&mut encoded, format)?;
            } else {
                image.write_to(&mut encoded, format)?;
            }
            let bytes = encoded.into_inner();
            std::fs::write(&path, &bytes)?;
            let loaded = load_editor_image(&path)?;
            assert_eq!(loaded.dimensions(), (3, 2));
            if format != image::ImageFormat::Jpeg { assert_eq!(loaded, source); }
            assert_eq!(std::fs::read(&path)?, bytes);
        }
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100);
        // TIFF orientation 6: rotate 90 degrees clockwise.
        encoder.set_exif_metadata(vec![73,73,42,0,8,0,0,0,1,0,18,1,3,0,1,0,0,0,6,0,0,0,0,0,0,0])?;
        let rgb = image::RgbImage::from_fn(3, 2, |x, y| image::Rgb([(x * 80) as u8, (y * 120) as u8, 40]));
        encoder.write_image(rgb.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)?;
        std::fs::write(&path, &jpeg)?;
        let unrotated = image::load_from_memory(&jpeg)?.into_rgba8();
        assert_eq!(load_editor_image(&path)?, image::imageops::rotate90(&unrotated));
        assert_eq!(std::fs::read(&path)?, jpeg);
        for bytes in [b"not an image".as_slice(), b"\x89PNG\r\n\x1a\n", b"GIF89a"] {
            std::fs::write(&path, bytes)?;
            assert!(load_editor_image(&path).is_err());
        }
        std::fs::remove_file(path)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    fn editor_image_rejects_oversized_header_before_pixel_decode() -> Result<()> {
        let path = std::env::temp_dir().join(format!("screenshots-editor-limit-{}.png", uuid::Uuid::new_v4()));
        image::RgbaImage::new(1, 1).save(&path)?;
        let mut bytes = std::fs::read(&path)?;
        bytes[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&100_000_u32.to_be_bytes());
        let mut crc = u32::MAX;
        for byte in &bytes[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 { crc = (crc >> 1) ^ (0xedb88320 & (0_u32.wrapping_sub(crc & 1))); }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        std::fs::write(&path, bytes)?;
        let error = load_editor_image(&path).unwrap_err().to_string();
        assert!(error.contains("limit"), "{error}");
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn thumbnails_are_bounded_cancellable_and_keep_pngs_unchanged() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-preview-{}", uuid::Uuid::new_v4()));
        let path = root.join("image.png");
        assert!(
            load_png_thumbnail(&path, || true)
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        assert!(!root.exists());
        std::fs::create_dir_all(&root)?;
        for (width, height, expected) in [
            (640, 480, (320, 240)),
            (1, 10000, (1, 240)),
            (10000, 1, (400, 1)),
            (3, 2, (3, 2)),
        ] {
            image::RgbaImage::from_pixel(width, height, image::Rgba([11, 22, 33, 90]))
                .save(&path)?;
            let before = std::fs::read(&path)?;
            let thumbnail = load_png_thumbnail(&path, || false)?;
            assert_eq!(thumbnail.dimensions(), expected);
            assert!(thumbnail.as_raw().len() <= 400 * 240 * 4);
            assert!(thumbnail.pixels().all(|pixel| pixel.0 == [11, 22, 33, 90]));
            assert_eq!(std::fs::read(&path)?, before);
        }
        for cancel_at in [2, 3] {
            let calls = std::cell::Cell::new(0);
            assert!(
                load_png_thumbnail(&path, || {
                    calls.set(calls.get() + 1);
                    calls.get() == cancel_at
                })
                .is_err()
            );
            assert_eq!(calls.get(), cancel_at);
        }
        std::fs::write(&path, b"invalid PNG")?;
        assert!(load_png_thumbnail(&path, || false).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "short optimized synthetic 4K history preview comparison; no desktop or clipboard"]
    fn history_preview_4k_cost() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "screenshots-preview-cost-{}.png",
            uuid::Uuid::new_v4()
        ));
        image::RgbaImage::from_fn(3840, 2160, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8, 255])
        })
        .save(&path)?;
        let mut decode = Vec::new();
        let mut previews = Vec::new();
        let mut editor = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            std::hint::black_box(load_saved_png(&path)?);
            decode.push(start.elapsed());
            let start = std::time::Instant::now();
            let preview = load_png_thumbnail(&path, || false)?;
            previews.push(start.elapsed());
            assert_eq!(preview.dimensions(), (400, 225));
            assert_eq!(preview.as_raw().len(), 360_000);
            let start = std::time::Instant::now();
            std::hint::black_box(load_editor_image(&path)?);
            editor.push(start.elapsed());
        }
        decode.sort_unstable();
        previews.sort_unstable();
        editor.sort_unstable();
        eprintln!("4K editor image decode median={:?}; 5 synthetic warm samples", editor[2]);
        eprintln!(
            "4K PNG decode median={:?}; decode+thumbnail median={:?}; retained=360000 bytes; source RGBA=33177600 bytes; 5 synthetic warm samples",
            decode[2], previews[2]
        );
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    #[ignore = "uses the system clipboard and replaces its contents"]
    fn png_round_trip() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("clipboard-spike-{}.png", uuid::Uuid::new_v4()));
        let image = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 80) as u8, (y * 120) as u8, 47, 255])
        });
        image.save(&path)?;
        let mut clipboard = ImageClipboard::default();
        let result = clipboard.copy_image(&image);
        let blocked_parent =
            std::env::temp_dir().join(format!("save-failure-{}", uuid::Uuid::new_v4()));
        std::fs::write(&blocked_parent, b"not a directory")?;
        let shot = crate::CapturedImage {
            display_id: "test".into(),
            window_id: None,
            width: image.width(),
            height: image.height(),
            pixels: std::sync::Arc::new(image.clone()),
            png_path: blocked_parent.join("capture.png"),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        assert!(shot.save_png().is_err());
        std::fs::remove_file(blocked_parent)?;
        let pasted = clipboard
            .clipboard
            .as_mut()
            .unwrap()
            .get_image()
            .context("read clipboard image")?;
        anyhow::ensure!(
            pasted.width == image.width() as usize
                && pasted.height == image.height() as usize
                && pasted.bytes.as_ref() == image.as_raw().as_slice(),
            "clipboard round trip changed image dimensions or RGBA pixels"
        );

        std::fs::remove_file(path)?;
        assert_eq!(result?, (3, 2));
        Ok(())
    }
}
