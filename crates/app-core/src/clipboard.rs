//! Phase 0 image round trip. Keep the clipboard alive for clipboard ownership on Linux.

use std::{borrow::Cow, path::Path};

use anyhow::{Context, Result, ensure};

#[derive(Default)]
pub struct ImageClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl ImageClipboard {
    pub fn copy_png_and_verify(&mut self, path: &Path) -> Result<(u32, u32)> {
        let image = image::open(path).context("read capture PNG")?.into_rgba8();
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
        let pasted = clipboard.get_image().context("read clipboard image")?;
        ensure!(
            pasted.width == width as usize
                && pasted.height == height as usize
                && pasted.bytes.as_ref() == image.as_raw().as_slice(),
            "clipboard round trip changed image dimensions or RGBA pixels"
        );
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
        let result = ImageClipboard::default().copy_png_and_verify(&path);
        std::fs::remove_file(path)?;
        assert_eq!(result?, (3, 2));
        Ok(())
    }
}
