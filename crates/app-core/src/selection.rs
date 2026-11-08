use std::time::Instant;

use crate::CapturedImage;
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAnnotation {
    pub area: PixelRegion,
    pub color: u32,
}

impl FrameAnnotation {
    pub fn ellipse_contours(self) -> Vec<Vec<[f32; 2]>> {
        let rx = self.area.width as f32 / 2.0;
        let ry = self.area.height as f32 / 2.0;
        let center = [self.area.x as f32 + rx, self.area.y as f32 + ry];
        let steps = ((rx.max(ry).sqrt() * 8.0).ceil() as usize).clamp(32, 256);
        let contour = |x: f32, y: f32, reverse: bool| {
            (0..steps)
                .map(|index| {
                    let index = if reverse { steps - 1 - index } else { index };
                    let angle = index as f32 * std::f32::consts::TAU / steps as f32;
                    [center[0] + angle.cos() * x, center[1] + angle.sin() * y]
                })
                .collect::<Vec<_>>()
        };
        let mut contours = vec![contour(rx, ry, false)];
        if rx > 3.0 && ry > 3.0 {
            contours.push(contour(rx - 3.0, ry - 3.0, true));
        }
        contours
    }
    /// Shared preview/export geometry: a three-source-pixel border inside the rectangle.
    pub fn segments(self) -> [PixelRegion; 4] {
        let area = self.area;
        let horizontal = 3.min(area.height);
        let vertical = 3.min(area.width);
        [
            PixelRegion {
                height: horizontal,
                ..area
            },
            PixelRegion {
                y: area
                    .y
                    .saturating_add(area.height.saturating_sub(horizontal)),
                height: horizontal,
                ..area
            },
            PixelRegion {
                width: vertical,
                ..area
            },
            PixelRegion {
                x: area.x.saturating_add(area.width.saturating_sub(vertical)),
                width: vertical,
                ..area
            },
        ]
    }
}

impl PixelRegion {
    /// Map window-local logical coordinates to the frozen image's physical pixels.
    pub fn from_drag(
        start: [f32; 2],
        end: [f32; 2],
        viewport: [f32; 2],
        image: [u32; 2],
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
        let mut lo = [0; 2];
        let mut hi = [0; 2];
        for axis in 0..2 {
            let a = start[axis].clamp(0.0, viewport[axis]);
            let b = end[axis].clamp(0.0, viewport[axis]);
            if a == b {
                return None;
            }
            let scale = image[axis] as f32 / viewport[axis];
            lo[axis] = (a.min(b) * scale).floor() as u32;
            hi[axis] = ((a.max(b) * scale).ceil() as u32).min(image[axis]);
        }
        Some(Self {
            x: lo[0],
            y: lo[1],
            width: hi[0] - lo[0],
            height: hi[1] - lo[1],
        })
    }
}

/// Raster-only opaque redaction; no original pixels are embedded in the exported PNG.
pub fn redact_snapshot(source: &CapturedImage, regions: &[PixelRegion]) -> Result<CapturedImage> {
    edit_snapshot(source, regions, None)
}

pub fn edit_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
) -> Result<CapturedImage> {
    render_edited_snapshot(source, regions, crop, &[])
}

pub fn render_edited_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
) -> Result<CapturedImage> {
    render_annotated_snapshot(source, regions, crop, frames, &[])
}

pub fn render_annotated_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
    arrows: &[crate::ArrowAnnotation],
) -> Result<CapturedImage> {
    render_document_snapshot(source, regions, crop, frames, arrows, &[])
}

pub fn render_document_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
    arrows: &[crate::ArrowAnnotation],
    ellipses: &[FrameAnnotation],
) -> Result<CapturedImage> {
    render_pixelated_snapshot(source, regions, crop, frames, arrows, ellipses, &[], None)
}

pub fn render_pixelated_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
    arrows: &[crate::ArrowAnnotation],
    ellipses: &[FrameAnnotation],
    pixelations: &[PixelRegion],
    grid: Option<&crate::PixelationGrid>,
) -> Result<CapturedImage> {
    render_filtered_snapshot(
        source,
        regions,
        crop,
        frames,
        arrows,
        ellipses,
        pixelations,
        grid,
        &[],
    )
}

pub fn render_filtered_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
    arrows: &[crate::ArrowAnnotation],
    ellipses: &[FrameAnnotation],
    pixelations: &[PixelRegion],
    grid: Option<&crate::PixelationGrid>,
    blurs: &[PixelRegion],
) -> Result<CapturedImage> {
    render_text_snapshot(
        source,
        regions,
        crop,
        frames,
        arrows,
        ellipses,
        pixelations,
        grid,
        blurs,
        &[],
    )
}

