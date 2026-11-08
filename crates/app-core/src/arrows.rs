use crate::PixelRegion;
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrowAnnotation {
    pub start: [u32; 2],
    pub end: [u32; 2],
    pub color: u32,
    pub width: u8,
}

impl ArrowAnnotation {
    /// Snap the source-pixel direction to the nearest horizontal, vertical or 45-degree axis.
    pub fn from_drag_constrained(
        start: [f32; 2],
        end: [f32; 2],
        viewport: [f32; 2],
        image: [u32; 2],
        color: u32,
        snap: bool,
    ) -> Option<Self> {
        let mut arrow = Self::from_drag(start, end, viewport, image, color)?;
        if !snap {
            return Some(arrow);
        }
        let dx = arrow.start[0].abs_diff(arrow.end[0]);
        let dy = arrow.start[1].abs_diff(arrow.end[1]);
        let threshold = 1.0 + std::f64::consts::SQRT_2;
        if f64::from(dx) > f64::from(dy) * threshold {
            arrow.end[1] = arrow.start[1];
        } else if f64::from(dy) > f64::from(dx) * threshold {
            arrow.end[0] = arrow.start[0];
        } else {
            let reverse: [bool; 2] =
                std::array::from_fn(|axis| arrow.end[axis] < arrow.start[axis]);
            let mut side = ((u64::from(dx) + u64::from(dy) + 1) / 2) as u32;
            for axis in 0..2 {
                side = side.min(if reverse[axis] {
                    arrow.start[axis]
                } else {
                    image[axis] - 1 - arrow.start[axis]
                });
            }
            arrow.end = std::array::from_fn(|axis| {
                if reverse[axis] {
                    arrow.start[axis] - side
                } else {
                    arrow.start[axis] + side
                }
            });
        }
        (arrow.start != arrow.end).then_some(arrow)
    }
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
            width: 3,
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
        let width = f32::from(self.width);
        let head = (6.0 * width).min(length * 0.4);
        let half_head = (7.0 * width / 3.0).min(head * 0.5);
        let half_shaft = (width / 2.0).min(half_head * 0.5);
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
    pub fn hit_test(self, point: [f32; 2], tolerance: f32) -> bool {
        if point.iter().any(|v| !v.is_finite()) || !tolerance.is_finite() || tolerance < 0.0 {
            return false;
        }
        let vertices = self.vertices();
        let mut inside = false;
        for index in 0..vertices.len() {
            let a = vertices[index];
            let b = vertices[(index + 1) % vertices.len()];
            let delta = [b[0] - a[0], b[1] - a[1]];
            let length = delta[0] * delta[0] + delta[1] * delta[1];
            let t = if length > 0.0 {
                (((point[0] - a[0]) * delta[0] + (point[1] - a[1]) * delta[1]) / length)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance_squared =
                (point[0] - a[0] - t * delta[0]).powi(2) + (point[1] - a[1] - t * delta[1]).powi(2);
            if distance_squared <= tolerance * tolerance {
                return true;
            }
            if (a[1] > point[1]) != (b[1] > point[1])
                && point[0] < a[0] + (point[1] - a[1]) * delta[0] / delta[1]
            {
                inside = !inside;
            }
        }
        inside
    }
}

pub(crate) fn paint_shapes(
    layer: &mut tiny_skia::Pixmap,
    area: PixelRegion,
    arrows: &[ArrowAnnotation],
    ellipses: &[crate::FrameAnnotation],
) -> Result<()> {
    if arrows.is_empty() && ellipses.is_empty() {
        return Ok(());
    }
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constrained_arrows_snap_in_source_pixels_and_stay_in_image() {
        let horizontal = ArrowAnnotation::from_drag_constrained(
            [10.0; 2],
            [80.0, 20.0],
            [100.0; 2],
            [100; 2],
            0,
            true,
        )
        .unwrap();
        assert_eq!(horizontal.end, [80, 10]);
        let vertical = ArrowAnnotation::from_drag_constrained(
            [10.0; 2],
            [20.0, 80.0],
            [100.0; 2],
            [100; 2],
            0,
            true,
        )
        .unwrap();
        assert_eq!(vertical.end, [10, 80]);
        let diagonal = ArrowAnnotation::from_drag_constrained(
            [10.0; 2],
            [60.0; 2],
            [200.0, 100.0],
            [400; 2],
            0,
            true,
        )
        .unwrap();
        assert_eq!(diagonal.start, [20, 40]);
        assert_eq!(diagonal.end, [170, 190]);
        for viewport in [[100.0; 2], [200.0, 100.0], [333.3, 77.7]] {
            for image in [[100; 2], [400, 200], [1, 200]] {
                for end in [
                    [0.0; 2],
                    [1000.0; 2],
                    [13.5, 69.2],
                    [90.0, 20.0],
                    [30.0, 75.0],
                ] {
                    let start = [viewport[0] / 2.0, viewport[1] / 2.0];
                    if let Some(arrow) = ArrowAnnotation::from_drag_constrained(
                        start, end, viewport, image, 0x22c55e, true,
                    ) {
                        let dx = arrow.start[0].abs_diff(arrow.end[0]);
                        let dy = arrow.start[1].abs_diff(arrow.end[1]);
                        assert!(dx == 0 || dy == 0 || dx == dy);
                        assert!(arrow.start != arrow.end);
                        assert!(arrow.end[0] < image[0] && arrow.end[1] < image[1]);
                        assert_eq!(arrow.color, 0x22c55e);
                    }
                    assert_eq!(
                        ArrowAnnotation::from_drag_constrained(
                            start, end, viewport, image, 0, false
                        ),
                        ArrowAnnotation::from_drag(start, end, viewport, image, 0)
                    );
                }
            }
        }
        assert!(
            ArrowAnnotation::from_drag_constrained(
                [1.0; 2], [1.0; 2], [100.0; 2], [100; 2], 0, true
            )
            .is_none()
        );
        assert!(
            ArrowAnnotation::from_drag_constrained(
                [f32::NAN; 2],
                [2.0; 2],
                [100.0; 2],
                [100; 2],
                0,
                true
            )
            .is_none()
        );
    }
    #[test]
    fn arrow_hit_testing_follows_width_polygon_direction_and_tolerance() {
        for width in [1, 3, 6, 12] {
            for end in [
                [220, 64],
                [20, 64],
                [128, 8],
                [128, 120],
                [200, 112],
                [129, 64],
            ] {
                let arrow = ArrowAnnotation {
                    start: [128, 64],
                    end,
                    color: 0,
                    width,
                };
                let vertices = arrow.vertices();
                assert!(vertices.iter().flatten().all(|v| v.is_finite()));
                assert!(arrow.hit_test(vertices[3], 0.0));
                let inner_head = [
                    (vertices[2][0] + vertices[3][0] + vertices[4][0]) / 3.0,
                    (vertices[2][1] + vertices[3][1] + vertices[4][1]) / 3.0,
                ];
                assert!(arrow.hit_test(inner_head, 0.0));
                assert!(!arrow.hit_test([0.0, 0.0], 1.0));
            }
        }
        let arrow = ArrowAnnotation {
            start: [10, 64],
            end: [220, 64],
            color: 0,
            width: 12,
        };
        assert!(arrow.hit_test([150.0, 85.0], 0.0));
        assert!(!arrow.hit_test([50.0, 85.0], 1.0));
        assert!(!arrow.hit_test([50.0, 72.0], 0.0));
        assert!(arrow.hit_test([50.0, 72.0], 2.0));
        assert!(!arrow.hit_test([f32::NAN, 0.0], 2.0));
        assert!(!arrow.hit_test([50.0, 64.5], -1.0));
    }
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
            width: 3,
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
            width: 3,
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
                    width: 3,
                    end: [100, 16],
                    ..arrow
                }]
            )
            .is_err()
        );
        Ok(())
    }
}
