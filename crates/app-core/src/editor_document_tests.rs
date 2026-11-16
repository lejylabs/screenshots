use super::*;
use image::Rgba;

fn source(width: u32, height: u32) -> CapturedImage {
    let pixels = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x * 17 % 256) as u8,
            (y * 29 % 256) as u8,
            61,
            if x == 0 { 0 } else { 255 },
        ])
    });
    CapturedImage {
        display_id: "editor-fixture".into(),
        window_id: None,
        width,
        height,
        pixels: Arc::new(pixels),
        png_path: "not-written.png".into(),
        elapsed_ms: 0,
        source_format: crate::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}
fn layer(id: u64, bounds: Rect, content: LayerKind) -> Layer {
    Layer {
        id,
        bounds,
        transform: Affine::IDENTITY,
        opacity: 255,
        content,
    }
}
fn fill(id: u64, bounds: Rect, color: [u8; 4]) -> Layer {
    layer(
        id,
        bounds,
        LayerKind::Draw {
            shape: Shape::Rectangle,
            style: Style {
                fill: Some(color),
                stroke: None,
                width: 0.0,
            },
        },
    )
}

#[test]
fn identity_preserves_exact_alpha_hidden_rgb_and_original_backing() -> Result<()> {
    let source = source(9, 7);
    let original = source.pixels.clone();
    let document = EditorDocument::from_image(source)?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.as_ref(), original.as_ref());
    assert_ne!(Arc::as_ptr(&output.pixels), Arc::as_ptr(&original));
    assert_eq!(document.memory().retained_raster_bytes, 9 * 7 * 4);
    Ok(())
}
#[test]
fn effect_order_is_real_and_translucent_vector_ink_retains_selected_rgba() -> Result<()> {
    let mut input = source(8, 4);
    input.pixels = Arc::new(RgbaImage::from_pixel(8, 4, Rgba([0, 0, 255, 255])));
    let mut document = EditorDocument::from_image(input)?;
    document.insert(fill(1, Rect::new(0.0, 0.0, 2.0, 4.0), [255, 0, 0, 255]))?;
    document.insert(layer(
        2,
        Rect::new(0.0, 0.0, 8.0, 4.0),
        LayerKind::Pixelate {
            block: 4,
            input: EffectInput::Previous,
        },
    ))?;
    assert_eq!(
        document
            .snapshot()
            .render(|| false)?
            .pixels
            .get_pixel(3, 1)
            .0,
        [127, 0, 127, 255]
    );
    document.reorder(2, 0)?;
    assert_eq!(
        document
            .snapshot()
            .render(|| false)?
            .pixels
            .get_pixel(3, 1)
            .0,
        [0, 0, 255, 255]
    );
    let mut input = source(8, 4);
    input.pixels = Arc::new(RgbaImage::from_pixel(8, 4, Rgba([217, 13, 131, 0])));
    let mut document = EditorDocument::from_image(input)?;
    document.insert(fill(1, Rect::new(1.0, 1.0, 5.0, 2.0), [19, 37, 61, 17]))?;
    assert_eq!(
        document
            .snapshot()
            .render(|| false)?
            .pixels
            .get_pixel(3, 1)
            .0,
        [19, 37, 61, 17]
    );
    Ok(())
}
#[test]
fn clone_samples_sanitized_stage_even_after_drawing_over_an_active_mask() -> Result<()> {
    let mut input = source(8, 4);
    input.pixels = Arc::new(RgbaImage::from_fn(8, 4, |x, _| {
        if x == 2 {
            Rgba([217, 13, 131, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    }));
    let original = input.pixels.clone();
    let mut document = EditorDocument::from_image(input)?;
    document.insert(layer(
        1,
        Rect::new(2.0, 0.0, 1.0, 4.0),
        LayerKind::Redaction,
    ))?;
    document.insert(fill(2, Rect::new(2.0, 0.0, 1.0, 4.0), [255, 255, 0, 255]))?;
    document.insert(layer(
        3,
        Rect::new(4.0, 0.0, 4.0, 4.0),
        LayerKind::Clone {
            source: Affine::translate(-4.0, 0.0),
            interpolation: Interpolation::Nearest,
        },
    ))?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.get_pixel(2, 1).0, [0, 0, 0, 255]);
    assert_eq!(output.pixels.get_pixel(6, 1).0, [0, 0, 0, 255]);
    assert_eq!(output.pixels.get_pixel(5, 1).0, [0, 0, 255, 255]);
    assert_eq!(original.get_pixel(2, 1).0, [217, 13, 131, 255]);
    Ok(())
}
#[test]
fn rotated_existing_crop_matches_rotated_cropped_pixels() -> Result<()> {
    let source = source(4, 3);
    let expected = image::imageops::rotate90(
        &image::imageops::crop_imm(source.pixels.as_ref(), 1, 0, 2, 2).to_image(),
    );
    let mut document = EditorDocument::from_image(source)?;
    document.set_crop(Some(crate::PixelRegion {
        x: 1,
        y: 0,
        width: 2,
        height: 2,
    }))?;
    document.transform_document(
        Affine::translate(3.0, 0.0).then(Affine::rotate(90.0, [0.0; 2])),
        [3, 4],
    )?;
    assert_eq!(
        document.snapshot().render(|| false)?.pixels.as_ref(),
        &expected
    );
    document.undo();
    assert_eq!(document.spec().canvas, [4, 3]);
    Ok(())
}
#[test]
fn undo_and_external_snapshots_share_asset_leases_until_the_last_reference_drops() -> Result<()> {
    let mut document = EditorDocument::from_image(source(16, 16))?;
    document.insert_raster(
        "logo",
        Arc::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))),
    )?;
    document.insert(layer(
        1,
        Rect::new(0.0, 0.0, 4.0, 4.0),
        LayerKind::Raster {
            asset: "logo".into(),
            interpolation: Interpolation::Nearest,
            kind: AssetKind::Logo,
        },
    ))?;
    let snapshot = document.snapshot();
    let expected = 16 * 16 * 4 + 4 * 4 * 4;
    for _ in 0..120 {
        document.transform_layer(1, Affine::translate(0.01, 0.0))?;
    }
    assert_eq!(document.memory().retained_raster_bytes, expected);
    assert!(document.memory().undo_states <= 100);
    assert!(document.memory().metadata_bytes <= MAX_METADATA);
    document.remove(1)?;
    document.clear_history();
    assert_eq!(document.memory().retained_raster_bytes, expected);
    drop(snapshot);
    assert_eq!(document.memory().retained_raster_bytes, 16 * 16 * 4);
    Ok(())
}
#[test]
fn invalid_edits_and_preset_extension_conflicts_leave_state_unchanged() -> Result<()> {
    let mut document = EditorDocument::from_image(source(16, 16))?;
    let revision = document.revision();
    let spec = document.spec().clone();
    assert!(
        document
            .insert(layer(
                1,
                Rect::new(0.0, 0.0, 1.0, 1.0),
                LayerKind::Raster {
                    asset: "missing".into(),
                    interpolation: Interpolation::Nearest,
                    kind: AssetKind::Image
                }
            ))
            .is_err()
    );
    assert_eq!(document.spec(), &spec);
    assert_eq!(document.revision(), revision);
    assert!(
        ExportPreset::default()
            .validate_extension(std::path::Path::new("result.jpg"))
            .is_err()
    );
    assert!(
        document
            .transform_document(Affine([0.0; 6]), [16, 16])
            .is_err()
    );
    assert_eq!(document.revision(), revision);
    Ok(())
}

