//! Still capture output is straight RGBA8 sRGB; HDR is explicitly compressed to SDR.
#[cfg(any(target_os = "macos", test))]
use crate::{image_cancel::ImageWork, validate_image_size};
use anyhow::{Result, ensure};

/// Native source + float color context + output must fit the PLAN active budget.
/// The actual native stride, rather than RGBA8 output size, is the source charge.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn float_admission(width: u32, height: u32, source_bytes: usize) -> Result<()> {
    validate_image_size(width, height)?;
    let output = u64::from(width) * u64::from(height) * 4;
    let source = u64::try_from(source_bytes)?;
    ensure!(
        source > 0 && source <= crate::MAX_CAPTURE_BYTES,
        "Native color source exceeds decoded memory admission"
    );
    ensure!(
        source + 5 * output <= 2 * source + 2 * output + 32 * 1024 * 1024,
        "Native float color conversion exceeds active-image memory admission; choose a smaller source"
    );
    Ok(())
}

/// IEC 61966-2-1 output encoding, after explicit linear gamut clipping.
fn encode(value: f64) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let value = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (value * 255.0 + 0.5).floor() as u8
}

/// Relative display-linear sRGB, where 1 is source reference white. Preserve
/// luminance <= .75; use a continuous unit-slope shoulder above .75, tending to 1.
/// This is a declared fixed SDR rendition, never HDR preservation or BT.2390.
pub(crate) fn linear_pixel(rgba: [f32; 4], hdr: bool) -> Result<[u8; 4]> {
    ensure!(
        rgba.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&rgba[3]),
        "Invalid native floating color component"
    );
    let alpha = f64::from(rgba[3]);
    if alpha == 0.0 {
        return Ok([0; 4]);
    }
    // Both native adapters supply premultiplied linear components.
    let mut rgb = [rgba[0], rgba[1], rgba[2]].map(|v| f64::from(v) / alpha);
    let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    if hdr && y > 0.75 {
        let excess = y - 0.75;
        let scale = (0.75 + excess / (1.0 + 4.0 * excess)) / y;
        for value in &mut rgb {
            *value *= scale;
        }
    }
    Ok([
        encode(rgb[0]),
        encode(rgb[1]),
        encode(rgb[2]),
        (alpha * 255.0 + 0.5).floor() as u8,
    ])
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn linear_rgba_to_sdr(
    width: u32,
    height: u32,
    pixels: &[f32],
    hdr: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<image::RgbaImage> {
    validate_image_size(width, height)?;
    ensure!(
        pixels.len() == width as usize * height as usize * 4,
        "Invalid native linear color extent"
    );
    let mut work = ImageWork::new(cancelled)?;
    // Extended containers also carry ordinary SDR. Only apply the HDR shoulder
    // when this frame actually exceeds reference white (with float noise slack).
    let mut extended_content = false;
    if hdr {
        for pixel in pixels.chunks_exact(4) {
            work.charge(16)?;
            let alpha = f64::from(pixel[3]);
            if alpha > 0.0
                && (0.2126 * f64::from(pixel[0])
                    + 0.7152 * f64::from(pixel[1])
                    + 0.0722 * f64::from(pixel[2]))
                    / alpha
                    > 1.0 + 1.0 / 1024.0
            {
                extended_content = true;
                break;
            }
        }
    }
    let hdr = hdr && extended_content;
    let mut output = image::RgbaImage::new(width, height);
    let target: &mut [u8] = output.as_mut();
    for (source, target) in pixels.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
        work.charge(16)?;
        target.copy_from_slice(&linear_pixel(source.try_into().unwrap(), hdr)?);
    }
    work.check().map_err(anyhow::Error::from)?;
    Ok(output)
}

/// IEEE 754 binary16 including subnormals, signed zero and non-finite values.
/// Invalid values are rejected by linear_pixel rather than silently quantized.
pub(crate) fn half(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 31);
    let fraction = u32::from(bits & 1023);
    let value = match (exponent, fraction) {
        (0, 0) => sign,
        (0, mut fraction) => {
            let mut exponent = 113u32;
            while fraction & 1024 == 0 {
                fraction <<= 1;
                exponent -= 1;
            }
            sign | (exponent << 23) | ((fraction & 1023) << 13)
        }
        (31, _) => sign | 0x7f80_0000 | (fraction << 13),
        _ => sign | ((exponent + 112) << 23) | (fraction << 13),
    };
    f32::from_bits(value)
}

