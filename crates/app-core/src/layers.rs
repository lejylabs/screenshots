use crate::{ArrowAnnotation, FrameAnnotation, PixelRegion, TextAnnotation};
use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightAnnotation {
    pub area: PixelRegion,
    pub color: u32,
    pub opacity: u8,
}

#[derive(Clone)]
pub enum DrawAnnotation {
    Pen(crate::PenAnnotation),
    Highlight(HighlightAnnotation),
    Frame(FrameAnnotation),
    Arrow(ArrowAnnotation),
    Ellipse(FrameAnnotation),
    Text(TextAnnotation),
}

fn raster_area(crop: PixelRegion, source_size: [u32; 2]) -> Result<PixelRegion> {
    let raster = PixelRegion {
        x: crop.x.saturating_sub(1),
        y: crop.y.saturating_sub(1),
        width: (crop.x + crop.width + 1).min(source_size[0]) - crop.x.saturating_sub(1),
        height: (crop.y + crop.height + 1).min(source_size[1]) - crop.y.saturating_sub(1),
    };
    let extra_bytes = (u64::from(raster.width) * u64::from(raster.height)
        - u64::from(crop.width) * u64::from(crop.height))
        * 4;
    ensure!(
        extra_bytes <= 4 * 1024 * 1024,
        "This crop is too long and narrow for bounded vector rendering. Choose a wider or shorter crop."
    );
    Ok(raster)
}

// Flush only the region a direct frame/text operation will touch, then reuse the vector layer.
fn flush(
    layer: &mut tiny_skia::Pixmap,
    pixels: &mut image::RgbaImage,
    crop: PixelRegion,
    raster: PixelRegion,
    area: PixelRegion,
) {
    for y in area.y.max(crop.y)..(area.y + area.height).min(crop.y + crop.height) {
        for x in area.x.max(crop.x)..(area.x + area.width).min(crop.x + crop.width) {
            let index = (((y - raster.y) * raster.width + x - raster.x) * 4) as usize;
            let overlay = &mut layer.data_mut()[index..index + 4];
            let alpha = u32::from(overlay[3]);
            if alpha == 0 {
                continue;
            }
            let destination = pixels.get_pixel_mut(x - crop.x, y - crop.y);
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
            overlay.fill(0);
        }
    }
}