#[test]
fn source_privacy_survives_bilinear_upscale_lens_blur_and_both_erasers() -> Result<()> {
    let mut input = source(16, 16);
    input.pixels = Arc::new(RgbaImage::from_fn(16, 16, |x, y| {
        if (4..8).contains(&x) && (4..8).contains(&y) {
            Rgba([217, 13, 131, if x == 4 { 0 } else { 255 }])
        } else {
            Rgba([0, 0, 255, 255])
        }
    }));
    let mut spec = DocumentSpec::new(32, 32);
    spec.base = Affine::scale(2.0, 2.0);
    spec.interpolation = Interpolation::Bilinear;
    spec.layers.push(layer(
        1,
        Rect::new(8.0, 8.0, 8.0, 8.0),
        LayerKind::Redaction,
    ));
    spec.layers.push(layer(
        2,
        Rect::new(0.0, 0.0, 32.0, 32.0),
        LayerKind::Blur {
            radius: 2,
            input: EffectInput::Previous,
        },
    ));
    spec.layers.push(layer(
        3,
        Rect::new(16.0, 16.0, 16.0, 16.0),
        LayerKind::Magnifier {
            source: Affine::scale(0.5, 0.5),
            interpolation: Interpolation::Bilinear,
            style: Style {
                fill: None,
                stroke: Some([0, 255, 0, 255]),
                width: 1.0,
            },
        },
    ));
    for (id, mode) in [(4, EraseMode::Transparent), (5, EraseMode::RestoreBase)] {
        spec.layers.push(layer(
            id,
            Rect::new(8.0, 8.0, 8.0, 8.0),
            LayerKind::Erase {
                mode,
                shape: Shape::Rectangle,
                width: 12.0,
            },
        ));
    }
    let document = EditorDocument::new(input, EditorAssets::new(), spec)?;
    let output = document.snapshot().render(|| false)?;
    assert!(output.pixels.pixels().all(|pixel| pixel[0] == 0));
    for y in 8..16 {
        for x in 8..16 {
            assert_eq!(output.pixels.get_pixel(x, y).0, [0, 0, 0, 255]);
        }
    }
    Ok(())
}
#[test]
fn bubble_tail_and_explicit_rgba_emoji_are_real_raster_objects() -> Result<()> {
    let mut input = source(20, 20);
    input.pixels = Arc::new(RgbaImage::new(20, 20));
    let mut document = EditorDocument::from_image(input)?;
    document.insert(layer(
        1,
        Rect::new(2.0, 2.0, 8.0, 6.0),
        LayerKind::Draw {
            shape: Shape::SpeechBubble {
                tail: [14.0, 14.0],
                corner: 2.0,
            },
            style: Style {
                fill: Some([0, 255, 0, 200]),
                stroke: None,
                width: 0.0,
            },
        },
    ))?;
    document.insert_raster(
        "emoji",
        Arc::new(RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 128]))),
    )?;
    document.insert(layer(
        2,
        Rect::new(15.0, 0.0, 4.0, 4.0),
        LayerKind::Raster {
            asset: "emoji".into(),
            interpolation: Interpolation::Nearest,
            kind: AssetKind::Emoji,
        },
    ))?;
    let output = document.snapshot().render(|| false)?;
    assert!(output.pixels.get_pixel(12, 12)[3] > 0);
    assert_eq!(output.pixels.get_pixel(16, 1).0, [255, 0, 0, 128]);
    assert!(output.pixels.get_pixel(4, 4)[3] > 0);
    Ok(())
}
#[test]
fn shadow_blurs_into_padding_and_watermark_cannot_overpaint_final_privacy() -> Result<()> {
    let mut input = source(3, 3);
    input.pixels = Arc::new(RgbaImage::from_pixel(3, 3, Rgba([255; 4])));
    let mut document = EditorDocument::from_image(input)?;
    document.set_export(ExportPreset {
        padding: [4; 4],
        shadow: Some(Shadow {
            radius: 2,
            offset: [1, 1],
            color: [0, 0, 0, 128],
        }),
        ..Default::default()
    })?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.dimensions(), (11, 11));
    assert_eq!(output.pixels.get_pixel(8, 6).0, [0, 0, 0, 31]);
    assert_eq!(output.pixels.get_pixel(10, 10)[3], 0);
    document.insert(layer(
        1,
        Rect::new(1.0, 1.0, 1.0, 1.0),
        LayerKind::Redaction,
    ))?;
    document.insert_raster(
        "mark",
        Arc::new(RgbaImage::from_pixel(1, 1, Rgba([0, 0, 255, 255]))),
    )?;
    document.edit(|spec| {
        spec.export.watermark = Some(Watermark {
            asset: "mark".into(),
            area: Rect::new(0.0, 0.0, 11.0, 11.0),
            opacity: 255,
        });
        Ok(())
    })?;
    assert_eq!(
        document
            .snapshot()
            .render(|| false)?
            .pixels
            .get_pixel(5, 5)
            .0,
        [0, 0, 0, 255]
    );
    Ok(())
}
#[test]
fn large_canvas_small_crop_refuses_before_raster_work_and_cancellation_never_mutates_state()
-> Result<()> {
    let mut spec = DocumentSpec::new(8192, 8192);
    spec.crop = Some(crate::PixelRegion {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    });
    let document = EditorDocument::new(source(16, 16), EditorAssets::new(), spec)?;
    assert!(
        document
            .snapshot()
            .render(|| false)
            .unwrap_err()
            .to_string()
            .contains("work extent")
    );
    assert_eq!(document.memory().retained_raster_bytes, 1024);
    let document = EditorDocument::from_image(source(256, 1024))?;
    let revision = document.revision();
    let calls = std::cell::Cell::new(0u32);
    let result = document.snapshot().render(|| {
        calls.set(calls.get() + 1);
        calls.get() > 8
    });
    assert!(matches!(
        result.unwrap_err().downcast_ref::<crate::CaptureError>(),
        Some(crate::CaptureError::Canceled)
    ));
    assert_eq!(document.revision(), revision);
    assert!(calls.get() < 12);
    Ok(())
}

