use super::*;
use image::{ExtendedColorType, ImageEncoder, Rgba};
use std::cell::Cell;

const P3: &[u8] = include_bytes!("../tests/fixtures/color/lcms-display-p3.icc");
const SRGB: &[u8] = include_bytes!("../tests/fixtures/color/lcms-srgb.icc");
const P3_PNG16: &[u8] = include_bytes!("../tests/fixtures/color/lcms-p3-rgba16.png");
const ORACLE: &[u8] = include_bytes!("../tests/fixtures/color/lcms-p3-rgba16-to-srgb8.rgba");

fn chunk(kind: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(bytes);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(bytes);
    output.extend_from_slice(&crc.finalize().to_be_bytes());
    output
}
fn png_pixels() -> (Vec<u8>, RgbaImage) {
    let pixels = RgbaImage::from_fn(4, 2, |x, y| {
        Rgba([
            30 + x as u8 * 40,
            110 + y as u8 * 60,
            90,
            [0, 1, 128, 255][x as usize],
        ])
    });
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(pixels.as_raw(), 4, 2, ExtendedColorType::Rgba8)
        .unwrap();
    (bytes, pixels)
}
fn with_chunks(bytes: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut output = bytes[..33].to_vec();
    for chunk in chunks {
        output.extend_from_slice(chunk);
    }
    output.extend_from_slice(&bytes[33..]);
    output
}
fn iccp(profile: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(profile).unwrap();
    let mut payload = b"profile\0\0".to_vec();
    payload.extend(encoder.finish().unwrap());
    chunk(b"iCCP", &payload)
}
fn without_iccp(bytes: &[u8]) -> Vec<u8> {
    let mut result = bytes[..8].to_vec();
    let mut at = 8;
    while at < bytes.len() {
        let length = be32(bytes, at).unwrap() as usize;
        let end = at + length + 12;
        if &bytes[at + 4..at + 8] != b"iCCP" {
            result.extend_from_slice(&bytes[at..end]);
        }
        at = end;
    }
    result
}
fn assert_oracle(actual: &RgbaImage, expected: &[u8]) {
    assert_oracle_named(actual, expected, "independent LittleCMS");
}
fn assert_oracle_named(actual: &RgbaImage, expected: &[u8], name: &str) {
    assert_eq!(actual.as_raw().len(), expected.len());
    for (index, (actual, expected)) in actual.as_raw().iter().zip(expected).enumerate() {
        if index % 4 == 3 {
            assert_eq!(actual, expected, "{name}: alpha at byte {index}");
        } else {
            assert!(
                actual.abs_diff(*expected) <= 2,
                "{name}: LittleCMS disagreement at byte {index}: {actual} vs {expected}"
            );
        }
    }
}

const CHANNEL_CASES: &[(&str, &[u8], &[u8])] = &[
    (
        "para-rg",
        include_bytes!("../tests/fixtures/color/channels/para-rg.png"),
        include_bytes!("../tests/fixtures/color/channels/para-rg.rgba"),
    ),
    (
        "para-gb",
        include_bytes!("../tests/fixtures/color/channels/para-gb.png"),
        include_bytes!("../tests/fixtures/color/channels/para-gb.rgba"),
    ),
    (
        "para-rb",
        include_bytes!("../tests/fixtures/color/channels/para-rb.png"),
        include_bytes!("../tests/fixtures/color/channels/para-rb.rgba"),
    ),
    (
        "para-equal1",
        include_bytes!("../tests/fixtures/color/channels/para-equal1.png"),
        include_bytes!("../tests/fixtures/color/channels/para-equal1.rgba"),
    ),
    (
        "para-equal2",
        include_bytes!("../tests/fixtures/color/channels/para-equal2.png"),
        include_bytes!("../tests/fixtures/color/channels/para-equal2.rgba"),
    ),
    (
        "curv-rg",
        include_bytes!("../tests/fixtures/color/channels/curv-rg.png"),
        include_bytes!("../tests/fixtures/color/channels/curv-rg.rgba"),
    ),
    (
        "curv-gb",
        include_bytes!("../tests/fixtures/color/channels/curv-gb.png"),
        include_bytes!("../tests/fixtures/color/channels/curv-gb.rgba"),
    ),
    (
        "curv-rb",
        include_bytes!("../tests/fixtures/color/channels/curv-rb.png"),
        include_bytes!("../tests/fixtures/color/channels/curv-rb.rgba"),
    ),
    (
        "curv-equal1",
        include_bytes!("../tests/fixtures/color/channels/curv-equal1.png"),
        include_bytes!("../tests/fixtures/color/channels/curv-equal1.rgba"),
    ),
    (
        "curv-equal2",
        include_bytes!("../tests/fixtures/color/channels/curv-equal2.png"),
        include_bytes!("../tests/fixtures/color/channels/curv-equal2.rgba"),
    ),
];

