//! In-memory clipboard images. Keep the clipboard alive for clipboard ownership on Linux.

use std::borrow::Cow;

use anyhow::{Context, Result};

#[derive(Default)]
pub struct ImageClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl ImageClipboard {
    /// Explicit recopy only; the live capture/selection pipeline stays in memory.
    pub fn copy_saved_png(&mut self, path: &std::path::Path) -> Result<(u32, u32)> {
        let mut reader = image::ImageReader::open(path)?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
        reader.limits(limits);
        let decoded = reader.decode()?;
        crate::validate_image_size(decoded.width(), decoded.height())?;
        self.copy_image(&decoded.into_rgba8())
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