#[test]
fn legacy_import_matches_drawing_and_overlapping_original_effect_oracles() -> Result<()> {
    let input = source(64, 48);
    let frame = crate::FrameAnnotation {
        area: crate::PixelRegion {
            x: 8,
            y: 6,
            width: 40,
            height: 30,
        },
        color: 0x22c55e,
        width: 3,
        filled: false,
    };
    let draws = vec![
        crate::DrawAnnotation::Frame(frame),
        crate::DrawAnnotation::Arrow(crate::ArrowAnnotation {
            start: [0, 0],
            end: [60, 40],
            color: 0xef4444,
            width: 4,
        }),
        crate::DrawAnnotation::Ellipse(crate::FrameAnnotation {
            area: crate::PixelRegion {
                x: 12,
                y: 9,
                width: 25,
                height: 22,
            },
            color: 0xffffff,
            ..frame
        }),
        crate::DrawAnnotation::Highlight(crate::HighlightAnnotation {
            area: crate::PixelRegion {
                x: 4,
                y: 4,
                width: 30,
                height: 20,
            },
            color: 0xff8000,
            opacity: 91,
            outside: false,
        }),
        crate::DrawAnnotation::Pen(
            crate::PenAnnotation::from_trace(&[[2, 10], [16, 2], [30, 18]], 0x3366ff).unwrap(),
        ),
        crate::DrawAnnotation::Text(crate::TextAnnotation {
            area: crate::PixelRegion {
                x: 20,
                y: 25,
                width: 15,
                height: 6,
            },
            color: 0xffffff,
            width: 3,
            height: 2,
            mask: Arc::from([255, 128, 0, 64, 200, 255]),
        }),
    ];
    let crop = Some(crate::PixelRegion {
        x: 3,
        y: 2,
        width: 51,
        height: 40,
    });
    let blurs = [crate::PixelRegion {
        x: 8,
        y: 8,
        width: 48,
        height: 32,
    }];
    let pixelations = [crate::PixelRegion {
        x: 20,
        y: 4,
        width: 40,
        height: 35,
    }];
    for effects in [false, true] {
        let (pixels, blur) = if effects {
            (&pixelations[..], &blurs[..])
        } else {
            (&[][..], &[][..])
        };
        let grid = (!pixels.is_empty())
            .then(|| crate::PixelationGrid::prepare(&input.pixels, || false))
            .transpose()?;
        let expected =
            crate::render_ordered_snapshot(&input, &[], crop, &draws, pixels, grid.as_ref(), blur)?;
        let document = EditorDocument::from_legacy(input.clone(), &[], crop, &draws, pixels, blur)?;
        assert_eq!(
            document.snapshot().render(|| false)?.pixels.as_ref(),
            expected.pixels.as_ref(),
            "legacy effects={effects}"
        );
    }
    Ok(())
}

