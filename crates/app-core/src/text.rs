use crate::PixelRegion;
use anyhow::{Result, ensure};
use std::sync::Arc;

/// Immutable shaped glyphs; undo shares the mask rather than copying its pixels.
#[derive(Clone)]
pub struct TextAnnotation {
    pub area: PixelRegion,
    pub color: u32,
    pub width: u32,
    pub height: u32,
    pub mask: Arc<[u8]>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> crate::CapturedImage {
        crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 4,
            height: 3,
            pixels: Arc::new(image::RgbaImage::from_pixel(
                4,
                3,
                image::Rgba([19, 37, 61, 0]),
            )),
            png_path: "not-written.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        }
    }
    #[test]
    fn glyph_masks_clip_scale_and_composite_without_leaking_original_pixels() -> Result<()> {
        let source = source();
        let area = PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 1,
        };
        let stamp = TextAnnotation {
            area,
            color: 0xff8000,
            width: 2,
            height: 1,
            mask: vec![255, 128].into(),
        };
        let output = crate::render_text_snapshot(
            &source,
            &[],
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
            &[stamp.clone()],
        )?;
        assert_eq!(output.pixels.get_pixel(1, 1).0, [255, 128, 0, 255]);
        assert_eq!(output.pixels.get_pixel(2, 1).0, [255, 128, 0, 128]);
        assert_eq!(output.pixels.get_pixel(0, 0), source.pixels.get_pixel(0, 0));
        let cropped = crate::render_text_snapshot(
            &source,
            &[],
            Some(PixelRegion {
                x: 2,
                y: 1,
                width: 1,
                height: 1,
            }),
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
            &[stamp.clone()],
        )?;
        assert_eq!(
            cropped.pixels.get_pixel(0, 0),
            output.pixels.get_pixel(2, 1)
        );
        let redacted = crate::render_text_snapshot(
            &source,
            &[area],
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
            &[stamp.clone()],
        )?;
        assert_eq!(redacted.pixels.get_pixel(1, 1).0, [0, 0, 0, 255]);
        let mut resized = stamp.clone();
        resized.area = PixelRegion {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let scaled = crate::render_text_snapshot(
            &source,
            &[],
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            &[],
            &[resized],
        )?;
        assert_eq!(scaled.pixels.get_pixel(0, 0).0, [255, 128, 0, 255]);
        assert_eq!(scaled.pixels.get_pixel(3, 1).0, [255, 128, 0, 128]);
        assert_eq!(source.pixels.get_pixel(1, 1).0, [19, 37, 61, 0]);
        let mut invalid = stamp.clone();
        invalid.mask = vec![255].into();
        assert!(
            crate::render_text_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &[],
                None,
                &[],
                &[invalid]
            )
            .is_err()
        );
        assert!(
            crate::render_text_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &[],
                None,
                &[],
                &vec![stamp; 101]
            )
            .is_err()
        );
        Ok(())
    }
}
impl TextAnnotation {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.color <= 0xffffff
                && self.width > 0
                && self.width <= 1024
                && self.height > 0
                && self.height <= 128
                && self.mask.len() == (self.width * self.height) as usize,
            "Invalid text raster"
        );
        Ok(())
    }
    pub(crate) fn paint(&self, output: &mut image::RgbaImage, crop: PixelRegion) {
        for y in self.area.y.max(crop.y)..(self.area.y + self.area.height).min(crop.y + crop.height)
        {
            for x in
                self.area.x.max(crop.x)..(self.area.x + self.area.width).min(crop.x + crop.width)
            {
                let sx = (((x - self.area.x) as f64 + 0.5) * self.width as f64
                    / self.area.width as f64
                    - 0.5)
                    .clamp(0.0, (self.width - 1) as f64);
                let sy = (((y - self.area.y) as f64 + 0.5) * self.height as f64
                    / self.area.height as f64
                    - 0.5)
                    .clamp(0.0, (self.height - 1) as f64);
                let (left, top) = (sx.floor() as u32, sy.floor() as u32);
                let sample = |px: u32, py: u32| self.mask[(py * self.width + px) as usize] as f64;
                let (fx, fy) = (sx - left as f64, sy - top as f64);
                let alpha = ((sample(left, top) * (1.0 - fx)
                    + sample((left + 1).min(self.width - 1), top) * fx)
                    * (1.0 - fy)
                    + (sample(left, (top + 1).min(self.height - 1)) * (1.0 - fx)
                        + sample(
                            (left + 1).min(self.width - 1),
                            (top + 1).min(self.height - 1),
                        ) * fx)
                        * fy)
                    .round() as u32;
                if alpha == 0 {
                    continue;
                }
                let destination = output.get_pixel_mut(x - crop.x, y - crop.y);
                let previous_alpha = u32::from(destination[3]);
                let out_alpha = alpha * 255 + previous_alpha * (255 - alpha);
                for (channel, shift) in [16, 8, 0].into_iter().enumerate() {
                    let color = (self.color >> shift) & 255;
                    destination[channel] = ((color * alpha * 255
                        + u32::from(destination[channel]) * previous_alpha * (255 - alpha)
                        + out_alpha / 2)
                        / out_alpha) as u8;
                }
                destination[3] = ((out_alpha + 127) / 255) as u8;
            }
        }
    }
}
