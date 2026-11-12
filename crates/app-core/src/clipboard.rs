//! In-memory clipboard images. Keep the clipboard alive for clipboard ownership on Linux.

use std::borrow::Cow;

use anyhow::{Context, Result};

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
        for _ in 0..5 {
            let start = std::time::Instant::now();
            std::hint::black_box(load_saved_png(&path)?);
            decode.push(start.elapsed());
            let start = std::time::Instant::now();
            let preview = load_png_thumbnail(&path, || false)?;
            previews.push(start.elapsed());
            assert_eq!(preview.dimensions(), (400, 225));
            assert_eq!(preview.as_raw().len(), 360_000);
        }
        decode.sort_unstable();
        previews.sort_unstable();
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