pub fn render_text_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    frames: &[FrameAnnotation],
    arrows: &[crate::ArrowAnnotation],
    ellipses: &[FrameAnnotation],
    pixelations: &[PixelRegion],
    grid: Option<&crate::PixelationGrid>,
    blurs: &[PixelRegion],
    texts: &[crate::TextAnnotation],
) -> Result<CapturedImage> {
    ensure!(
        [frames.len(), arrows.len(), ellipses.len(), texts.len()]
            .iter()
            .all(|count| *count <= 100)
            && frames.len() + arrows.len() + ellipses.len() + texts.len() <= 100,
        "Add up to 100 annotations"
    );
    let annotations = frames
        .iter()
        .copied()
        .map(crate::DrawAnnotation::Frame)
        .chain(ellipses.iter().copied().map(crate::DrawAnnotation::Ellipse))
        .chain(arrows.iter().copied().map(crate::DrawAnnotation::Arrow))
        .chain(texts.iter().cloned().map(crate::DrawAnnotation::Text))
        .collect::<Vec<_>>();
    render_ordered_snapshot(
        source,
        regions,
        crop,
        &annotations,
        pixelations,
        grid,
        blurs,
    )
}

pub fn render_ordered_snapshot(
    source: &CapturedImage,
    regions: &[PixelRegion],
    crop: Option<PixelRegion>,
    annotations: &[crate::DrawAnnotation],
    pixelations: &[PixelRegion],
    grid: Option<&crate::PixelationGrid>,
    blurs: &[PixelRegion],
) -> Result<CapturedImage> {
    ensure!(
        [
            annotations.len(),
            regions.len(),
            pixelations.len(),
            blurs.len()
        ]
        .iter()
        .all(|count| *count <= 100),
        "Add up to 100 annotations"
    );
    let frames = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Frame(frame) = item {
                Some(*frame)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let pens = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Pen(pen) = item {
                Some(pen)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let highlights = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Highlight(highlight) = item {
                Some(*highlight)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let arrows = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Arrow(arrow) = item {
                Some(*arrow)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let ellipses = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Ellipse(ellipse) = item {
                Some(*ellipse)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let texts = annotations
        .iter()
        .filter_map(|item| {
            if let crate::DrawAnnotation::Text(text) = item {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    ensure!(
        (!regions.is_empty()
            || !annotations.is_empty()
            || !pixelations.is_empty()
            || !blurs.is_empty()
            || crop.is_some())
            && regions.len() + annotations.len() + pixelations.len() + blurs.len() <= 100,
        "Choose a crop or add up to 100 annotations"
    );
    let (width, height) = source.pixels.dimensions();
    crate::validate_image_size(width, height)?;
    for pen in &pens {
        pen.validate()?;
    }
    ensure!(
        pens.iter().map(|pen| pen.points.len()).sum::<usize>() <= crate::MAX_DOCUMENT_PEN_POINTS,
        "Pen point budget exceeded"
    );
    for text in &texts {
        text.validate()?;
    }
    ensure!(
        texts.iter().map(|text| text.mask.len()).sum::<usize>() <= 4 * 1024 * 1024,
        "Text masks exceed the export limit"
    );
    ensure!(
        arrows.iter().all(|arrow| arrow.color <= 0xffffff
            && arrow.start != arrow.end
            && arrow.start[0] < width
            && arrow.end[0] < width
            && arrow.start[1] < height
            && arrow.end[1] < height),
        "Invalid arrow annotation"
    );
    ensure!(
        frames
            .iter()
            .chain(ellipses.iter())
            .chain(highlights.iter())
            .all(|frame| frame.color <= 0xffffff),
        "Invalid frame color"
    );
    for region in regions
        .iter()
        .chain(crop.iter())
        .chain(frames.iter().map(|frame| &frame.area))
        .chain(ellipses.iter().map(|frame| &frame.area))
        .chain(highlights.iter().map(|highlight| &highlight.area))
        .chain(pixelations)
        .chain(blurs)
        .chain(texts.iter().map(|text| &text.area))
        .chain(pens.iter().map(|pen| &pen.area))
    {
        ensure!(
            region.width > 0
                && region.height > 0
                && region
                    .x
                    .checked_add(region.width)
                    .is_some_and(|end| end <= width)
                && region
                    .y
                    .checked_add(region.height)
                    .is_some_and(|end| end <= height),
            "Edit area is outside the image"
        );
    }
    let started = Instant::now();
    let area = crop.unwrap_or(PixelRegion {
        x: 0,
        y: 0,
        width,
        height,
    });
    let mut pixels = if crop.is_some() {
        image::imageops::crop_imm(
            source.pixels.as_ref(),
            area.x,
            area.y,
            area.width,
            area.height,
        )
        .to_image()
    } else {
        source.pixels.as_ref().clone()
    };
    if !blurs.is_empty() {
        crate::blur::paint_blur(&source.pixels, &mut pixels, area, blurs)?;
    }
    if !pixelations.is_empty() {
        grid.ok_or_else(|| anyhow::anyhow!("Pixelation preview is not ready"))?
            .paint(&mut pixels, area, pixelations, [width, height])?;
    }
    crate::layers::paint_ordered(&mut pixels, area, annotations)?;
    // Redaction is always the final opaque layer, including where other annotations overlap it.
    for region in regions {
        for y in region.y.max(area.y)..(region.y + region.height).min(area.y + area.height) {
            for x in region.x.max(area.x)..(region.x + region.width).min(area.x + area.width) {
                pixels.put_pixel(x - area.x, y - area.y, image::Rgba([0, 0, 0, 255]));
            }
        }
    }
    Ok(CapturedImage {
        pixels: std::sync::Arc::new(pixels),
        width: area.width,
        height: area.height,
        png_path: source.png_path.with_file_name(format!(
            "{}-{}.png",
            if !regions.is_empty() {
                "redacted"
            } else if !frames.is_empty()
                || !arrows.is_empty()
                || !ellipses.is_empty()
                || !pixelations.is_empty()
                || !blurs.is_empty()
                || !texts.is_empty()
                || !highlights.is_empty()
                || !pens.is_empty()
            {
                "annotated"
            } else {
                "cropped"
            },
            uuid::Uuid::new_v4()
        )),
        elapsed_ms: started.elapsed().as_millis(),
        ..source.clone()
    })
}

pub fn crop_snapshot(source: &CapturedImage, region: PixelRegion) -> Result<CapturedImage> {
    let started = Instant::now();
    let image = &source.pixels;
    crate::capture::validate_image_size(region.width, region.height)?;
    ensure!(
        region.width > 0
            && region.height > 0
            && region
                .x
                .checked_add(region.width)
                .is_some_and(|x| x <= image.width())
            && region
                .y
                .checked_add(region.height)
                .is_some_and(|y| y <= image.height()),
        "selection is outside the snapshot"
    );
    let path = source
        .png_path
        .with_file_name(format!("selection-{}.png", uuid::Uuid::new_v4()));
    let pixels = std::sync::Arc::new(
        image::imageops::crop_imm(
            image.as_ref(),
            region.x,
            region.y,
            region.width,
            region.height,
        )
        .to_image(),
    );
    Ok(CapturedImage {
        png_path: path,
        pixels,
        width: region.width,
        height: region.height,
        elapsed_ms: started.elapsed().as_millis(),
        ..source.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_match_segments_clip_to_crop_and_stay_below_redaction() -> Result<()> {
        let original = image::RgbaImage::from_pixel(12, 10, image::Rgba([31, 42, 53, 255]));
        let source = CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 12,
            height: 10,
            pixels: std::sync::Arc::new(original.clone()),
            png_path: "source.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let frame = FrameAnnotation {
            area: PixelRegion {
                x: 1,
                y: 1,
                width: 10,
                height: 8,
            },
            color: 0xef4444,
        };
        assert_eq!(frame.segments()[0].height, 3);
        assert_eq!(frame.segments()[2].width, 3);
        let redact = PixelRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let crop = PixelRegion {
            x: 2,
            y: 2,
            width: 9,
            height: 7,
        };
        let result = render_edited_snapshot(&source, &[redact], Some(crop), &[frame])?;
        for y in 0..crop.height {
            for x in 0..crop.width {
                let sx = x + crop.x;
                let sy = y + crop.y;
                let in_frame = frame.segments().iter().any(|area| {
                    sx >= area.x
                        && sx < area.x + area.width
                        && sy >= area.y
                        && sy < area.y + area.height
                });
                let expected = if sx < 3 && sy < 3 {
                    image::Rgba([0, 0, 0, 255])
                } else if in_frame {
                    image::Rgba([239, 68, 68, 255])
                } else {
                    *original.get_pixel(sx, sy)
                };
                assert_eq!(*result.pixels.get_pixel(x, y), expected);
            }
        }
        assert_eq!(*source.pixels, original);
        assert!(
            render_edited_snapshot(
                &source,
                &[],
                None,
                &[FrameAnnotation {
                    color: 0xff000000,
                    ..frame
                }]
            )
            .is_err()
        );
        assert!(render_edited_snapshot(&source, &[], None, &vec![frame; 101]).is_err());
        Ok(())
    }

    #[test]
    fn editor_crop_clips_redactions_and_exports_only_selected_pixels() -> Result<()> {
        let original =
            image::RgbaImage::from_fn(5, 4, |x, y| image::Rgba([x as u8, y as u8, 91, 13]));
        let source = CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 5,
            height: 4,
            pixels: std::sync::Arc::new(original.clone()),
            png_path: "source.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let crop = PixelRegion {
            x: 2,
            y: 1,
            width: 2,
            height: 2,
        };
        let edits = [
            PixelRegion {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
            PixelRegion {
                x: 0,
                y: 3,
                width: 1,
                height: 1,
            },
        ];
        let output = edit_snapshot(&source, &edits, Some(crop))?;
        assert_eq!((output.width, output.height), (2, 2));
        assert_eq!(*output.pixels.get_pixel(0, 0), image::Rgba([0, 0, 0, 255]));
        assert_eq!(output.pixels.get_pixel(1, 0), original.get_pixel(3, 1));
        assert_eq!(output.pixels.get_pixel(0, 1), original.get_pixel(2, 2));
        assert_eq!(*source.pixels, original);
        let plain = edit_snapshot(&source, &[], Some(crop))?;
        assert_eq!(plain.pixels.get_pixel(0, 0), original.get_pixel(2, 1));
        assert!(
            edit_snapshot(
                &source,
                &[],
                Some(PixelRegion {
                    x: 4,
                    width: 2,
                    ..crop
                })
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn redaction_export_is_opaque_and_keeps_original_unchanged() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-redaction-{}", uuid::Uuid::new_v4()));
        let original = image::RgbaImage::from_pixel(4, 3, image::Rgba([91, 123, 200, 17]));
        let source = CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 4,
            height: 3,
            pixels: std::sync::Arc::new(original.clone()),
            png_path: root.join("original.png"),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        source.save_png()?;
        let original_file = std::fs::read(&source.png_path)?;
        let redacted = redact_snapshot(
            &source,
            &[PixelRegion {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            }],
        )?;
        redacted.save_png()?;
        let exported = image::open(&redacted.png_path)?.into_rgba8();
        for y in 0..3 {
            for x in 0..4 {
                let expected = if x >= 1 && x < 3 && y >= 1 {
                    image::Rgba([0, 0, 0, 255])
                } else {
                    *original.get_pixel(x, y)
                };
                assert_eq!(*exported.get_pixel(x, y), expected);
            }
        }
        assert_eq!(*source.pixels, original);
        assert_eq!(std::fs::read(&source.png_path)?, original_file);
        assert!(
            redact_snapshot(
                &source,
                &[PixelRegion {
                    x: u32::MAX,
                    y: 0,
                    width: 2,
                    height: 1
                }]
            )
            .is_err()
        );
        assert!(redact_snapshot(&source, &[]).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn drag_handles_dpi_reverse_direction_and_edges() {
        assert_eq!(
            PixelRegion::from_drag([40.2, 30.2], [10.1, 5.1], [100.0, 50.0], [150, 100]),
            Some(PixelRegion {
                x: 15,
                y: 10,
                width: 46,
                height: 51
            })
        );
        assert_eq!(
            PixelRegion::from_drag([-20.0, -10.0], [200.0, 70.0], [100.0, 50.0], [150, 100]),
            Some(PixelRegion {
                x: 0,
                y: 0,
                width: 150,
                height: 100
            })
        );
        assert!(
            PixelRegion::from_drag([5.0, 5.0], [5.0, 8.0], [100.0, 50.0], [150, 100]).is_none()
        );
        assert!(
            PixelRegion::from_drag([f32::NAN, 5.0], [7.0, 8.0], [100.0, 50.0], [150, 100])
                .is_none()
        );
    }

    #[test]
    fn crop_uses_frozen_pixels_and_preserves_source() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("selection-test-{}.png", uuid::Uuid::new_v4()));
        let image =
            image::RgbaImage::from_fn(4, 3, |x, y| image::Rgba([x as u8, y as u8, 20, 255]));
        image.save(&path)?;
        let source = CapturedImage {
            window_id: None,
            display_id: "test".into(),
            width: 4,
            height: 3,
            png_path: path.clone(),
            pixels: std::sync::Arc::new(image.clone()),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        };
        let result = crop_snapshot(
            &source,
            PixelRegion {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        )?;
        assert!(std::sync::Arc::ptr_eq(
            &source.pixels,
            &source.clone().pixels
        ));
        assert!(!result.png_path.exists());
        let cropped = &result.pixels;
        assert_eq!(cropped.dimensions(), (2, 2));
        assert_eq!(cropped.get_pixel(0, 0), image.get_pixel(1, 1));
        assert_eq!(image::open(&path)?.into_rgba8(), image);
        assert!(
            crop_snapshot(
                &source,
                PixelRegion {
                    x: 3,
                    y: 2,
                    width: 2,
                    height: 2
                }
            )
            .is_err()
        );
        std::fs::remove_file(path)?;

        Ok(())
    }
}