pub(crate) fn paint_ordered(
    pixels: &mut image::RgbaImage,
    crop: PixelRegion,
    annotations: &[DrawAnnotation],
    source_size: [u32; 2],
) -> Result<()> {
    // Keep crop pixels away from rasterizer clipping edges without a second vector buffer.
    let raster = if annotations.iter().any(|item| {
        matches!(
            item,
            DrawAnnotation::Arrow(_) | DrawAnnotation::Ellipse(_) | DrawAnnotation::Pen(_)
        )
    }) {
        raster_area(crop, source_size)?
    } else {
        crop
    };
    let mut layer: Option<tiny_skia::Pixmap> = None;
    for annotation in annotations {
        match annotation {
            DrawAnnotation::Highlight(highlight) => {
                let area = highlight.area;
                if let Some(layer) = &mut layer {
                    flush(layer, pixels, crop, raster, area);
                }
                for y in area.y.max(crop.y)..(area.y + area.height).min(crop.y + crop.height) {
                    for x in area.x.max(crop.x)..(area.x + area.width).min(crop.x + crop.width) {
                        crate::text::blend_ink(
                            pixels.get_pixel_mut(x - crop.x, y - crop.y),
                            highlight.color,
                            u32::from(highlight.opacity),
                        );
                    }
                }
            }
            DrawAnnotation::Arrow(_) | DrawAnnotation::Ellipse(_) | DrawAnnotation::Pen(_) => {
                if layer.is_none() {
                    layer = Some(
                        tiny_skia::Pixmap::new(raster.width, raster.height)
                            .context("Could not allocate vector raster layer")?,
                    );
                }
                let layer = layer.as_mut().unwrap();
                match annotation {
                    DrawAnnotation::Pen(pen) => pen.paint(layer, raster)?,
                    DrawAnnotation::Arrow(arrow) => crate::arrows::paint_shapes(
                        layer,
                        raster,
                        std::slice::from_ref(arrow),
                        &[],
                    )?,
                    DrawAnnotation::Ellipse(ellipse) => crate::arrows::paint_shapes(
                        layer,
                        raster,
                        &[],
                        std::slice::from_ref(ellipse),
                    )?,
                    _ => unreachable!(),
                }
            }
            DrawAnnotation::Frame(frame) => {
                let color = image::Rgba([
                    (frame.color >> 16) as u8,
                    (frame.color >> 8) as u8,
                    frame.color as u8,
                    255,
                ]);
                for area in frame.segments() {
                    if let Some(layer) = &mut layer {
                        flush(layer, pixels, crop, raster, area);
                    }
                    for y in area.y.max(crop.y)..(area.y + area.height).min(crop.y + crop.height) {
                        for x in area.x.max(crop.x)..(area.x + area.width).min(crop.x + crop.width)
                        {
                            pixels.put_pixel(x - crop.x, y - crop.y, color);
                        }
                    }
                }
            }
            DrawAnnotation::Text(text) => {
                if let Some(layer) = &mut layer {
                    flush(layer, pixels, crop, raster, text.area);
                }
                text.paint(pixels, crop);
            }
        }
    }
    if let Some(layer) = &mut layer {
        flush(layer, pixels, crop, raster, crop);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_crop_padding_stays_inside_source_and_bounds_skinny_image_cost() -> Result<()> {
        let full = PixelRegion {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert_eq!(raster_area(full, [3840, 2160])?, full);
        let inset = PixelRegion {
            x: 1,
            y: 1,
            width: 3838,
            height: 2158,
        };
        assert_eq!(raster_area(inset, [3840, 2160])?, full);
        let corner = PixelRegion {
            x: 3839,
            y: 2159,
            width: 1,
            height: 1,
        };
        assert_eq!(
            raster_area(corner, [3840, 2160])?,
            PixelRegion {
                x: 3838,
                y: 2158,
                width: 2,
                height: 2
            }
        );
        crate::validate_image_size(3, 22_000_000)?;
        let skinny = PixelRegion {
            x: 1,
            y: 0,
            width: 1,
            height: 22_000_000,
        };
        assert!(raster_area(skinny, [3, 22_000_000]).is_err());
        let full_skinny = PixelRegion {
            x: 0,
            y: 0,
            width: 1,
            height: 67_000_000,
        };
        assert_eq!(raster_area(full_skinny, [1, 67_000_000])?, full_skinny);
        Ok(())
    }
    #[test]
    fn shape_widths_preserve_crop_alpha_and_redaction_and_reject_invalid_values() -> Result<()> {
        let source = source(256, 128);
        let area = PixelRegion {
            x: 16,
            y: 16,
            width: 192,
            height: 96,
        };
        let crop = PixelRegion {
            x: 24,
            y: 24,
            width: 176,
            height: 80,
        };
        for width in [1, 3, 6, 12] {
            let shape = FrameAnnotation {
                area,
                color: 0xef4444,
                width,
            };
            assert_eq!(shape.segments()[0].height, u32::from(width));
            let tiny = FrameAnnotation {
                area: PixelRegion {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                ..shape
            };
            assert!(
                tiny.segments()
                    .iter()
                    .all(|segment| segment.width == 1 && segment.height == 1)
            );
            if width >= 6 {
                let small = FrameAnnotation {
                    area: PixelRegion {
                        x: 0,
                        y: 0,
                        width: 16,
                        height: 8,
                    },
                    ..shape
                };
                assert_eq!(small.ellipse_contours().len(), 1);
            }
            let arrow = ArrowAnnotation {
                start: [16, 64],
                end: [224, 64],
                color: 0xef4444,
                width,
            };
            assert!((arrow.vertices()[0][1] - 64.5 - f32::from(width) / 2.0).abs() < 0.01);
            for (kind, draw) in [
                DrawAnnotation::Frame(shape),
                DrawAnnotation::Ellipse(shape),
                DrawAnnotation::Arrow(arrow),
            ]
            .into_iter()
            .enumerate()
            {
                let full = crate::render_ordered_snapshot(
                    &source,
                    &[],
                    None,
                    std::slice::from_ref(&draw),
                    &[],
                    None,
                    &[],
                )?;
                for crop in [
                    crop,
                    PixelRegion {
                        x: 58,
                        y: 24,
                        width: 1,
                        height: 1,
                    },
                    PixelRegion {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 128,
                    },
                    PixelRegion {
                        x: 255,
                        y: 0,
                        width: 1,
                        height: 128,
                    },
                ] {
                    let clipped = crate::render_ordered_snapshot(
                        &source,
                        &[],
                        Some(crop),
                        std::slice::from_ref(&draw),
                        &[],
                        None,
                        &[],
                    )?;
                    for (x, y, pixel) in clipped.pixels.enumerate_pixels() {
                        assert_eq!(
                            pixel,
                            full.pixels.get_pixel(x + crop.x, y + crop.y),
                            "width={width}, kind={kind}, at={x},{y}"
                        );
                    }
                }
                assert_eq!(full.pixels.get_pixel(0, 0), source.pixels.get_pixel(0, 0));
                assert!(
                    full.pixels
                        .pixels()
                        .any(|pixel| pixel.0 == [239, 68, 68, 255])
                );
                let redacted = crate::render_ordered_snapshot(
                    &source,
                    &[area],
                    None,
                    &[draw],
                    &[],
                    None,
                    &[],
                )?;
                assert_eq!(redacted.pixels.get_pixel(16, 64).0, [0, 0, 0, 255]);
            }
        }
        for width in [0, 13, 255] {
            let shape = FrameAnnotation {
                area,
                color: 0,
                width,
            };
            let arrow = ArrowAnnotation {
                start: [16, 64],
                end: [224, 64],
                color: 0,
                width,
            };
            for draw in [
                DrawAnnotation::Frame(shape),
                DrawAnnotation::Ellipse(shape),
                DrawAnnotation::Arrow(arrow),
            ] {
                assert!(
                    crate::render_ordered_snapshot(&source, &[], None, &[draw], &[], None, &[])
                        .is_err()
                );
            }
        }
        Ok(())
    }
    #[test]
    fn highlight_opacity_matches_straight_alpha_and_rejects_invisible_marks() -> Result<()> {
        let mut source = source(8, 8);
        let area = PixelRegion {
            x: 1,
            y: 1,
            width: 4,
            height: 4,
        };
        for background in [[255, 255, 255, 255], [19, 37, 61, 0], [19, 37, 61, 128]] {
            source.pixels =
                std::sync::Arc::new(image::RgbaImage::from_pixel(8, 8, image::Rgba(background)));
            for opacity in [64, 85, 128, 191, 255] {
                let highlight = DrawAnnotation::Highlight(HighlightAnnotation {
                    area,
                    color: 0xfacc15,
                    opacity,
                });
                let output = crate::render_ordered_snapshot(
                    &source,
                    &[],
                    Some(area),
                    &[highlight],
                    &[],
                    None,
                    &[],
                )?;
                let mut expected = image::Rgba(background);
                crate::text::blend_ink(&mut expected, 0xfacc15, u32::from(opacity));
                assert!(output.pixels.pixels().all(|pixel| *pixel == expected));
                if opacity == 255 {
                    assert_eq!(expected.0, [250, 204, 21, 255]);
                }
            }
        }
        let invisible = DrawAnnotation::Highlight(HighlightAnnotation {
            area,
            color: 0xfacc15,
            opacity: 0,
        });
        assert!(
            crate::render_ordered_snapshot(&source, &[], None, &[invisible], &[], None, &[])
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn highlights_blend_clip_follow_order_and_stay_below_redaction() -> Result<()> {
        let mut source = source(48, 32);
        source.pixels = std::sync::Arc::new(image::RgbaImage::from_pixel(
            48,
            32,
            image::Rgba([255, 255, 255, 255]),
        ));
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 24,
            height: 16,
        };
        let highlight = DrawAnnotation::Highlight(HighlightAnnotation {
            opacity: 85,
            area,
            color: 0xfacc15,
        });
        let render = |items: &[DrawAnnotation], crop| {
            crate::render_ordered_snapshot(&source, &[], crop, items, &[], None, &[])
        };
        let output = render(std::slice::from_ref(&highlight), None)?;
        assert_eq!(output.pixels.get_pixel(12, 12).0, [253, 238, 177, 255]);
        assert_eq!(output.pixels.get_pixel(0, 0).0, [255, 255, 255, 255]);
        let frame = DrawAnnotation::Frame(FrameAnnotation {
            width: 3,
            area,
            color: 0xef4444,
        });
        let tinted = render(&[frame.clone(), highlight.clone()], None)?;
        assert_eq!(tinted.pixels.get_pixel(10, 16).0, [243, 113, 52, 255]);
        let opaque = render(&[highlight.clone(), frame], None)?;
        assert_eq!(opaque.pixels.get_pixel(10, 16).0, [239, 68, 68, 255]);
        let crop = PixelRegion {
            x: 12,
            y: 12,
            width: 8,
            height: 8,
        };
        let cropped = render(std::slice::from_ref(&highlight), Some(crop))?;
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
        let redacted = crate::render_ordered_snapshot(
            &source,
            &[area],
            None,
            std::slice::from_ref(&highlight),
            &[],
            None,
            &[],
        )?;
        assert_eq!(redacted.pixels.get_pixel(12, 12).0, [0, 0, 0, 255]);
        let invalid = DrawAnnotation::Highlight(HighlightAnnotation {
            opacity: 85,
            area,
            color: 0x1000000,
        });
        assert!(render(&[invalid], None).is_err());
        let invalid = DrawAnnotation::Highlight(HighlightAnnotation {
            opacity: 85,
            area: PixelRegion {
                x: 47,
                y: 0,
                width: 2,
                height: 1,
            },
            color: 0xfacc15,
        });
        assert!(render(&[invalid], None).is_err());
        let mut transparent = self::source(48, 32);
        transparent.pixels = std::sync::Arc::new(image::RgbaImage::from_pixel(
            48,
            32,
            image::Rgba([19, 37, 61, 0]),
        ));
        let tinted = crate::render_ordered_snapshot(
            &transparent,
            &[],
            None,
            std::slice::from_ref(&highlight),
            &[],
            None,
            &[],
        )?;
        assert_eq!(
            tinted.pixels.get_pixel(0, 0),
            transparent.pixels.get_pixel(0, 0)
        );
        assert_eq!(tinted.pixels.get_pixel(12, 12).0, [250, 204, 21, 85]);
        let ellipse = DrawAnnotation::Ellipse(FrameAnnotation {
            width: 3,
            area,
            color: 0x22c55e,
        });
        let first = render(std::slice::from_ref(&ellipse), None)?;
        let reference = crate::render_ordered_snapshot(
            &first,
            &[],
            None,
            std::slice::from_ref(&highlight),
            &[],
            None,
            &[],
        )?;
        let combined = render(&[ellipse, highlight], None)?;
        assert_eq!(combined.pixels, reference.pixels);
        Ok(())
    }
    fn source(width: u32, height: u32) -> crate::CapturedImage {
        crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width,
            height,
            pixels: std::sync::Arc::new(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([17, 31, 73, 19]),
            )),
            png_path: "not-written.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        }
    }
    #[test]
    fn ordered_frames_arrows_text_and_redaction_match_crop_and_alpha() -> Result<()> {
        let source = source(48, 32);
        let area = PixelRegion {
            x: 10,
            y: 10,
            width: 24,
            height: 16,
        };
        let frame = DrawAnnotation::Frame(FrameAnnotation {
            width: 3,
            area,
            color: 0x22c55e,
        });
        let arrow = DrawAnnotation::Arrow(ArrowAnnotation {
            width: 3,
            start: [2, 16],
            end: [44, 16],
            color: 0xef4444,
        });
        let text = DrawAnnotation::Text(TextAnnotation {
            area: PixelRegion {
                x: 10,
                y: 15,
                width: 2,
                height: 2,
            },
            color: 0xfacc15,
            width: 2,
            height: 2,
            mask: vec![128, 255, 255, 128].into(),
        });
        let render = |items: &[DrawAnnotation], crop| {
            crate::render_ordered_snapshot(&source, &[], crop, items, &[], None, &[])
        };
        let above = render(&[frame.clone(), arrow.clone()], None)?;
        let below = render(&[arrow.clone(), frame.clone()], None)?;
        assert_eq!(above.pixels.get_pixel(10, 16).0, [239, 68, 68, 255]);
        assert_eq!(below.pixels.get_pixel(10, 16).0, [34, 197, 94, 255]);
        let order = vec![arrow.clone(), text.clone(), frame.clone()];
        let output = render(&order, None)?;
        assert_eq!(output.pixels.get_pixel(10, 16).0, [34, 197, 94, 255]);
        let text_above = render(&[arrow.clone(), frame.clone(), text.clone()], None)?;
        assert_eq!(text_above.pixels.get_pixel(10, 16).0, [250, 204, 21, 255]);
        let crop = PixelRegion {
            x: 8,
            y: 12,
            width: 24,
            height: 12,
        };
        let cropped = render(&order, Some(crop))?;
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
        assert_eq!(output.pixels.get_pixel(0, 0), source.pixels.get_pixel(0, 0));
        let redacted =
            crate::render_ordered_snapshot(&source, &[area], None, &order, &[], None, &[])?;
        assert_eq!(redacted.pixels.get_pixel(10, 16).0, [0, 0, 0, 255]);
        assert!(render(&vec![frame; 101], None).is_err());
        Ok(())
    }
    #[test]
    fn partial_flush_preserves_vectors_outside_later_text() -> Result<()> {
        let source = source(48, 40);
        let ellipse = DrawAnnotation::Ellipse(FrameAnnotation {
            width: 3,
            area: PixelRegion {
                x: 4,
                y: 4,
                width: 36,
                height: 30,
            },
            color: 0x22c55e,
        });
        let text = DrawAnnotation::Text(TextAnnotation {
            area: PixelRegion {
                x: 5,
                y: 16,
                width: 2,
                height: 4,
            },
            color: 0xff8000,
            width: 2,
            height: 4,
            mask: vec![128; 8].into(),
        });
        let first = crate::render_ordered_snapshot(
            &source,
            &[],
            None,
            std::slice::from_ref(&ellipse),
            &[],
            None,
            &[],
        )?;
        let reference = crate::render_ordered_snapshot(
            &first,
            &[],
            None,
            std::slice::from_ref(&text),
            &[],
            None,
            &[],
        )?;
        let ordered =
            crate::render_ordered_snapshot(&source, &[], None, &[ellipse, text], &[], None, &[])?;
        assert_eq!(ordered.pixels, reference.pixels);
        Ok(())
    }
    #[test]
    #[ignore = "short optimized 4K highlight comparison; no desktop, files or clipboard"]
    fn highlight_4k_cost() -> Result<()> {
        let mut source = source(3840, 2160);
        for pixel in std::sync::Arc::make_mut(&mut source.pixels).pixels_mut() {
            pixel[3] = 255;
        }
        let area = PixelRegion {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        for (name, opacity) in [
            ("copy", 0),
            ("highlight-33", 85),
            ("highlight-50", 128),
            ("highlight-100", 255),
        ] {
            let annotations = if opacity == 0 {
                Vec::new()
            } else {
                vec![DrawAnnotation::Highlight(HighlightAnnotation {
                    opacity,
                    area,
                    color: 0xfacc15,
                })]
            };
            let mut samples = Vec::new();
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let output = crate::render_ordered_snapshot(
                    &source,
                    &[],
                    Some(area),
                    &annotations,
                    &[],
                    None,
                    &[],
                )?;
                std::hint::black_box(output);
                samples.push(start.elapsed());
            }
            samples.sort();
            eprintln!(
                "4K {name} export median={:?}; highlight adds no image-sized scratch",
                samples[1]
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "short optimized ordered export timing; no desktop, files or clipboard"]
    fn ordered_export_4k_cost() -> Result<()> {
        let source = source(3840, 2160);
        let mut annotations = Vec::new();
        for index in 0..50 {
            annotations.push(DrawAnnotation::Ellipse(FrameAnnotation {
                width: 3,
                area: PixelRegion {
                    x: index * 8,
                    y: index * 8,
                    width: 512,
                    height: 256,
                },
                color: 0x22c55e,
            }));
            annotations.push(DrawAnnotation::Frame(FrameAnnotation {
                width: 3,
                area: PixelRegion {
                    x: index * 8,
                    y: index * 8,
                    width: 256,
                    height: 128,
                },
                color: 0xef4444,
            }));
        }
        let grouped = annotations
            .iter()
            .filter(|item| matches!(item, DrawAnnotation::Frame(_)))
            .chain(
                annotations
                    .iter()
                    .filter(|item| matches!(item, DrawAnnotation::Ellipse(_))),
            )
            .cloned()
            .collect::<Vec<_>>();
        let wide = annotations
            .iter()
            .cloned()
            .map(|mut item| {
                match &mut item {
                    DrawAnnotation::Frame(shape) | DrawAnnotation::Ellipse(shape) => {
                        shape.width = 12
                    }
                    _ => unreachable!(),
                }
                item
            })
            .collect::<Vec<_>>();
        for (name, items) in [
            ("grouped-3px", &grouped),
            ("interleaved-3px", &annotations),
            ("interleaved-12px", &wide),
        ] {
            let mut samples = Vec::new();
            for _ in 0..3 {
                let start = std::time::Instant::now();
                let output =
                    crate::render_ordered_snapshot(&source, &[], None, items, &[], None, &[])?;
                std::hint::black_box(output);
                samples.push(start.elapsed());
            }
            samples.sort();
            eprintln!(
                "4K/100 {name} layers: export median={:?}, one reused vector layer={} bytes",
                samples[1],
                3840 * 2160 * 4
            );
        }
        Ok(())
    }
}