#[test]
fn independent_channel_trcs_match_lcms_for_para_curv_and_all_equality_patterns() {
    for (name, png, oracle) in CHANNEL_CASES {
        let pixels = decode(png, ImageFormat::Png, 96, Some([3, 4]), &|| false)
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
        assert_oracle_named(&pixels, oracle, name);
        assert_eq!(
            pixels.get_pixel(0, 0).0[3],
            0,
            "{name}: straight hidden RGB"
        );
        if name.ends_with("-rg") {
            // Linear red/green128 encodes as188, while gamma2 blue128 encodes as137.
            // The faulty upstream common-red path produced blue188 instead.
            assert_eq!(&oracle[..4], &[188, 188, 137, 0]);
            assert!(pixels.get_pixel(0, 0).0[0].abs_diff(188) <= 2, "{name}");
            assert!(pixels.get_pixel(0, 0).0[2].abs_diff(137) <= 2, "{name}");
        }
        assert!(
            decode(png, ImageFormat::Png, 95, None, &|| false)
                .unwrap_err()
                .to_string()
                .contains("native precision"),
            "{name}"
        );
    }
}

#[test]
fn independent_channel_trcs_preserve_alpha_and_every_orientation_from_rgb16() {
    for (name, png, oracle) in CHANNEL_CASES
        .iter()
        .filter(|case| matches!(case.0, "para-rg" | "curv-gb"))
    {
        for value in 1..=8 {
            let mut exif =
                b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0".to_vec();
            exif[18] = value;
            let input = with_chunks(png, &[chunk(b"eXIf", &exif)]);
            let mut expected =
                DynamicImage::ImageRgba8(RgbaImage::from_raw(3, 4, oracle.to_vec()).unwrap());
            expected.apply_orientation(Orientation::from_exif(value).unwrap());
            let expected = expected.into_rgba8();
            let actual = decode(
                &input,
                ImageFormat::Png,
                96,
                Some([expected.width(), expected.height()]),
                &|| false,
            )
            .unwrap();
            assert_oracle_named(&actual, expected.as_raw(), name);
        }
    }
}

fn parametric_profile(kind: u16, coefficients: &[i32]) -> Vec<u8> {
    let mut profile = P3.to_vec();
    let offset = profile.len();
    let length = 12 + coefficients.len() * 4;
    profile.extend_from_slice(b"para\0\0\0\0");
    profile.extend_from_slice(&kind.to_be_bytes());
    profile.extend_from_slice(&[0, 0]);
    for coefficient in coefficients {
        profile.extend_from_slice(&coefficient.to_be_bytes());
    }
    let count = be32(&profile, 128).unwrap() as usize;
    for index in 0..count {
        let at = 132 + index * 12;
        if matches!(&profile[at..at + 4], b"rTRC" | b"gTRC" | b"bTRC") {
            profile[at + 4..at + 8].copy_from_slice(&(offset as u32).to_be_bytes());
            profile[at + 8..at + 12].copy_from_slice(&(length as u32).to_be_bytes());
        }
    }
    let length = profile.len() as u32;
    profile[..4].copy_from_slice(&length.to_be_bytes());
    profile
}

