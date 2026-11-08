use crate::PixelRegion;
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowAnnotation {
    pub start: [u32; 2],
    pub end: [u32; 2],
    pub color: u32,
}

impl ArrowAnnotation {
    pub fn from_drag(
        start: [f32; 2],
        end: [f32; 2],
        viewport: [f32; 2],
        image: [u32; 2],
        color: u32,
    ) -> Option<Self> {
        if start
            .into_iter()
            .chain(end)
            .chain(viewport)
            .any(|v| !v.is_finite())
            || viewport.iter().any(|v| *v <= 0.0)
            || image.contains(&0)
        {
            return None;
        }
        let map = |point: [f32; 2]| {
            std::array::from_fn(|axis| {
                ((point[axis].clamp(0.0, viewport[axis]) / viewport[axis] * image[axis] as f32)
                    .floor() as u32)
                    .min(image[axis] - 1)
            })
        };
        let arrow = Self {
            start: map(start),
            end: map(end),
            color,
        };
        (arrow.start != arrow.end).then_some(arrow)
    }

    /// Shared source-pixel polygon for GPU preview and raster export.
    pub fn vertices(self) -> [[f32; 2]; 7] {
        let dx = (i64::from(self.end[0]) - i64::from(self.start[0])) as f32;
        let dy = (i64::from(self.end[1]) - i64::from(self.start[1])) as f32;
        let length = dx.hypot(dy).max(1.0);
        let unit = [dx / length, dy / length];
        let normal = [-unit[1], unit[0]];
        let tip = [self.end[0] as f32 + 0.5, self.end[1] as f32 + 0.5];
        let start = [self.start[0] as f32 + 0.5, self.start[1] as f32 + 0.5];
        let head = 18.0f32.min(length * 0.4);
        let half_head = 7.0f32.min(head * 0.5);
        let half_shaft = 1.5f32.min(half_head * 0.5);
        let base = [tip[0] - unit[0] * head, tip[1] - unit[1] * head];
        let offset = |point: [f32; 2], amount: f32| {
            [point[0] + normal[0] * amount, point[1] + normal[1] * amount]
        };
        [
            offset(start, half_shaft),
            offset(base, half_shaft),
            offset(base, half_head),
            tip,
            offset(base, -half_head),
            offset(base, -half_shaft),
            offset(start, -half_shaft),
        ]
    }
}