#[test]
fn legacy_affine_pixels_and_cross_bank_mask_reservations() -> Result<()> {
    let mut input = source(32, 24);
    input.pixels = Arc::new(RgbaImage::new(32, 24));
    let draw = crate::DrawAnnotation::Frame(crate::FrameAnnotation {
        area: crate::PixelRegion {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        },
        color: 0xef4444,
        width: 3,
        filled: false,
    });
    let expected = crate::render_ordered_snapshot(
        &input,
        &[],
        None,
        std::slice::from_ref(&draw),
        &[],
        None,
        &[],
    )?;
    let expected = image::imageops::rotate90(expected.pixels.as_ref());
    let mut document = EditorDocument::from_legacy(input, &[], None, &[draw], &[], &[])?;
    document.transform_document(
        Affine::translate(24.0, 0.0).then(Affine::rotate(90.0, [0.0; 2])),
        [24, 32],
    )?;
    assert_eq!(
        document.snapshot().render(|| false)?.pixels.as_ref(),
        &expected
    );
    let assets = EditorAssets::new();
    let annotation = LegacyAnnotation::new(
        crate::DrawAnnotation::Text(crate::TextAnnotation {
            area: crate::PixelRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            color: 0xffffff,
            width: 2,
            height: 1,
            mask: Arc::from([255, 128]),
        }),
        [16, 16],
        &assets,
    )?;
    let mut spec = DocumentSpec::new(16, 16);
    spec.layers.push(layer(
        1,
        annotation.bounds(),
        LayerKind::Legacy { annotation },
    ));
    assert!(EditorDocument::new(source(16, 16), EditorAssets::new(), spec).is_err());
    Ok(())
}