#[test]
fn parametric_trcs_validate_complete_branches_between_former_samples() {
    let (png, _) = png_pixels();
    // All former i/256 sample points are finite; the active branch is undefined
    // for x in [1/65536,64/65536), which the real CMS samples more densely.
    let negative_domain = parametric_profile(3, &[32768, 65536, -64, 0, 1]);
    let error = decode(
        &with_chunks(&png, &[iccp(&negative_domain)]),
        ImageFormat::Png,
        32,
        None,
        &|| false,
    )
    .unwrap_err();
    assert!(error.to_string().contains("power-branch domain"));
    // A downward discontinuity and decreasing low branch can also fit between
    // those old samples; neither reaches the CMS.
    for coefficients in [[65536, 65536, -64, 65536, 64], [65536, 65536, 0, -1, 64]] {
        let profile = parametric_profile(3, &coefficients);
        assert!(
            decode(
                &with_chunks(&png, &[iccp(&profile)]),
                ImageFormat::Png,
                32,
                None,
                &|| false
            )
            .is_err()
        );
    }
    // Exact zero at the active power-branch boundary is valid and monotone.
    let boundary = parametric_profile(3, &[32768, 65536, -64, 0, 64]);
    assert!(
        decode(
            &with_chunks(&png, &[iccp(&boundary)]),
            ImageFormat::Png,
            32,
            None,
            &|| false
        )
        .is_ok()
    );
    // One s15Fixed16 unit of junction/endpoint rounding remains admissible.
    let rounded = parametric_profile(4, &[65536, 65536, 0, 65536, 32768, 1, 0]);
    assert!(
        decode(
            &with_chunks(&png, &[iccp(&rounded)]),
            ImageFormat::Png,
            32,
            None,
            &|| false
        )
        .is_ok()
    );
}

#[test]
fn p3_icc_and_cicp_match_independent_lcms_rgb16_oracle_with_straight_alpha() {
    let image = decode(
        P3_PNG16,
        ImageFormat::Png,
        crate::MAX_CAPTURE_BYTES,
        None,
        &|| false,
    )
    .unwrap();
    assert_oracle(&image, ORACLE);
    // Alpha zero retains converted hidden RGB; it is not multiplied or discarded.
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 0]);
    // LittleCMS gives red=0 here; reducing the RGB16 input first gives red=7.
    assert!(image.get_pixel(2, 3).0[0] <= 2);
    let cicp = with_chunks(&without_iccp(P3_PNG16), &[chunk(b"cICP", &[12, 13, 0, 1])]);
    assert_oracle(
        &decode(
            &cicp,
            ImageFormat::Png,
            crate::MAX_CAPTURE_BYTES,
            None,
            &|| false,
        )
        .unwrap(),
        ORACLE,
    );
    let precision_error = decode(P3_PNG16, ImageFormat::Png, 95, None, &|| false).unwrap_err();
    assert!(precision_error.to_string().contains("native precision"));
}

#[test]
fn untagged_assumption_and_explicit_srgb_preserve_alpha_and_hidden_rgb() {
    let (bytes, pixels) = png_pixels();
    assert_eq!(
        decode(&bytes, ImageFormat::Png, 32, None, &|| false).unwrap(),
        pixels
    );
    for chunks in [
        vec![chunk(b"sRGB", &[0])],
        vec![chunk(b"cICP", &[1, 13, 0, 1])],
        vec![iccp(SRGB)],
    ] {
        let bytes = with_chunks(&bytes, &chunks);
        assert_oracle(
            &decode(&bytes, ImageFormat::Png, 32, None, &|| false).unwrap(),
            pixels.as_raw(),
        );
    }
}

