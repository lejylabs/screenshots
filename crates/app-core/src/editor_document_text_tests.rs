use super::*;
fn fonts() -> Result<EditorAssets> {
    let mut assets = EditorAssets::new();
    assets.insert_font(
        "mono",
        Arc::from(&include_bytes!("../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf")[..]),
        0,
    )?;
    assets.insert_font(
        "arabic",
        Arc::from(&include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf")[..]),
        0,
    )?;
    assets.insert_font(
        "hebrew",
        Arc::from(&include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf")[..]),
        0,
    )?;
    Ok(assets)
}
fn label(text: &str) -> RichText {
    RichText {
        spans: vec![TextSpan {
            text: text.into(),
            style: TextStyle {
                fonts: vec!["mono".into(), "arabic".into(), "hebrew".into()],
                size: 24.0,
                weight: 500,
                italic: false,
                underline: false,
                strike: false,
                color: [19, 37, 61, 128],
            },
        }],
        alignment: Alignment::Left,
        line_height: 1.25,
    }
}
#[test]
fn explicit_outline_fonts_render_real_multiline_bidi_and_alpha() -> Result<()> {
    let assets = fonts()?;
    let cancelled = || false;
    let mut work = ImageWork::new(&cancelled)?;
    let text = label("Hello مرحبا\nשלום world");
    let image = prepare(&text, &assets, [600, 160], &mut work)?;
    assert!(image.pixels().filter(|pixel| pixel[3] > 0).count() > 100);
    assert!(image.pixels().all(|pixel| pixel[3] <= 128));
    assert!(
        image
            .pixels()
            .filter(|pixel| pixel[3] > 0)
            .all(|pixel| pixel.0[..3] == [19, 37, 61])
    );
    Ok(())
}
#[test]
fn malformed_fonts_and_overlong_contextual_runs_refuse_without_splitting() -> Result<()> {
    assert!(admit_font(b"not a font", 0).is_err());
    let assets = fonts()?;
    let cancelled = || false;
    let mut work = ImageWork::new(&cancelled)?;
    let text = label(&"a".repeat(129));
    let error = prepare(&text, &assets, [2048, 128], &mut work).unwrap_err();
    assert!(error.to_string().contains("128"), "{error}");
    Ok(())
}

#[test]
fn contextual_arabic_combining_marks_and_styles_use_real_glyphs() -> Result<()> {
    let assets = fonts()?;
    let cancelled = || false;
    let mut work = ImageWork::new(&cancelled)?;
    let mut arabic = label("مرحبا");
    arabic.spans[0].style.fonts = vec!["arabic".into()];
    arabic.spans[0].style.weight = 400;
    let layout = Layout::new(&arabic, &assets);
    let shaped = layout.shape(0..layout.text.len(), arabic.line_height, &mut work)?;
    let asset = &assets.fonts["arabic"];
    let face = FontRef::from_index(&asset.data.bytes, asset.index).unwrap();
    let nominal = arabic.spans[0]
        .text
        .chars()
        .map(|character| face.charmap().map(character).unwrap().to_u32())
        .collect::<Vec<_>>();
    assert!(
        shaped
            .glyphs
            .iter()
            .any(|glyph| !nominal.contains(&glyph.id)),
        "contextual forms must differ from nominal cmap glyphs"
    );
    let combining = label("e\u{301}");
    let layout = Layout::new(&combining, &assets);
    let shaped = layout.shape(0..layout.text.len(), combining.line_height, &mut work)?;
    assert_eq!(
        shaped.glyphs.len(),
        1,
        "the real shaper normalizes a supported combining sequence"
    );
    let normal = prepare(&label("AV"), &assets, [256, 96], &mut work)?;
    let mut styled = label("AV");
    styled.spans[0].style.weight = 700;
    styled.spans[0].style.italic = true;
    styled.spans[0].style.underline = true;
    styled.spans[0].style.strike = true;
    let decorated = prepare(&styled, &assets, [256, 96], &mut work)?;
    assert_ne!(normal, decorated);
    assert!(
        decorated
            .pixels()
            .map(|pixel| u64::from(pixel[3]))
            .sum::<u64>()
            > normal
                .pixels()
                .map(|pixel| u64::from(pixel[3]))
                .sum::<u64>()
    );
    assert!(decorated.pixels().all(|pixel| pixel[3] <= 128));
    Ok(())
}

#[test]
fn variable_weight_changes_outlines_and_valid_fonts_respect_operation_budget() -> Result<()> {
    let mut assets = EditorAssets::new();
    let bytes: Arc<[u8]> =
        Arc::from(&include_bytes!("../../../vendor/cosmic-text/fonts/InterVariable.ttf")[..]);
    assets.insert_font("variable", bytes.clone(), 0)?;
    assets.insert_font("same", bytes, 0)?;
    assert_eq!(
        assets.font_bytes(),
        include_bytes!("../../../vendor/cosmic-text/fonts/InterVariable.ttf").len() as u64,
        "face aliases share immutable font backing"
    );
    let mut thin = label("MM");
    thin.spans[0].style.fonts = vec!["variable".into()];
    thin.spans[0].style.weight = 100;
    let mut heavy = thin.clone();
    heavy.spans[0].style.weight = 900;
    let cancelled = || false;
    let mut work = ImageWork::new(&cancelled)?;
    let a = prepare(&thin, &assets, [256, 96], &mut work)?;
    let b = prepare(&heavy, &assets, [256, 96], &mut work)?;
    assert_ne!(a, b);
    assert!(
        b.pixels().map(|pixel| u64::from(pixel[3])).sum::<u64>()
            > a.pixels().map(|pixel| u64::from(pixel[3])).sum::<u64>()
    );
    let source = crate::CapturedImage {
        display_id: "font-budget".into(),
        window_id: None,
        width: 16,
        height: 16,
        pixels: Arc::new(RgbaImage::new(16, 16)),
        png_path: "not-written.png".into(),
        elapsed_ms: 0,
        source_format: crate::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    };
    let mut spec = DocumentSpec::new(16, 16);
    spec.layers.push(Layer {
        id: 1,
        bounds: Rect::new(0.0, 0.0, 128.0, 64.0),
        transform: Affine::IDENTITY,
        opacity: 255,
        content: LayerKind::Text { text: heavy },
    });
    let document = EditorDocument::new(source, assets, spec)?;
    assert!(
        document
            .snapshot()
            .render(|| false)
            .unwrap_err()
            .to_string()
            .contains("work extent")
    );
    Ok(())
}

#[test]
fn font_directory_guards_and_real_glyph_preparation_cancellation() -> Result<()> {
    let original = include_bytes!("../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf");
    let mut duplicate = original.to_vec();
    let tag = duplicate[12..16].to_vec();
    duplicate[28..32].copy_from_slice(&tag);
    assert!(admit_font(&duplicate, 0).is_err());
    let mut extent = original.to_vec();
    extent[24..28].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(admit_font(&extent, 0).is_err());
    assert!(admit_font(&vec![0; 2 * 1024 * 1024 + 1], 0).is_err());
    assert!(admit_font(original, 16).is_err());
    let assets = fonts()?;
    let calls = std::cell::Cell::new(0);
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 10
    };
    let mut work = ImageWork::new(&cancelled)?;
    let result = prepare(&label(&"@".repeat(64)), &assets, [1024, 128], &mut work);
    assert!(matches!(
        result.unwrap_err().downcast_ref::<crate::CaptureError>(),
        Some(crate::CaptureError::Canceled)
    ));
    assert!(calls.get() <= 12);
    Ok(())
}

// Source-derived from HarfRust 0.15's repeated_pair_sets_charge_every_record_visit:
// many valid offsets alias small pair sets, so encoded bytes do not bound visits.
fn aliased_pair_set_font(subtables: u16) -> Vec<u8> {
    fn words(values: impl IntoIterator<Item = u16>) -> Vec<u8> {
        values.into_iter().flat_map(u16::to_be_bytes).collect()
    }
    fn checksum(bytes: &[u8]) -> u32 {
        bytes.chunks(4).fold(0u32, |sum, chunk| {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            sum.wrapping_add(u32::from_be_bytes(word))
        })
    }
    let mut scripts = words([1]);
    scripts.extend_from_slice(b"DFLT");
    scripts.extend(words([8, 4, 0, 0, 0xffff, 1, 0]));
    let mut features = words([1]);
    features.extend_from_slice(b"kern");
    features.extend(words([8, 0, 1, 0]));
    let mut gpos = words([
        1,
        0,
        10,
        10 + scripts.len() as u16,
        10 + scripts.len() as u16 + features.len() as u16,
    ]);
    gpos.extend(scripts);
    gpos.extend(features);
    gpos.extend(words([1, 4, 2, 0, subtables]));
    gpos.extend(words(std::iter::repeat_n(
        6 + 2 * subtables,
        subtables as usize,
    )));
    // PairPos1: 128 pair-set offsets, all pointing at the same 128 records.
    // Both coverages and second glyphs use IDs 0..127 from the real base font.
    gpos.extend(words([1, 266, 0, 0, 128]));
    gpos.extend(words(std::iter::repeat_n(526, 128)));
    gpos.extend(words([1, 128]));
    gpos.extend(words(0..128));
    gpos.extend(words([128]));
    gpos.extend(words(0..128));
    let mut font = include_bytes!("../../../vendor/cosmic-text/fonts/FiraMono-Medium.ttf").to_vec();
    while !font.len().is_multiple_of(4) {
        font.push(0);
    }
    let offset = font.len();
    font.extend_from_slice(&gpos);
    while !font.len().is_multiple_of(4) {
        font.push(0);
    }
    let table_count = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
    let mut found = false;
    let mut head = None;
    for entry in 0..table_count {
        let start = 12 + entry * 16;
        match &font[start..start + 4] {
            b"GPOS" => {
                font[start + 4..start + 8].copy_from_slice(&checksum(&gpos).to_be_bytes());
                font[start + 8..start + 12].copy_from_slice(&(offset as u32).to_be_bytes());
                font[start + 12..start + 16].copy_from_slice(&(gpos.len() as u32).to_be_bytes());
                found = true;
            }
            b"head" => {
                head = Some(
                    u32::from_be_bytes(font[start + 8..start + 12].try_into().unwrap()) as usize,
                )
            }
            _ => {}
        }
    }
    assert!(found);
    let adjust = head.unwrap() + 8;
    font[adjust..adjust + 4].fill(0);
    let adjustment = 0xb1b0afbau32.wrapping_sub(checksum(&font));
    font[adjust..adjust + 4].copy_from_slice(&adjustment.to_be_bytes());
    assert_eq!(checksum(&font), 0xb1b0afba);
    font
}

#[test]
fn exhausted_real_gpos_preprocessing_is_observable_and_refused_before_glyph_output() -> Result<()> {
    let cancelled = || false;
    for (subtables, exhausted) in [(1, false), (512, true)] {
        let bytes: Arc<[u8]> = aliased_pair_set_font(subtables).into();
        admit_font(&bytes, 0)?;
        let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes.clone());
        let face = harfrust::Font::new(blob, 0).unwrap();
        let shaper = harfrust::ShaperFont::new(&face);
        assert!(!shaper.preprocessing_budget_exhausted());
        let mut buffer = harfrust::Buffer::new();
        buffer.push_str("AA");
        buffer.guess_segment_properties();
        harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new()).unwrap();
        assert!(
            buffer.allocation_successful(),
            "the buffer alone does not detect font preprocessing exhaustion"
        );
        assert_eq!(shaper.preprocessing_budget_exhausted(), exhausted);
        // Cached exhaustion must persist when the same font is reused/instanced.
        let instance = face.instance_builder().build();
        assert_eq!(
            harfrust::ShaperFont::new(&instance).preprocessing_budget_exhausted(),
            exhausted
        );
        let mut assets = EditorAssets::new();
        assets.insert_font("budget", bytes, 0)?;
        let mut text = label("AA");
        text.spans[0].style.fonts = vec!["budget".into()];
        let mut work = ImageWork::new(&cancelled)?;
        let result = prepare(&text, &assets, [128, 64], &mut work);
        if exhausted {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("preprocessing exhausted")
            );
        } else {
            assert!(result?.pixels().any(|pixel| pixel[3] != 0));
        }
    }
    Ok(())
}
