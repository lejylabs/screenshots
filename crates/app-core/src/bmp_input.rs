//! Immutable-snapshot admission for a narrow truecolor BMP subset.

use anyhow::{Context, Result, ensure};

pub(crate) fn validate_bmp(bytes: &[u8], budget: u64) -> Result<()> {
    ensure!(
        budget > 0 && budget <= crate::MAX_CAPTURE_BYTES,
        "Invalid BMP decoded image budget"
    );
    ensure!(
        bytes.len() <= 32 * 1024 * 1024 && bytes.len() >= 54,
        "BMP input must contain a complete header within 32 MiB"
    );
    ensure!(&bytes[..2] == b"BM", "Invalid BMP signature");
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let u16_at = |offset: usize| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
    ensure!(
        u32_at(2) as usize == bytes.len() && u32_at(6) == 0,
        "BMP file size or reserved fields are invalid"
    );
    let header = u32_at(14) as usize;
    ensure!(
        matches!(header, 40 | 56 | 108 | 124) && 14 + header <= bytes.len(),
        "Unsupported or truncated BMP header"
    );
    let width = i32::from_le_bytes(bytes[18..22].try_into().unwrap());
    let signed_height = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
    ensure!(
        width > 0 && width <= 65535 && signed_height != 0 && signed_height != i32::MIN,
        "Invalid BMP dimensions"
    );
    let height = signed_height.unsigned_abs();
    ensure!(
        height <= 65535,
        "BMP dimensions exceed the decoder side limit"
    );
    let width = width as u32;
    crate::validate_image_size(width, height)?;
    ensure!(
        u64::from(width) * u64::from(height) * 4 <= budget,
        "BMP dimensions exceed the decoded image budget"
    );
    ensure!(
        u16_at(26) == 1 && u32_at(46) == 0 && u32_at(50) == 0,
        "BMP planes or palette counts are unsupported"
    );
    let bits = u16_at(28);
    let compression = u32_at(30);
    if header == 40 {
        ensure!(
            compression == 0 && matches!(bits, 24 | 32),
            "BMP input requires uncompressed 24-bit or 32-bit truecolor pixels"
        );
    } else {
        ensure!(
            compression == 3 && bits == 32,
            "BMP extended headers require 32-bit bitfields"
        );
        ensure!(
            [u32_at(54), u32_at(58), u32_at(62), u32_at(66)]
                == [0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000],
            "BMP requires standard BGRA8 channel masks"
        );
        if header >= 108 {
            ensure!(
                u32_at(70) == 0x73524742 && bytes[74..122].iter().all(|byte| *byte == 0),
                "BMP requires sRGB without calibrated endpoints or gamma"
            );
        }
        if header == 124 {
            ensure!(
                u32_at(126) == 0 && u32_at(130) == 0 && u32_at(134) == 0,
                "BMP embedded or linked color profiles are unsupported"
            );
        }
    }
    let row = u64::from(width)
        .checked_mul(u64::from(bits))
        .context("BMP row size overflow")?
        .checked_add(31)
        .context("BMP row size overflow")?
        / 32
        * 4;
    let raster = row
        .checked_mul(u64::from(height))
        .context("BMP raster size overflow")?;
    let offset = u64::from(u32_at(10));
    ensure!(
        offset == (14 + header) as u64 && offset.checked_add(raster) == Some(bytes.len() as u64),
        "BMP pixel extent does not match its header and file size"
    );
    ensure!(
        u32_at(34) == 0 || u64::from(u32_at(34)) == raster,
        "BMP declared raster size is invalid"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgba, RgbaImage};

    fn rgba() -> (Vec<u8>, RgbaImage) {
        let image = RgbaImage::from_fn(3, 2, |x, y| {
            Rgba([x as u8 * 60, y as u8 * 90, 197, [0, 128, 255][x as usize]])
        });
        let mut bytes = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut bytes)
            .write_image(image.as_raw(), 3, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        (bytes, image)
    }
    fn set32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn truecolor(bits: u16, top_down: bool) -> Vec<u8> {
        let width = 3u32;
        let row = (width * u32::from(bits)).div_ceil(32) * 4;
        let size = 54 + row * 2;
        let mut bytes = vec![0; size as usize];
        bytes[..2].copy_from_slice(b"BM");
        set32(&mut bytes, 2, size);
        set32(&mut bytes, 10, 54);
        set32(&mut bytes, 14, 40);
        set32(&mut bytes, 18, width);
        set32(&mut bytes, 22, if top_down { (-2i32) as u32 } else { 2 });
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&bits.to_le_bytes());
        for y in 0..2usize {
            for x in 0..3usize {
                let at = 54 + y * row as usize + x * usize::from(bits / 8);
                bytes[at..at + 3].copy_from_slice(&[10 + x as u8, 20 + y as u8, 30]);
                if bits == 32 {
                    bytes[at + 3] = 17;
                }
            }
        }
        bytes
    }

    #[test]
    fn exported_v4_preserves_alpha_and_truecolor_rows_decode_correctly() {
        let (bytes, pixels) = rgba();
        validate_bmp(&bytes, 24).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().into_rgba8(),
            pixels
        );
        for bits in [24, 32] {
            for top_down in [false, true] {
                let bytes = truecolor(bits, top_down);
                validate_bmp(&bytes, 24).unwrap();
                let image = image::load_from_memory(&bytes).unwrap().into_rgba8();
                for y in 0..2 {
                    for x in 0..3 {
                        assert_eq!(
                            *image.get_pixel(x, y),
                            Rgba([
                                30,
                                20 + if top_down { y as u8 } else { 1 - y as u8 },
                                10 + x as u8,
                                255
                            ])
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn v3_and_v5_standard_masks_are_admitted_without_profiles() {
        let (base, pixels) = rgba();
        for header in [56usize, 124] {
            let mut bytes = base[..14 + 56].to_vec();
            bytes.resize(14 + header, 0);
            set32(&mut bytes, 14, header as u32);
            if header >= 108 {
                set32(&mut bytes, 70, 0x73524742);
            }
            bytes.extend_from_slice(&base[122..]);
            let len = bytes.len() as u32;
            set32(&mut bytes, 2, len);
            set32(&mut bytes, 10, (14 + header) as u32);
            validate_bmp(&bytes, 24).unwrap();
            assert_eq!(
                image::load_from_memory(&bytes).unwrap().into_rgba8(),
                pixels
            );
        }
    }

    #[test]
    fn malformed_extents_masks_profiles_and_dimensions_are_rejected() {
        let (base, _) = rgba();
        for (offset, value) in [
            (2, 1),
            (6, 1),
            (10, 121),
            (14, 12),
            (18, 0),
            (18, u32::MAX),
            (18, 65536),
            (22, i32::MIN as u32),
            (22, (-65536i32) as u32),
            (30, 1),
            (34, 1),
            (46, 1),
            (50, 1),
            (54, 0),
            (58, 0x00ff0000),
            (66, 0),
            (70, 0),
            (74, 1),
        ] {
            let mut bytes = base.clone();
            set32(&mut bytes, offset, value);
            assert!(
                validate_bmp(&bytes, crate::MAX_CAPTURE_BYTES).is_err(),
                "{offset}:{value}"
            );
        }
        for size in 0..base.len() {
            assert!(validate_bmp(&base[..size], crate::MAX_CAPTURE_BYTES).is_err());
        }
        assert!(validate_bmp(&base, 23).is_err());
        assert!(validate_bmp(&base, 0).is_err());
        assert!(validate_bmp(&base, crate::MAX_CAPTURE_BYTES + 1).is_err());
        let mut trailing = base.clone();
        trailing.push(0);
        let size = trailing.len() as u32;
        set32(&mut trailing, 2, size);
        assert!(validate_bmp(&trailing, 24).is_err());
        let mut v5 = base[..122].to_vec();
        v5.resize(138, 0);
        v5.extend_from_slice(&base[122..]);
        set32(&mut v5, 14, 124);
        set32(&mut v5, 10, 138);
        let size = v5.len() as u32;
        set32(&mut v5, 2, size);
        for offset in [126, 130, 134] {
            let mut invalid = v5.clone();
            set32(&mut invalid, offset, 1);
            assert!(validate_bmp(&invalid, 24).is_err());
        }
    }
}