#[test]
fn every_exif_orientation_keeps_color_and_alpha_and_checks_oriented_dimensions() {
    let base = decode(P3_PNG16, ImageFormat::Png, 96, None, &|| false).unwrap();
    for value in 1..=8 {
        let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0".to_vec();
        exif[18] = value;
        let bytes = with_chunks(P3_PNG16, &[chunk(b"eXIf", &exif)]);
        let orientation = Orientation::from_exif(value).unwrap();
        let mut expected = DynamicImage::ImageRgba8(base.clone());
        expected.apply_orientation(orientation);
        let expected = expected.into_rgba8();
        assert_eq!(
            decode(
                &bytes,
                ImageFormat::Png,
                96,
                Some([expected.width(), expected.height()]),
                &|| false
            )
            .unwrap(),
            expected
        );
        assert!(
            decode(&bytes, ImageFormat::Png, 96, Some([1, 1]), &|| false)
                .unwrap_err()
                .to_string()
                .contains("source_size")
        );
    }
}

#[test]
fn icc_refuses_bombs_bad_extents_duplicates_clut_cmyk_and_missing_curves() {
    assert!(icc(P3, &|| false).is_ok());
    for (offset, value) in [
        (0, 0xffff_ffffu32),
        (128, 65),
        (136, 0xffff_fffc),
        (140, 0xffff_ffff),
    ] {
        let mut bytes = P3.to_vec();
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        assert!(icc(&bytes, &|| false).is_err());
    }
    for (offset, value) in [
        (16, *b"CMYK"),
        (132, *b"A2B0"),
        (132, *b"cicp"),
        (132, *b"rTRC"),
    ] {
        let mut bytes = P3.to_vec();
        bytes[offset..offset + 4].copy_from_slice(&value);
        assert!(icc(&bytes, &|| false).is_err());
    }
    let mut bytes = P3.to_vec();
    let second = bytes[144..148].to_vec();
    bytes[132..136].copy_from_slice(&second);
    assert!(icc(&bytes, &|| false).is_err());
    let mut bytes = P3.to_vec();
    let tag = (0..be32(&bytes, 128).unwrap() as usize)
        .map(|i| 132 + 12 * i)
        .find(|at| &bytes[*at..*at + 4] == b"rTRC")
        .unwrap();
    let offset = be32(&bytes, tag + 4).unwrap() as usize;
    bytes[offset..offset + 4].copy_from_slice(b"curv");
    bytes[offset + 8..offset + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(icc(&bytes, &|| false).is_err());
    let (png, _) = png_pixels();
    assert!(
        decode(
            &with_chunks(&png, &[iccp(&vec![0; MAX_PROFILE + 1])]),
            ImageFormat::Png,
            32,
            None,
            &|| false
        )
        .unwrap_err()
        .to_string()
        .contains("after decompression")
    );
}

#[test]
fn png_refuses_conflicting_unsupported_hdr_legacy_and_bad_crc_metadata() {
    let (png, _) = png_pixels();
    for chunks in [
        vec![iccp(P3), chunk(b"cICP", &[12, 13, 0, 1])],
        vec![iccp(P3), chunk(b"sRGB", &[0])],
        vec![chunk(b"cICP", &[9, 16, 0, 1])],
        vec![chunk(b"cICP", &[9, 18, 0, 1])],
        vec![chunk(b"cICP", &[12, 13, 0, 0])],
        vec![chunk(b"cLLI", &[0; 8])],
        vec![chunk(b"gAMA", &45455u32.to_be_bytes())],
        vec![chunk(b"sRGB", &[0]), chunk(b"sRGB", &[0])],
    ] {
        assert!(
            decode(
                &with_chunks(&png, &chunks),
                ImageFormat::Png,
                32,
                None,
                &|| false
            )
            .is_err()
        );
    }
    let mut corrupt = iccp(P3);
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(
        decode(
            &with_chunks(&png, &[corrupt]),
            ImageFormat::Png,
            32,
            None,
            &|| false
        )
        .unwrap_err()
        .to_string()
        .contains("CRC")
    );
    // The image codec never receives this compressed text payload.
    let bytes = with_chunks(&png, &[chunk(b"zTXt", b"key\0\0not a zlib stream")]);
    assert!(decode(&bytes, ImageFormat::Png, 32, None, &|| false).is_ok());
}

fn app2(fragment: &[u8], index: u8, count: u8) -> Vec<u8> {
    let mut payload = b"ICC_PROFILE\0".to_vec();
    payload.extend_from_slice(&[index, count]);
    payload.extend_from_slice(fragment);
    let mut segment = vec![0xff, 0xe2];
    segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    segment.extend(payload);
    segment
}
#[test]
fn jpeg_icc_fragments_are_complete_unique_bounded_and_applied() {
    let pixels = image::RgbImage::from_pixel(3, 2, image::Rgb([130, 170, 100]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
        .write_image(pixels.as_raw(), 3, 2, ExtendedColorType::Rgb8)
        .unwrap();
    let add = |parts: &[Vec<u8>]| {
        let mut out = jpeg[..2].to_vec();
        for part in parts {
            out.extend_from_slice(part);
        }
        out.extend_from_slice(&jpeg[2..]);
        out
    };
    let tagged = add(&[app2(&P3[200..], 2, 2), app2(&P3[..200], 1, 2)]);
    let actual = decode(&tagged, ImageFormat::Jpeg, 24, None, &|| false).unwrap();
    let raw = image::load_from_memory(&jpeg).unwrap().into_rgb8();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(raw.as_raw(), 3, 2, ExtendedColorType::Rgb8)
        .unwrap();
    let expected = decode(
        &with_chunks(&png, &[iccp(P3)]),
        ImageFormat::Png,
        24,
        None,
        &|| false,
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert_ne!(actual, image::load_from_memory(&jpeg).unwrap().into_rgba8());
    for parts in [
        vec![app2(P3, 1, 2)],
        vec![app2(P3, 1, 1), app2(P3, 1, 1)],
        vec![app2(P3, 0, 1)],
        vec![app2(P3, 1, 0)],
        vec![app2(&P3[..200], 1, 2), app2(&P3[200..], 2, 3)],
    ] {
        assert!(decode(&add(&parts), ImageFormat::Jpeg, 24, None, &|| false).is_err());
    }
}

#[test]
fn large_encoded_tiny_jpeg_is_refused_before_decoder_input_copy() {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .write_image(&[130, 170, 100], 1, 1, ExtendedColorType::Rgb8)
        .unwrap();
    let mut padded = jpeg[..2].to_vec();
    let padding = vec![0; 65533];
    while padded.len() < 24 * 1024 * 1024 {
        padded.extend_from_slice(&[0xff, 0xe0, 0xff, 0xff]);
        padded.extend_from_slice(&padding);
    }
    padded.extend_from_slice(&jpeg[2..]);
    INPUT_READ_CALLS.with(|calls| calls.set(0));
    let error = decode(&padded, ImageFormat::Jpeg, 4, None, &|| false).unwrap_err();
    assert!(error.to_string().contains("active-image memory budget"));
    INPUT_READ_CALLS.with(|calls| {
        assert_eq!(
            calls.get(),
            0,
            "JPEG constructor must not read/allocate a duplicate before working-set admission"
        )
    });
}

#[test]
fn actual_owner_capacity_is_admitted_before_any_managed_index_or_decoder() {
    let pixels = RgbaImage::from_pixel(1, 1, Rgba([130, 170, 100, 255]));
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(pixels.as_raw(), 1, 1, ExtendedColorType::Rgba8)
        .unwrap();
    let mut owner = Vec::with_capacity(32 * 1024 * 1024);
    owner.extend_from_slice(&png[..33]);
    let mut text = b"Comment\0".to_vec();
    text.resize(20 * 1024 * 1024, b'a');
    owner.extend(chunk(b"tEXt", &text));
    owner.extend_from_slice(&png[33..]);
    drop(text);
    assert!(owner.len() < owner.capacity());
    assert_eq!(owner.capacity(), 32 * 1024 * 1024);
    INPUT_INDEX_ALLOCS.with(|calls| calls.set(0));
    INPUT_READ_CALLS.with(|calls| calls.set(0));
    let error =
        crate::decode_editor_image_bytes_with_backing_and_cancel(&owner, owner.capacity(), &|| {
            false
        })
        .unwrap_err();
    assert!(error.to_string().contains("active-image memory budget"));
    INPUT_INDEX_ALLOCS.with(|calls| assert_eq!(calls.get(), 0));
    INPUT_READ_CALLS.with(|calls| assert_eq!(calls.get(), 0));
    // A separately owned exact-sized representation has a different, admissible
    // working set. Production does not shrink/reallocate the oversized owner.
    let exact = owner.to_vec();
    drop(owner);
    assert_eq!(exact.capacity(), exact.len());
    assert_eq!(
        crate::decode_editor_image_bytes_with_backing_and_cancel(&exact, exact.capacity(), &|| {
            false
        })
        .unwrap(),
        pixels
    );
}

#[test]
fn tiny_tiff_pixel_ceiling_leaves_bounded_header_and_codec_working_space() {
    let pixels = RgbaImage::from_fn(3, 5, |x, y| {
        Rgba([x as u8 * 20, y as u8 * 30, 90, [0, 128, 255][x as usize]])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new(&mut bytes).unwrap();
        let mut image = encoder
            .new_image::<tiff::encoder::colortype::RGBA8>(3, 5)
            .unwrap();
        image
            .encoder()
            .write_tag(tiff::tags::Tag::ExtraSamples, &[2u16][..])
            .unwrap();
        image.write_data(pixels.as_raw()).unwrap();
    }
    let bytes = bytes.into_inner();
    assert_eq!(
        decode_with_backing(
            &bytes,
            bytes.capacity(),
            ImageFormat::Tiff,
            60,
            None,
            &|| false
        )
        .unwrap(),
        pixels
    );
    assert!(
        decode_with_backing(
            &bytes,
            bytes.capacity(),
            ImageFormat::Tiff,
            59,
            None,
            &|| false
        )
        .unwrap_err()
        .to_string()
        .contains("memory limit")
    );
}

#[test]
fn caller_pixel_rejection_keeps_the_memory_limit_diagnostic_before_decode() {
    let (mut png, _) = png_pixels();
    png[16..20].copy_from_slice(&2049u32.to_be_bytes());
    png[20..24].copy_from_slice(&2048u32.to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(&png[12..29]);
    png[29..33].copy_from_slice(&crc.finalize().to_be_bytes());
    INPUT_READ_CALLS.with(|calls| calls.set(0));
    let error = decode(&png, ImageFormat::Png, 16 * 1024 * 1024, None, &|| false).unwrap_err();
    assert_eq!(error.to_string(), "Decoded image exceeds the memory limit");
    INPUT_READ_CALLS.with(|calls| assert_eq!(calls.get(), 0));
}

#[test]
fn webp_icc_uses_the_shared_transform_and_refuses_duplicate_profiles() {
    let (_, pixels) = png_pixels();
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .write_image(pixels.as_raw(), 4, 2, ExtendedColorType::Rgba8)
        .unwrap();
    let riff_chunk = |kind: &[u8; 4], bytes: &[u8]| {
        let mut out = kind.to_vec();
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
        if !bytes.len().is_multiple_of(2) {
            out.push(0);
        }
        out
    };
    let add = |duplicate| {
        let mut body = b"WEBP".to_vec();
        body.extend(riff_chunk(b"VP8X", &[0x30, 0, 0, 0, 3, 0, 0, 1, 0, 0]));
        body.extend(riff_chunk(b"ICCP", P3));
        if duplicate {
            body.extend(riff_chunk(b"ICCP", P3));
        }
        body.extend_from_slice(&webp[12..]);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    };
    let actual = decode(&add(false), ImageFormat::WebP, 32, None, &|| false).unwrap();
    let (png, _) = png_pixels();
    let expected = decode(
        &with_chunks(&png, &[iccp(P3)]),
        ImageFormat::Png,
        32,
        None,
        &|| false,
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert!(decode(&add(true), ImageFormat::WebP, 32, None, &|| false).is_err());
}

#[test]
fn borrowed_segment_reader_seeks_and_limits_each_read_without_copying_the_image() {
    let cancelled = || false;
    let bytes = b"ignoredABCDignoredEFG";
    let mut input = Input::new(bytes, &cancelled);
    input.push(7, 11).unwrap();
    input.push(18, 21).unwrap();
    assert_eq!(input.seek(SeekFrom::End(-3)).unwrap(), 4);
    let mut out = [0; 3];
    input.read_exact(&mut out).unwrap();
    assert_eq!(&out, b"EFG");
    input.seek(SeekFrom::Start(2)).unwrap();
    let mut out = [0; 4];
    input.read_exact(&mut out).unwrap();
    assert_eq!(&out, b"CDEF");
    assert!(input.seek(SeekFrom::Current(-100)).is_err());
    let large = vec![1u8; WORK_BYTES * 2];
    let mut input = Input::new(&large, &cancelled);
    input.push(0, large.len()).unwrap();
    assert!(input.fill_buf().unwrap().len() <= WORK_BYTES);
}

#[test]
fn cancelled_preflight_and_conversion_do_not_return_pixels() {
    assert!(
        decode(P3_PNG16, ImageFormat::Png, 96, None, &|| true)
            .unwrap_err()
            .to_string()
            .contains("cancel")
    );
    let pixels = DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 100, Rgba([99, 80, 40, 0])));
    let calls = Cell::new(0usize);
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 3
    };
    assert!(
        convert(
            pixels,
            Some(&icc(P3, &|| false).unwrap()),
            Orientation::NoTransforms,
            &cancelled
        )
        .is_err()
    );
    assert_eq!(calls.get(), 4);
}

#[test]
fn cmyk_jpeg_is_refused_before_decoder_rgb_substitution() {
    let bytes = include_bytes!("../tests/fixtures/color/cmyk-jpeg.jpg");
    assert!(
        decode(
            bytes,
            ImageFormat::Jpeg,
            crate::MAX_CAPTURE_BYTES,
            None,
            &|| false
        )
        .unwrap_err()
        .to_string()
        .contains("CMYK")
    );
}

#[test]
#[ignore = "optimized worker timing report; run explicitly with --release --nocapture"]
fn optimized_4k_import_report() {
    use std::time::Instant;
    let pixels = RgbaImage::from_pixel(3840, 2160, Rgba([130, 170, 100, 128]));
    let mut untagged = Vec::new();
    image::codecs::png::PngEncoder::new(&mut untagged)
        .write_image(pixels.as_raw(), 3840, 2160, ExtendedColorType::Rgba8)
        .unwrap();
    let tagged = with_chunks(&untagged, &[iccp(P3)]);
    drop(pixels);
    let baseline = || {
        image::load_from_memory_with_format(&untagged, ImageFormat::Png)
            .unwrap()
            .into_rgba8()
    };
    let managed = || {
        decode(
            &untagged,
            ImageFormat::Png,
            crate::MAX_CAPTURE_BYTES,
            None,
            &|| false,
        )
        .unwrap()
    };
    let p3 = || {
        decode(
            &tagged,
            ImageFormat::Png,
            crate::MAX_CAPTURE_BYTES,
            None,
            &|| false,
        )
        .unwrap()
    };
    fn sample(operation: impl Fn() -> RgbaImage) -> [f64; 2] {
        for _ in 0..3 {
            std::hint::black_box(operation());
        }
        let mut times = Vec::new();
        for _ in 0..15 {
            let begin = Instant::now();
            std::hint::black_box(operation());
            times.push(begin.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        [times[times.len() / 2], *times.last().unwrap()]
    }
    // Report rather than assert noisy timings; fixed CPU-image bytes are distinct from process RSS.
    println!(
        "managed_color_4k_report {{\"runs\":15,\"dimensions\":[3840,2160],\"rgba_bytes\":{},\"encoded_bytes\":{},\"legacy_png_median_max_ms\":{:?},\"managed_untagged_median_max_ms\":{:?},\"managed_p3_median_max_ms\":{:?}}}",
        3840u64 * 2160 * 4,
        untagged.len(),
        sample(baseline),
        sample(managed),
        sample(p3)
    );
}
