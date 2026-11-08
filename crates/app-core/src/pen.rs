use crate::PixelRegion;
use anyhow::{Result, ensure};
use std::sync::Arc;

pub const MAX_PEN_POINTS: usize = 1024;
pub const MAX_DOCUMENT_PEN_POINTS: usize = 8192;

#[derive(Clone)]
pub struct PenAnnotation {
    pub area: PixelRegion,
    pub original_size: [u32; 2],
    pub points: Arc<[[u32; 2]]>,
    pub color: u32,
}
impl PenAnnotation {
    pub fn from_trace(trace: &[[u32; 2]], color: u32) -> Option<Self> {
        if trace.is_empty()
            || trace.len() > MAX_PEN_POINTS
            || color > 0xffffff
            || trace.windows(2).any(|pair| pair[0] == pair[1])
        {
            return None;
        }
        let min = [
            trace.iter().map(|p| p[0]).min()?,
            trace.iter().map(|p| p[1]).min()?,
        ];
        let size = [
            trace
                .iter()
                .map(|p| p[0])
                .max()?
                .checked_sub(min[0])?
                .checked_add(1)?,
            trace
                .iter()
                .map(|p| p[1])
                .max()?
                .checked_sub(min[1])?
                .checked_add(1)?,
        ];
        Some(Self {
            area: PixelRegion {
                x: min[0],
                y: min[1],
                width: size[0],
                height: size[1],
            },
            original_size: size,
            points: trace
                .iter()
                .map(|p| [p[0] - min[0], p[1] - min[1]])
                .collect::<Vec<_>>()
                .into(),
            color,
        })
    }
    /// Shared source-pixel centerline. Resizing changes geometry, with a fixed three-pixel ink width.
    pub fn positions(&self) -> impl Iterator<Item = [f32; 2]> + '_ {
        self.points.iter().map(|point| {
            [
                self.area.x as f32
                    + (point[0] as f32 + 0.5) * self.area.width as f32
                        / self.original_size[0] as f32,
                self.area.y as f32
                    + (point[1] as f32 + 0.5) * self.area.height as f32
                        / self.original_size[1] as f32,
            ]
        })
    }
    pub fn distance(&self, point: [f32; 2]) -> f32 {
        let mut positions = self.positions();
        let Some(mut previous) = positions.next() else {
            return f32::INFINITY;
        };
        let mut distance = (point[0] - previous[0]).hypot(point[1] - previous[1]);
        for next in positions {
            let delta = [next[0] - previous[0], next[1] - previous[1]];
            let length = delta[0] * delta[0] + delta[1] * delta[1];
            let ratio = if length > 0.0 {
                (((point[0] - previous[0]) * delta[0] + (point[1] - previous[1]) * delta[1])
                    / length)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            distance = distance.min(
                (point[0] - previous[0] - ratio * delta[0])
                    .hypot(point[1] - previous[1] - ratio * delta[1]),
            );
            previous = next;
        }
        distance
    }
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.color <= 0xffffff
                && !self.points.is_empty()
                && self.points.len() <= MAX_PEN_POINTS
                && !self.original_size.contains(&0)
                && self
                    .points
                    .iter()
                    .all(|p| p[0] < self.original_size[0] && p[1] < self.original_size[1])
                && self.points.windows(2).all(|pair| pair[0] != pair[1]),
            "Invalid pen stroke"
        );
        Ok(())
    }
    pub(crate) fn paint(&self, layer: &mut tiny_skia::Pixmap, crop: PixelRegion) -> Result<()> {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(
            (self.color >> 16) as u8,
            (self.color >> 8) as u8,
            self.color as u8,
            255,
        );
        paint.anti_alias = true;
        let mut positions = self.positions();
        let first = positions.next().unwrap();
        if self.points.len() == 1 {
            let rect = tiny_skia::Rect::from_xywh(
                first[0] - crop.x as f32 - 1.5,
                first[1] - crop.y as f32 - 1.5,
                3.0,
                3.0,
            )
            .unwrap();
            layer.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
        } else {
            let mut path = tiny_skia::PathBuilder::new();
            path.move_to(first[0] - crop.x as f32, first[1] - crop.y as f32);
            for point in positions {
                path.line_to(point[0] - crop.x as f32, point[1] - crop.y as f32);
            }
            let path = path
                .finish()
                .ok_or_else(|| anyhow::anyhow!("Invalid pen path"))?;
            let stroke = tiny_skia::Stroke {
                width: 3.0,
                miter_limit: 1.0,
                ..Default::default()
            };
            layer.stroke_path(
                &path,
                &paint,
                &stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(width: u32, height: u32) -> crate::CapturedImage {
        crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width,
            height,
            pixels: Arc::new(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([19, 37, 61, 17]),
            )),
            png_path: "not-written.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        }
    }
    #[test]
    fn pen_geometry_export_crop_dot_and_redaction_keep_original_intact() -> Result<()> {
        let source = source(64, 48);
        let pen = PenAnnotation::from_trace(&[[8, 16], [24, 16], [40, 32]], 0xef4444).unwrap();
        assert_eq!(pen.positions().next(), Some([8.5, 16.5]));
        assert!(pen.distance([16.0, 16.5]) < 0.01);
        assert!(pen.distance([16.0, 30.0]) > 5.0);
        let draw = crate::DrawAnnotation::Pen(pen.clone());
        let output = crate::render_ordered_snapshot(
            &source,
            &[],
            None,
            std::slice::from_ref(&draw),
            &[],
            None,
            &[],
        )?;
        assert_eq!(output.pixels.get_pixel(16, 16).0, [239, 68, 68, 255]);
        assert_eq!(output.pixels.get_pixel(0, 0), source.pixels.get_pixel(0, 0));
        let crop = PixelRegion {
            x: 12,
            y: 12,
            width: 32,
            height: 24,
        };
        let cropped = crate::render_ordered_snapshot(
            &source,
            &[],
            Some(crop),
            std::slice::from_ref(&draw),
            &[],
            None,
            &[],
        )?;
        assert_eq!(
            cropped.pixels.as_ref(),
            &image::imageops::crop_imm(
                output.pixels.as_ref(),
                crop.x,
                crop.y,
                crop.width,
                crop.height
            )
            .to_image()
        );
        let redacted =
            crate::render_ordered_snapshot(&source, &[crop], None, &[draw], &[], None, &[])?;
        assert_eq!(redacted.pixels.get_pixel(16, 16).0, [0, 0, 0, 255]);
        let dot = PenAnnotation::from_trace(&[[0, 0]], 0x22c55e).unwrap();
        let dotted = crate::render_ordered_snapshot(
            &source,
            &[],
            None,
            &[crate::DrawAnnotation::Pen(dot)],
            &[],
            None,
            &[],
        )?;
        assert_eq!(dotted.pixels.get_pixel(0, 0).0, [34, 197, 94, 255]);
        assert_eq!(source.pixels.get_pixel(16, 16).0, [19, 37, 61, 17]);
        assert!(PenAnnotation::from_trace(&[], 0).is_none());
        assert!(PenAnnotation::from_trace(&[[1, 1], [1, 1]], 0).is_none());
        assert!(PenAnnotation::from_trace(&[[1, 1]], 0x1000000).is_none());
        let oversized = (0..1025).map(|i| [i, 0]).collect::<Vec<_>>();
        assert!(PenAnnotation::from_trace(&oversized, 0).is_none());
        let trace = (0..1024).map(|i| [i, 0]).collect::<Vec<_>>();
        let mut large = PenAnnotation::from_trace(&trace, 0).unwrap();
        large.area = PixelRegion {
            x: 0,
            y: 0,
            width: 64,
            height: 1,
        };
        assert!(
            crate::render_ordered_snapshot(
                &source,
                &[],
                None,
                &vec![crate::DrawAnnotation::Pen(large); 9],
                &[],
                None,
                &[]
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    #[ignore = "short optimized 4K pen comparison; no desktop, files or clipboard"]
    fn pen_4k_cost() -> Result<()> {
        let source = source(3840, 2160);
        let trace = (0..1024)
            .map(|i| [i * 2, 100 + (i % 2) * 3])
            .collect::<Vec<_>>();
        let pen = PenAnnotation::from_trace(&trace, 0xef4444).unwrap();
        for count in [1, 8] {
            let draws = vec![crate::DrawAnnotation::Pen(pen.clone()); count];
            let mut samples = Vec::new();
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let output =
                    crate::render_ordered_snapshot(&source, &[], None, &draws, &[], None, &[])?;
                std::hint::black_box(output);
                samples.push(start.elapsed());
            }
            samples.sort();
            eprintln!(
                "4K pen: {} points, export median={:?}; shared point assets, one reused vector layer",
                count * 1024,
                samples[1]
            );
        }
        Ok(())
    }
}