/// Locked/mapped little-endian RGBA16 float with explicit scRGB reference white.
/// Decode directly into output: no additional image-sized float expansion.
#[cfg(test)]
pub(crate) fn half_rgba_to_sdr(
    width: u32,
    height: u32,
    stride: usize,
    bytes: &[u8],
    hdr: bool,
    white_level: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<image::RgbaImage> {
    validate_image_size(width, height)?;
    ensure!(
        (1000..=100_000).contains(&white_level)
            && stride >= width as usize * 8
            && stride
                .checked_mul(height as usize)
                .is_some_and(|extent| extent <= bytes.len())
            && bytes.len() <= crate::MAX_CAPTURE_BYTES as usize,
        "Invalid native float color extent"
    );
    let mut work = ImageWork::new(cancelled)?;
    let mut output = image::RgbaImage::new(width, height);
    let target: &mut [u8] = output.as_mut();
    let white = white_level as f32 / 1000.0;
    let mut extended_content = false;
    if hdr {
        'rows: for source in bytes.chunks(stride).take(height as usize) {
            for pixel in source[..width as usize * 8].chunks_exact(8) {
                work.charge(8)?;
                let pixel = decode_half_pixel(pixel, white);
                let alpha = f64::from(pixel[3]);
                if alpha > 0.0
                    && (0.2126 * f64::from(pixel[0])
                        + 0.7152 * f64::from(pixel[1])
                        + 0.0722 * f64::from(pixel[2]))
                        / alpha
                        > 1.0 + 1.0 / 1024.0
                {
                    extended_content = true;
                    break 'rows;
                }
            }
        }
    }
    let hdr = hdr && extended_content;
    for (source, target) in bytes
        .chunks(stride)
        .take(height as usize)
        .zip(target.chunks_mut(width as usize * 4))
    {
        for (pixel, target) in source[..width as usize * 8]
            .chunks_exact(8)
            .zip(target.chunks_exact_mut(4))
        {
            work.charge(8)?;
            let rgba = decode_half_pixel(pixel, white);
            target.copy_from_slice(&linear_pixel(rgba, hdr)?);
        }
    }
    work.check()?;
    Ok(output)
}

#[cfg(any(target_os = "windows", test))]
fn decode_half_pixel(pixel: &[u8], white: f32) -> [f32; 4] {
    let mut rgba = [0.0; 4];
    for channel in 0..4 {
        rgba[channel] = half(u16::from_le_bytes([
            pixel[channel * 2],
            pixel[channel * 2 + 1],
        ]));
    }
    for channel in &mut rgba[..3] {
        *channel /= white;
    }
    rgba
}