#[test]
fn privacy_outside_crop_and_rotated_mask_shape() -> Result<()> {
    let mut input = source(32, 32);
    input.pixels = Arc::new(RgbaImage::from_fn(32, 32, |x, y| {
        if (8..10).contains(&x) && (8..10).contains(&y) {
            Rgba([217, 13, 131, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    }));
    let mut document = EditorDocument::from_image(input)?;
    document.insert(layer(
        1,
        Rect::new(8.0, 8.0, 2.0, 2.0),
        LayerKind::Redaction,
    ))?;
    document.insert(layer(
        2,
        Rect::new(0.0, 0.0, 2.0, 2.0),
        LayerKind::Clone {
            source: Affine::translate(8.0, 8.0),
            interpolation: Interpolation::Nearest,
        },
    ))?;
    document.set_crop(Some(crate::PixelRegion {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    }))?;
    assert_eq!(
        document
            .snapshot()
            .render(|| false)?
            .pixels
            .get_pixel(0, 0)
            .0,
        [0, 0, 0, 255]
    );
    let mut input = source(32, 32);
    input.pixels = Arc::new(RgbaImage::from_pixel(32, 32, Rgba([0, 0, 255, 255])));
    let mut document = EditorDocument::from_image(input)?;
    let mut mask = layer(1, Rect::new(12.0, 12.0, 8.0, 8.0), LayerKind::Redaction);
    mask.transform = Affine::rotate(45.0, [16.0, 16.0]);
    document.insert(mask)?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.get_pixel(10, 10).0, [0, 0, 255, 255]);
    assert_eq!(output.pixels.get_pixel(16, 16).0, [0, 0, 0, 255]);
    Ok(())
}

#[test]
fn preset_book_roundtrip_applies_actual_tool_and_export_pixels() -> Result<()> {
    let book = PresetBook {
        version: 1,
        tools: vec![ToolPreset {
            name: "green".into(),
            content: LayerKind::Draw {
                shape: Shape::Rectangle,
                style: Style {
                    fill: Some([0, 255, 0, 255]),
                    stroke: None,
                    width: 0.0,
                },
            },
        }],
        exports: vec![NamedExportPreset {
            name: "white".into(),
            preset: ExportPreset {
                padding: [2; 4],
                background: [255; 4],
                ..Default::default()
            },
        }],
    };
    let book = PresetBook::read_json(&serde_json::to_vec(&book)?)?;
    let mut document = EditorDocument::from_image(source(8, 4))?;
    document.insert(fill(1, Rect::new(1.0, 1.0, 4.0, 2.0), [255, 0, 0, 255]))?;
    book.apply_tool("green", &mut document, 1)?;
    book.apply_export("white", &mut document)?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.dimensions(), (12, 8));
    assert_eq!(output.pixels.get_pixel(0, 0).0, [255; 4]);
    assert_eq!(output.pixels.get_pixel(4, 3).0, [0, 255, 0, 255]);
    assert!(book.apply_tool("missing", &mut document, 1).is_err());
    let mut duplicate = book.clone();
    duplicate.tools.push(duplicate.tools[0].clone());
    assert!(duplicate.validate().is_err());
    assert!(PresetBook::read_json(br#"{"version":1,"extra":1}"#).is_err());
    Ok(())
}

#[test]
fn path_eraser_uses_real_coverage_and_cannot_clear_an_active_mask() -> Result<()> {
    let mut input = source(128, 64);
    input.pixels = Arc::new(RgbaImage::from_pixel(128, 64, Rgba([0, 255, 0, 255])));
    let mut document = EditorDocument::from_image(input)?;
    document.insert(layer(
        1,
        Rect::new(48.0, 16.0, 16.0, 16.0),
        LayerKind::Redaction,
    ))?;
    document.insert(layer(
        2,
        Rect::new(8.0, 18.0, 112.0, 12.0),
        LayerKind::Erase {
            mode: EraseMode::Transparent,
            shape: Shape::Path {
                points: vec![[8.0, 24.0], [120.0, 24.0]],
                closed: false,
            },
            width: 12.0,
        },
    ))?;
    let output = document.snapshot().render(|| false)?;
    assert_eq!(output.pixels.get_pixel(32, 24).0, [0; 4]);
    assert_eq!(output.pixels.get_pixel(56, 24).0, [0, 0, 0, 255]);
    assert_eq!(output.pixels.get_pixel(32, 40).0, [0, 255, 0, 255]);
    Ok(())
}

#[test]
#[ignore = "optimized synthetic renderer timing/owned-resource/RSS observation; no desktop or user pixels"]
fn optimized_editor_resource_observation() -> Result<()> {
    fn rss() -> Option<String> {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
    }
    let mut reports = Vec::new();
    let process_before = rss();
    for (width, height) in [(2560, 1440), (3840, 2160)] {
        let input = source(width, height);
        let bytes = input.pixels.as_raw().len() as u64;
        let identity = EditorDocument::from_image(input.clone())?;
        let mut baseline = Vec::new();
        let mut current = Vec::new();
        let all = crate::PixelRegion {
            x: 0,
            y: 0,
            width,
            height,
        };
        for _ in 0..2 {
            std::hint::black_box(crate::render_ordered_snapshot(
                &input,
                &[],
                Some(all),
                &[],
                &[],
                None,
                &[],
            )?);
            std::hint::black_box(identity.snapshot().render(|| false)?);
        }
        for index in 0..100 {
            for document_first in [index % 2 == 0, index % 2 != 0] {
                let started = std::time::Instant::now();
                let output = if document_first {
                    identity.snapshot().render(|| false)?
                } else {
                    crate::render_ordered_snapshot(&input, &[], Some(all), &[], &[], None, &[])?
                };
                let elapsed = started.elapsed().as_micros() as u64;
                if document_first {
                    current.push(elapsed);
                } else {
                    baseline.push(elapsed);
                }
                assert_eq!(output.pixels.as_ref(), input.pixels.as_ref());
            }
        }
        let mut document = EditorDocument::from_image(input.clone())?;
        document.insert(layer(
            1,
            Rect::new(100.0, 100.0, 80.0, 80.0),
            LayerKind::Redaction,
        ))?;
        document.insert(fill(
            2,
            Rect::new(200.0, 160.0, 360.0, 180.0),
            [19, 221, 37, 128],
        ))?;
        document.insert(layer(
            3,
            Rect::new(80.0, 80.0, 512.0, 384.0),
            LayerKind::Blur {
                radius: 8,
                input: EffectInput::Previous,
            },
        ))?;
        document.insert(layer(
            4,
            Rect::new(800.0, 200.0, 512.0, 384.0),
            LayerKind::Pixelate {
                block: 16,
                input: EffectInput::Previous,
            },
        ))?;
        document.insert(layer(
            5,
            Rect::new(1400.0, 200.0, 400.0, 240.0),
            LayerKind::Clone {
                source: Affine::translate(-1000.0, 0.0),
                interpolation: Interpolation::Nearest,
            },
        ))?;
        document.insert_font(
            "mono",
            Arc::from(&include_bytes!("../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf")[..]),
            0,
        )?;
        document.insert(layer(
            6,
            Rect::new(200.0, 600.0, 640.0, 96.0),
            LayerKind::Text {
                text: RichText {
                    spans: vec![TextSpan {
                        text: "Bounded editor AV 123".into(),
                        style: TextStyle {
                            fonts: vec!["mono".into()],
                            size: 24.0,
                            weight: 700,
                            italic: true,
                            underline: true,
                            strike: false,
                            color: [255, 255, 255, 200],
                        },
                    }],
                    alignment: Alignment::Left,
                    line_height: 1.25,
                },
            },
        ))?;
        let mut ordered = Vec::new();
        for _ in 0..2 {
            std::hint::black_box(document.snapshot().render(|| false)?);
        }
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let output = document.snapshot().render(|| false)?;
            ordered.push(started.elapsed().as_micros() as u64);
            std::hint::black_box(output);
        }
        let retained = document.memory();
        let mut metadata_latencies = Vec::new();
        for _ in 0..150 {
            let started = std::time::Instant::now();
            document.transform_layer(2, Affine::translate(0.01, 0.0))?;
            metadata_latencies.push(started.elapsed().as_nanos() as u64);
        }
        assert_eq!(document.memory().retained_raster_bytes, bytes);
        assert!(document.memory().undo_states <= 100);
        assert!(document.memory().metadata_bytes <= MAX_METADATA);
        let mut observer = document.assets.clone();
        observer.rasters.clear();
        observer.fonts.clear();
        drop(document);
        assert_eq!(observer.raster_bytes(), 0);
        assert_eq!(observer.font_bytes(), 0);
        baseline.sort_unstable();
        current.sort_unstable();
        ordered.sort_unstable();
        metadata_latencies.sort_unstable();
        reports.push(serde_json::json!({"width":width,"height":height,"samples":100,"warmup_iterations":2,"alternating_copy_order":true,"legacy_copy_us":baseline,"document_identity_us":current,"ordered_effects_text_us":ordered,"metadata_edit_p95_ns":metadata_latencies[142],"source_bytes":bytes,"output_bytes":bytes,"retained_font_bytes":retained.retained_font_bytes,"initial_metadata_bytes":retained.metadata_bytes,"owned_budget_target_bytes":bytes*4+32*1024*1024,"document_resource_leases_release":true,"process":rss()}));
        drop(identity);
        drop(input);
    }
    println!(
        "EDITOR_RESOURCE_REPORT:{}",
        serde_json::json!({"cases":reports,"process_before":process_before,"process_after":rss(),"reference_hardware_acceptance":false,"presentation_included":false,"rss_includes_test_process_and_allocator_caches":true})
    );
    Ok(())
}