pub(crate) fn paint_arrows(
    pixels: &mut image::RgbaImage,
    area: PixelRegion,
    arrows: &[ArrowAnnotation],
    ellipses: &[crate::FrameAnnotation],
) -> Result<()> {
    if arrows.is_empty() && ellipses.is_empty() {
        return Ok(());
    }
    // One bounded output-sized premultiplied overlay, released before redaction/save.
    let mut layer = tiny_skia::Pixmap::new(area.width, area.height)
        .context("Could not allocate arrow raster layer")?;
    for ellipse in ellipses {
        let mut builder = tiny_skia::PathBuilder::new();
        for contour in ellipse.ellipse_contours() {
            builder.move_to(contour[0][0] - area.x as f32, contour[0][1] - area.y as f32);
            for point in &contour[1..] {
                builder.line_to(point[0] - area.x as f32, point[1] - area.y as f32);
            }
            builder.close();
        }
        let path = builder.finish().context("Invalid ellipse geometry")?;
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(
            (ellipse.color >> 16) as u8,
            (ellipse.color >> 8) as u8,
            ellipse.color as u8,
            255,
        );
        paint.anti_alias = true;
        layer.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    for arrow in arrows {
        let vertices = arrow.vertices();
        let mut builder = tiny_skia::PathBuilder::new();
        builder.move_to(
            vertices[0][0] - area.x as f32,
            vertices[0][1] - area.y as f32,
        );
        for vertex in &vertices[1..] {
            builder.line_to(vertex[0] - area.x as f32, vertex[1] - area.y as f32);
        }
        builder.close();
        let path = builder.finish().context("Invalid arrow geometry")?;
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(
            (arrow.color >> 16) as u8,
            (arrow.color >> 8) as u8,
            arrow.color as u8,
            255,
        );
        paint.anti_alias = true;
        layer.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    for (destination, overlay) in pixels.pixels_mut().zip(layer.data().chunks_exact(4)) {
        let alpha = u32::from(overlay[3]);
        if alpha == 0 {
            continue;
        }
        let inverse = 255 - alpha;
        let old_alpha = u32::from(destination[3]);
        let out_alpha = alpha * 255 + old_alpha * inverse;
        for channel in 0..3 {
            destination[channel] = ((u32::from(overlay[channel]) * 65025
                + u32::from(destination[channel]) * old_alpha * inverse
                + out_alpha / 2)
                / out_alpha)
                .min(255) as u8;
        }
        destination[3] = ((out_alpha + 127) / 255) as u8;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ellipse_outline_keeps_center_and_respects_crop_and_redaction() -> Result<()> {
        let original = image::RgbaImage::from_pixel(48, 40, image::Rgba([80, 120, 200, 17]));
        let source = crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 48,
            height: 40,
            pixels: std::sync::Arc::new(original.clone()),
            png_path: "fixture.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let ellipse = crate::FrameAnnotation {
            area: PixelRegion {
                x: 8,
                y: 8,
                width: 32,
                height: 24,
            },
            color: 0x22c55e,
        };
        let output = crate::render_document_snapshot(&source, &[], None, &[], &[], &[ellipse])?;
        assert_eq!(
            *output.pixels.get_pixel(9, 20),
            image::Rgba([34, 197, 94, 255])
        );
        assert_eq!(output.pixels.get_pixel(24, 20), original.get_pixel(24, 20));
        assert_eq!(output.pixels.get_pixel(8, 8), original.get_pixel(8, 8));
        let crop = PixelRegion {
            x: 8,
            y: 8,
            width: 16,
            height: 24,
        };
        let redact = PixelRegion {
            x: 8,
            y: 18,
            width: 4,
            height: 4,
        };
        let output =
            crate::render_document_snapshot(&source, &[redact], Some(crop), &[], &[], &[ellipse])?;
        assert_eq!((output.width, output.height), (16, 24));
        assert_eq!(*output.pixels.get_pixel(1, 12), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*source.pixels, original);
        assert!(
            ellipse
                .ellipse_contours()
                .iter()
                .all(|contour| contour.len() <= 256)
        );
        Ok(())
    }
    #[test]
    fn arrow_drag_supports_axis_aligned_and_reversed_directions() {
        let arrow = ArrowAnnotation::from_drag(
            [100.0, 20.0],
            [0.0, 20.0],
            [100.0, 100.0],
            [200, 200],
            0xef4444,
        )
        .unwrap();
        assert_eq!(arrow.start, [199, 40]);
        assert_eq!(arrow.end, [0, 40]);
        assert_eq!(arrow.vertices()[3], [0.5, 40.5]);
        assert!(arrow.vertices()[2][0] > 0.5);
        assert!(
            ArrowAnnotation::from_drag([10.0, 90.0], [10.0, 20.0], [100.0, 100.0], [100, 100], 0)
                .is_some()
        );
        assert!(ArrowAnnotation::from_drag([1.0; 2], [1.0; 2], [100.0; 2], [100; 2], 0).is_none());
        assert!(
            ArrowAnnotation::from_drag([f32::NAN; 2], [2.0; 2], [100.0; 2], [100; 2], 0).is_none()
        );
    }

    #[test]
    fn arrow_export_preserves_unpainted_alpha_and_redaction_wins() -> Result<()> {
        let original = image::RgbaImage::from_pixel(64, 32, image::Rgba([80, 120, 200, 17]));
        let source = crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 64,
            height: 32,
            pixels: std::sync::Arc::new(original.clone()),
            png_path: "fixture.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let arrow = ArrowAnnotation {
            start: [5, 16],
            end: [48, 16],
            color: 0xef4444,
        };
        let result = crate::render_annotated_snapshot(&source, &[], None, &[], &[arrow])?;
        assert_eq!(
            *result.pixels.get_pixel(20, 16),
            image::Rgba([239, 68, 68, 255])
        );
        assert_eq!(result.pixels.get_pixel(0, 0), original.get_pixel(0, 0));
        let crop = PixelRegion {
            x: 8,
            y: 14,
            width: 28,
            height: 5,
        };
        let redact = PixelRegion {
            x: 18,
            y: 15,
            width: 5,
            height: 3,
        };
        let result =
            crate::render_annotated_snapshot(&source, &[redact], Some(crop), &[], &[arrow])?;
        assert_eq!((result.width, result.height), (28, 5));
        assert_eq!(*result.pixels.get_pixel(12, 2), image::Rgba([0, 0, 0, 255]));
        assert_eq!(
            *result.pixels.get_pixel(2, 2),
            image::Rgba([239, 68, 68, 255])
        );
        assert_eq!(*source.pixels, original);
        assert!(
            crate::render_annotated_snapshot(
                &source,
                &[],
                None,
                &[],
                &[ArrowAnnotation {
                    end: [100, 16],
                    ..arrow
                }]
            )
            .is_err()
        );
        Ok(())
    }
}