#[cfg(any(target_os = "windows", test))]
pub(crate) struct HalfRows<'a> {
    width: u32,
    height: u32,
    stride: usize,
    bytes: &'a [u8],
    white: f32,
}
#[cfg(any(target_os = "windows", test))]
impl<'a> HalfRows<'a> {
    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        bytes: &'a [u8],
        white: u32,
    ) -> std::result::Result<Self, crate::CaptureError> {
        crate::validate_image_size(width, height)?;
        let row = width as usize * 8;
        let charge = stride
            .checked_mul(height as usize)
            .ok_or(crate::CaptureError::MemoryLimit)?;
        let needed = (height as usize - 1)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(row))
            .ok_or(crate::CaptureError::MemoryLimit)?;
        if !(1000..=100_000).contains(&white)
            || stride < row
            || charge > crate::MAX_CAPTURE_BYTES as usize
            || bytes.len() > crate::MAX_CAPTURE_BYTES as usize
            || needed > bytes.len()
        {
            return Err(crate::CaptureError::MemoryLimit);
        }
        Ok(Self {
            width,
            height,
            stride,
            bytes,
            white: white as f32 / 1000.,
        })
    }
    /// No output allocation; the caller combines these decisions over one held frame.
    pub fn has_hdr(
        &self,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> std::result::Result<bool, crate::CaptureError> {
        check()?;
        for row in self.bytes.chunks(self.stride).take(self.height as usize) {
            check()?;
            for block in row[..self.width as usize * 8].chunks(64 * 1024) {
                check()?;
                for pixel in block.chunks_exact(8) {
                    let p = decode_half_pixel(pixel, self.white);
                    if p.iter().any(|v| !v.is_finite()) || !(0.0..=1.0).contains(&p[3]) {
                        return Err(crate::CaptureError::Unsupported(
                            "Invalid native floating color component",
                        ));
                    }
                    if p[3] > 0.
                        && (0.2126 * f64::from(p[0])
                            + 0.7152 * f64::from(p[1])
                            + 0.0722 * f64::from(p[2]))
                            / f64::from(p[3])
                            > 1. + 1. / 1024.
                    {
                        check()?;
                        return Ok(true);
                    }
                }
            }
        }
        check()?;
        Ok(false)
    }
    /// Write only copied pixels into a private final raster; no temporary RGBA tile.
    pub fn write_sdr(
        &self,
        hdr: bool,
        target: &mut [u8],
        target_stride: usize,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> std::result::Result<(), crate::CaptureError> {
        let row = self.width as usize * 4;
        let needed = (self.height as usize - 1)
            .checked_mul(target_stride)
            .and_then(|v| v.checked_add(row))
            .ok_or(crate::CaptureError::MemoryLimit)?;
        if target_stride < row
            || target.len() > crate::MAX_CAPTURE_BYTES as usize
            || needed > target.len()
        {
            return Err(crate::CaptureError::MemoryLimit);
        }
        check()?;
        for (source, target) in self
            .bytes
            .chunks(self.stride)
            .take(self.height as usize)
            .zip(target.chunks_mut(target_stride))
        {
            check()?;
            for (block, out) in source[..self.width as usize * 8]
                .chunks(64 * 1024)
                .zip(target[..row].chunks_mut(32 * 1024))
            {
                check()?;
                for (pixel, out) in block.chunks_exact(8).zip(out.chunks_exact_mut(4)) {
                    out.copy_from_slice(
                        &linear_pixel(decode_half_pixel(pixel, self.white), hdr).map_err(|_| {
                            crate::CaptureError::Unsupported(
                                "Invalid native floating color component",
                            )
                        })?,
                    );
                }
            }
        }
        check()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tile_rows_preserve_target_gaps_allow_no_trailing_padding_and_validate_before_write() {
        let words = [
            0x3800u16, 0x3400, 0x3000, 0x3c00, 0x3c00, 0x3c00, 0x3c00, 0x3c00,
        ];
        let row = words
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut bytes = row.clone();
        bytes.extend([199; 4]);
        bytes.extend(&row);
        let source = HalfRows::new(2, 2, 20, &bytes, 1000).unwrap();
        let mut target = [199u8; 24];
        source
            .write_sdr(false, &mut target, 16, &|| Ok(()))
            .unwrap();
        assert_eq!(&target[..8], &[188, 137, 99, 255, 255, 255, 255, 255]);
        assert_eq!(&target[8..16], &[199; 8]);
        assert_eq!(&target[16..], &target[..8]);
        let mut short = [199u8; 23];
        assert!(source.write_sdr(false, &mut short, 16, &|| Ok(())).is_err());
        assert_eq!(short, [199; 23]);
        assert!(HalfRows::new(2, 2, 15, &bytes, 1000).is_err());
        assert!(HalfRows::new(2, 2, usize::MAX, &bytes, 1000).is_err());
        assert!(HalfRows::new(2, 2, 20, &bytes[..35], 1000).is_err());
    }
    #[test]
    fn conversion_checks_late_nonfinite_and_row_cancellation_after_scan_activation() {
        let mut bytes = [0u8; 16];
        for (i, v) in [0x4000u16, 0x4000, 0x4000, 0x3c00, 0x7e01, 0, 0, 0x3c00]
            .into_iter()
            .enumerate()
        {
            bytes[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        let rows = HalfRows::new(2, 1, 16, &bytes, 1000).unwrap();
        assert!(rows.has_hdr(&|| Ok(())).unwrap());
        assert!(rows.write_sdr(true, &mut [0; 8], 8, &|| Ok(())).is_err());
        let data = vec![0u8; 4 * 8];
        let rows = HalfRows::new(1, 4, 8, &data, 1000).unwrap();
        let calls = std::cell::Cell::new(0);
        let result = rows.write_sdr(false, &mut [0; 16], 4, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 6 {
                Err(crate::CaptureError::NativeCanceled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(crate::CaptureError::NativeCanceled)));
    }
    #[test]
    fn mapped_half_rows_match_independent_binary16_decimal_oracle() {
        type Case = (u32, u32, usize, Vec<u8>, bool, u32, Vec<u8>);
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../tests/fixtures/color/capture-half-oracle.json"
        ))
        .unwrap();
        for (width, height, stride, source, hdr, white, expected) in cases {
            assert_eq!(
                half_rgba_to_sdr(width, height, stride, &source, hdr, white, &|| false)
                    .unwrap()
                    .as_raw(),
                &expected
            );
        }
    }
    #[test]
    fn half_rows_refuse_invalid_extent_metadata_nonfinite_and_cancellation() {
        for (stride, bytes, white) in [
            (7, vec![0; 8], 1000),
            (8, vec![0; 7], 1000),
            (usize::MAX, vec![0; 8], 1000),
            (8, vec![0; 8], 0),
            (8, vec![0; 8], 100001),
        ] {
            assert!(half_rgba_to_sdr(1, 1, stride, &bytes, true, white, &|| false).is_err());
        }
        assert!(half_rgba_to_sdr(1, 1, 8, &[0; 8], true, 1000, &|| true).is_err());
        assert!(half_rgba_to_sdr(1, 2, usize::MAX, &[], true, 1000, &|| false).is_err());
        assert!(
            half_rgba_to_sdr(
                1,
                1,
                8,
                &[0, 0x7c, 0, 0, 0, 0, 0, 0x3c],
                true,
                1000,
                &|| false
            )
            .is_err()
        );
        assert!(half_rgba_to_sdr(1, 1, 8, &[0; 8], true, 1000, &|| false).is_ok());
    }
    #[test]
    fn every_binary16_encoding_matches_independent_python_decoder() {
        use sha2_10::{Digest, Sha256};
        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/color/capture-half-decoder-oracle.json"
        ))
        .unwrap();
        let mut hash = Sha256::new();
        let mut count = 0u64;
        for bits in 0..=u16::MAX {
            if bits & 0x7c00 == 0x7c00 && bits & 1023 != 0 {
                assert!(half(bits).is_nan());
            } else {
                hash.update(half(bits).to_le_bytes());
                count += 1;
            }
        }
        assert_eq!(count, oracle["non_nan_encodings"].as_u64().unwrap());
        assert_eq!(
            format!("{:x}", hash.finalize()),
            oracle["f32_le_sha256"].as_str().unwrap()
        );
    }
    #[test]
    fn extended_container_without_hdr_luminance_preserves_sdr_white() {
        let sdr = [1.0, 1.0, 1.0, 1.0, 0.5, 0.25, 0.125, 1.0];
        assert_eq!(
            linear_rgba_to_sdr(2, 1, &sdr, true, &|| false).unwrap(),
            linear_rgba_to_sdr(2, 1, &sdr, false, &|| false).unwrap()
        );
        let hdr = [1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 1.0];
        assert_eq!(
            linear_rgba_to_sdr(2, 1, &hdr, true, &|| false)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [240, 240, 240, 255]
        );
    }
    #[test]
    fn fixed_policy_matches_independent_decimal_oracle() {
        let cases: Vec<([f32; 4], bool, [u8; 4])> = serde_json::from_str(include_str!(
            "../tests/fixtures/color/capture-hdr-oracle.json"
        ))
        .unwrap();
        for (input, hdr, expected) in cases {
            assert_eq!(
                linear_pixel(input, hdr).unwrap(),
                expected,
                "{input:?} hdr={hdr}"
            );
        }
    }
    #[test]
    fn invalid_components_alpha_memory_and_cancellation_fail() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(linear_pixel([value, 0.0, 0.0, 1.0], true).is_err());
        }
        for alpha in [-0.1, 1.1, f32::NAN] {
            assert!(linear_pixel([0.0, 0.0, 0.0, alpha], true).is_err());
        }
        assert_eq!(linear_pixel([12.5, 1.0, -3.0, 0.0], true).unwrap(), [0; 4]);
        assert!(
            linear_rgba_to_sdr(1, 1, &[0.0; 4], true, &|| true)
                .unwrap_err()
                .is::<crate::CaptureError>()
        );
        float_admission(3840, 2160, 3840 * 2160 * 8).unwrap();
        assert!(float_admission(4096, 2160, 4096 * 2160 * 8).is_err());
        // A tiny native crop still charges the full original backing, while
        // its float context and output contain only selected pixels.
        float_admission(1, 1, 3840 * 2160 * 8).unwrap();
        assert!(float_admission(1, 1, crate::MAX_CAPTURE_BYTES as usize + 1).is_err());
    }
    #[test]
    fn half_float_and_hdr_shoulder_preserve_precision_and_order() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0x4a40), 12.5);
        assert_eq!(half(1), 2.0f32.powi(-24));
        assert_eq!(half(0x8001), -2.0f32.powi(-24));
        assert!(half(0x7c00).is_infinite());
        assert!(half(0x7e00).is_nan());
        let mut previous = 0;
        for i in 0..=10000 {
            let y = i as f32 / 1000.0;
            let pixel = linear_pixel([y, y, y, 1.0], true).unwrap();
            assert!(pixel[0] >= previous);
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            previous = pixel[0];
        }
    }
}
