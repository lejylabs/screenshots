//! Tagged packed native RGB -> normalized SDR sRGB, without float image expansion.
use crate::{CaptureError, capture_color, image_cancel::ImageWork, validate_image_size};
use moxcms::{ColorPrimaries, Matrix3d, TransferCharacteristics, Vector3d, WHITE_POINT_D65};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRgbPacking {
    Rgba8,
    Bgra8,
    Rgbx8,
    Bgrx8,
    RgbaF16Le,
    RgbaF32Le,
    Xrgb210Le,
    Xbgr210Le,
    Rgbx102Le,
    Bgrx102Le,
    Argb210Le,
    Abgr210Le,
    Rgba102Le,
    Bgra102Le,
}
impl NativeRgbPacking {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::RgbaF16Le => 8,
            Self::RgbaF32Le => 16,
            _ => 4,
        }
    }
    fn pixel(self, bytes: &[u8]) -> [f64; 4] {
        use NativeRgbPacking::{
            Abgr210Le, Argb210Le, Bgra8, Bgra102Le, Bgrx8, Bgrx102Le, Rgba8, Rgba102Le, RgbaF16Le,
            RgbaF32Le, Rgbx8, Rgbx102Le, Xbgr210Le,
        };
        match self {
            RgbaF16Le => std::array::from_fn(|i| {
                f64::from(capture_color::half(u16::from_le_bytes(
                    bytes[i * 2..i * 2 + 2].try_into().unwrap(),
                )))
            }),
            RgbaF32Le => std::array::from_fn(|i| {
                f64::from(f32::from_le_bytes(
                    bytes[i * 4..i * 4 + 4].try_into().unwrap(),
                ))
            }),
            Rgba8 | Bgra8 | Rgbx8 | Bgrx8 => {
                let bgr = matches!(self, Bgra8 | Bgrx8);
                [
                    f64::from(bytes[if bgr { 2 } else { 0 }]) / 255.,
                    f64::from(bytes[1]) / 255.,
                    f64::from(bytes[if bgr { 0 } else { 2 }]) / 255.,
                    if matches!(self, Rgba8 | Bgra8) {
                        f64::from(bytes[3]) / 255.
                    } else {
                        1.
                    },
                ]
            }
            _ => {
                let word = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                let low_alpha = matches!(self, Rgbx102Le | Bgrx102Le | Rgba102Le | Bgra102Le);
                let bgr = matches!(self, Xbgr210Le | Bgrx102Le | Abgr210Le | Bgra102Le);
                let shift = if low_alpha { 2 } else { 0 };
                let rgb = [
                    (word >> (shift + 20)) & 1023,
                    (word >> (shift + 10)) & 1023,
                    (word >> shift) & 1023,
                ];
                let alpha = if matches!(self, Rgba102Le | Bgra102Le) {
                    f64::from(word & 3) / 3.
                } else if matches!(self, Argb210Le | Abgr210Le) {
                    f64::from(word >> 30) / 3.
                } else {
                    1.
                };
                [
                    f64::from(rgb[if bgr { 2 } else { 0 }]) / 1023.,
                    f64::from(rgb[1]) / 1023.,
                    f64::from(rgb[if bgr { 0 } else { 2 }]) / 1023.,
                    alpha,
                ]
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRgbTransfer {
    Linear,
    Srgb,
    Bt709,
    Bt2020Twelve,
    Gamma22,
    AdobeRgb,
    Pq,
    Hlg,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRgbPrimaries {
    Bt709,
    DisplayP3,
    Bt2020,
    AdobeRgb,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRgbColor {
    pub transfer: NativeRgbTransfer,
    pub primaries: NativeRgbPrimaries,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRgbSource {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub packing: NativeRgbPacking,
    pub premultiplied: bool,
    /// Required when the native transport cannot carry alpha-representation proof.
    #[serde(default)]
    pub opaque_alpha_only: bool,
    pub color: NativeRgbColor,
}
fn policy() -> CaptureError {
    CaptureError::Unsupported("Native RGB color metadata, precision or components are unsupported")
}
impl NativeRgbSource {
    fn validate(self, length: usize) -> Result<(), CaptureError> {
        validate_image_size(self.width, self.height)?;
        let row = self.width as usize * self.packing.bytes_per_pixel();
        let extent = (self.height as usize - 1)
            .checked_mul(self.stride)
            .and_then(|v| v.checked_add(row))
            .ok_or(CaptureError::MemoryLimit)?;
        let logical =
            u64::from(self.width) * u64::from(self.height) * self.packing.bytes_per_pixel() as u64;
        if self.stride < row
            || logical > crate::MAX_CAPTURE_BYTES
            || length > crate::MAX_CAPTURE_BYTES as usize
            || extent > length
        {
            return Err(CaptureError::MemoryLimit);
        }
        if matches!(
            self.color.transfer,
            NativeRgbTransfer::Pq | NativeRgbTransfer::Hlg
        ) && matches!(
            self.packing,
            NativeRgbPacking::Rgba8
                | NativeRgbPacking::Bgra8
                | NativeRgbPacking::Rgbx8
                | NativeRgbPacking::Bgrx8
        ) {
            return Err(policy());
        }
        Ok(())
    }
}
struct Conversion {
    source: NativeRgbSource,
    matrix: Matrix3d,
    luminance: [f64; 3],
}
impl Conversion {
    fn new(source: NativeRgbSource) -> Self {
        let primaries = match source.color.primaries {
            NativeRgbPrimaries::Bt709 => ColorPrimaries::BT_709,
            NativeRgbPrimaries::DisplayP3 => ColorPrimaries::DISPLAY_P3,
            NativeRgbPrimaries::Bt2020 => ColorPrimaries::BT_2020,
            NativeRgbPrimaries::AdobeRgb => ColorPrimaries::ADOBE_RGB,
        };
        // Reuse the managed-color dependency's D65 chromaticities/matrix algebra.
        let xyz = primaries.transform_to_xyz_d(WHITE_POINT_D65);
        let srgb = ColorPrimaries::BT_709.transform_to_xyz_d(WHITE_POINT_D65);
        Self {
            source,
            matrix: srgb.inverse() * xyz,
            luminance: xyz.v[1],
        }
    }
    fn linear(&self, bytes: &[u8]) -> Result<[f32; 4], CaptureError> {
        let rgba = self.source.packing.pixel(bytes);
        if rgba.iter().any(|v| !v.is_finite()) || !(0.0..=1.0).contains(&rgba[3]) {
            return Err(policy());
        }
        let a = rgba[3];
        if self.source.opaque_alpha_only && a != 0. && a != 1. {
            return Err(policy());
        }
        if a == 0. {
            return Ok([0.; 4]);
        }
        let mut rgb = [rgba[0], rgba[1], rgba[2]];
        if self.source.premultiplied {
            for c in &mut rgb {
                *c /= a;
            }
        }
        for c in &mut rgb {
            *c = match self.source.color.transfer {
                NativeRgbTransfer::Linear => *c,
                NativeRgbTransfer::Srgb => {
                    c.signum()
                        * if c.abs() <= 0.04045 {
                            c.abs() / 12.92
                        } else {
                            ((c.abs() + 0.055) / 1.055).powf(2.4)
                        }
                }
                NativeRgbTransfer::Bt709 => {
                    c.signum()
                        * if c.abs() < 0.081 {
                            c.abs() / 4.5
                        } else {
                            ((c.abs() + 0.099) / 1.099).powf(1. / 0.45)
                        }
                }
                NativeRgbTransfer::Bt2020Twelve => {
                    c.signum()
                        * if c.abs() < 0.08145 {
                            c.abs() / 4.5
                        } else {
                            ((c.abs() + 0.0993) / 1.0993).powf(1. / 0.45)
                        }
                }
                NativeRgbTransfer::Gamma22 => c.signum() * c.abs().powf(2.2),
                NativeRgbTransfer::AdobeRgb => c.signum() * c.abs().powf(2.19921875),
                NativeRgbTransfer::Pq => {
                    if !(0.0..=1.0).contains(c) {
                        return Err(policy());
                    }
                    // Existing managed dependency's ST2084 EOTF is relative to 10000 nits.
                    TransferCharacteristics::Smpte2084.linearize(*c) * 10000. / 203.
                }
                NativeRgbTransfer::Hlg => {
                    if *c < 0. {
                        return Err(policy());
                    }
                    if *c <= 0.5 {
                        *c * *c / 3.
                    } else {
                        (((*c - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.
                    }
                }
            };
        }
        if self.source.color.transfer == NativeRgbTransfer::Hlg {
            // BT.2100 reference display: 1000-nit peak, gamma 1.2, black zero.
            // Apply the coupled luminance OOTF, not independent channel gamma.
            let y = rgb
                .iter()
                .zip(self.luminance)
                .map(|(v, w)| v * w)
                .sum::<f64>();
            let scale = if y > 0. {
                y.powf(0.2) * 1000. / 203.
            } else {
                0.
            };
            for c in &mut rgb {
                *c *= scale;
            }
        }
        let rgb = self.matrix.mul_vector(Vector3d { v: rgb }).v;
        let result = [
            (rgb[0] * a) as f32,
            (rgb[1] * a) as f32,
            (rgb[2] * a) as f32,
            a as f32,
        ];
        if result.iter().any(|v| !v.is_finite()) {
            return Err(policy());
        }
        Ok(result)
    }
}
/// At most one output allocation; at most two source scans and 64 KiB between
/// cancellation checks. There is no expanded float image, lookup cache or GPU work.
pub fn decode_native_rgb_with_cancel(
    source: NativeRgbSource,
    bytes: &[u8],
    cancelled: &impl Fn() -> bool,
) -> Result<image::RgbaImage, CaptureError> {
    source.validate(bytes.len())?;
    let mut work = ImageWork::new(cancelled)?;
    if source.color
        == (NativeRgbColor {
            transfer: NativeRgbTransfer::Srgb,
            primaries: NativeRgbPrimaries::Bt709,
        })
        && matches!(
            source.packing,
            NativeRgbPacking::Rgba8
                | NativeRgbPacking::Bgra8
                | NativeRgbPacking::Rgbx8
                | NativeRgbPacking::Bgrx8
        )
    {
        let mut output = image::RgbaImage::new(source.width, source.height);
        let target: &mut [u8] = output.as_mut();
        let bgr = matches!(
            source.packing,
            NativeRgbPacking::Bgra8 | NativeRgbPacking::Bgrx8
        );
        let alpha = matches!(
            source.packing,
            NativeRgbPacking::Rgba8 | NativeRgbPacking::Bgra8
        );
        for (row, target) in bytes
            .chunks(source.stride)
            .take(source.height as usize)
            .zip(target.chunks_mut(source.width as usize * 4))
        {
            for (p, out) in row[..source.width as usize * 4]
                .chunks_exact(4)
                .zip(target.chunks_exact_mut(4))
            {
                work.charge(4)?;
                let a = if alpha { p[3] } else { 255 };
                if source.opaque_alpha_only && a != 0 && a != 255 {
                    return Err(policy());
                }
                let expand = |v: u8| {
                    if !source.premultiplied || !alpha {
                        v
                    } else if a == 0 {
                        0
                    } else {
                        ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                    }
                };
                out.copy_from_slice(&[
                    expand(p[if bgr { 2 } else { 0 }]),
                    expand(p[1]),
                    expand(p[if bgr { 0 } else { 2 }]),
                    a,
                ]);
            }
        }
        work.check()?;
        return Ok(output);
    }
    let convert = Conversion::new(source);
    let bpp = source.packing.bytes_per_pixel();
    let hdr_candidate = matches!(
        source.packing,
        NativeRgbPacking::RgbaF16Le | NativeRgbPacking::RgbaF32Le
    ) || matches!(
        source.color.transfer,
        NativeRgbTransfer::Linear | NativeRgbTransfer::Pq | NativeRgbTransfer::Hlg
    );
    let mut hdr = false;
    if hdr_candidate {
        'rows: for row in bytes.chunks(source.stride).take(source.height as usize) {
            for pixel in row[..source.width as usize * bpp].chunks_exact(bpp) {
                work.charge(bpp)?;
                let p = convert.linear(pixel)?;
                if p[3] > 0.
                    && (0.2126 * f64::from(p[0])
                        + 0.7152 * f64::from(p[1])
                        + 0.0722 * f64::from(p[2]))
                        / f64::from(p[3])
                        > 1. + 1. / 1024.
                {
                    hdr = true;
                    break 'rows;
                }
            }
        }
    }
    let mut output = image::RgbaImage::new(source.width, source.height);
    let target: &mut [u8] = output.as_mut();
    for (row, target) in bytes
        .chunks(source.stride)
        .take(source.height as usize)
        .zip(target.chunks_mut(source.width as usize * 4))
    {
        for (pixel, target) in row[..source.width as usize * bpp]
            .chunks_exact(bpp)
            .zip(target.chunks_exact_mut(4))
        {
            work.charge(bpp)?;
            target.copy_from_slice(
                &capture_color::linear_pixel(convert.linear(pixel)?, hdr).map_err(|_| policy())?,
            );
        }
    }
    work.check()?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize)]
    struct Case {
        name: String,
        source: NativeRgbSource,
        bytes: Vec<u8>,
        rgba: Vec<u8>,
    }
    #[test]
    fn tagged_native_formats_match_independent_decimal_and_chromaticity_oracles() {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../tests/fixtures/color/native-rgb-oracle.json"
        ))
        .unwrap();
        for case in cases {
            let actual =
                decode_native_rgb_with_cancel(case.source, &case.bytes, &|| false).unwrap();
            assert!(
                actual
                    .as_raw()
                    .iter()
                    .zip(&case.rgba)
                    .enumerate()
                    .all(|(i, (a, b))| if i % 4 == 3 {
                        a == b
                    } else {
                        a.abs_diff(*b) <= 1
                    }),
                "{}",
                case.name
            );
            assert_eq!(actual.as_raw().len(), case.rgba.len());
            if case.source.color
                == (NativeRgbColor {
                    transfer: NativeRgbTransfer::Srgb,
                    primaries: NativeRgbPrimaries::Bt709,
                })
                && matches!(
                    case.source.packing,
                    NativeRgbPacking::Rgba8
                        | NativeRgbPacking::Bgra8
                        | NativeRgbPacking::Rgbx8
                        | NativeRgbPacking::Bgrx8
                )
            {
                assert_eq!(actual.as_raw(), &case.rgba, "exact SDR {}", case.name);
            }
        }
    }
    #[test]
    fn native_rgb_refuses_extent_nonfinite_and_cancel_without_partial_output() {
        let source = NativeRgbSource {
            width: 1,
            height: 1,
            stride: 16,
            packing: NativeRgbPacking::RgbaF32Le,
            premultiplied: false,
            opaque_alpha_only: false,
            color: NativeRgbColor {
                transfer: NativeRgbTransfer::Linear,
                primaries: NativeRgbPrimaries::Bt709,
            },
        };
        assert!(decode_native_rgb_with_cancel(source, &[0; 15], &|| false).is_err());
        assert!(decode_native_rgb_with_cancel(source, &[0; 16], &|| true).is_err());
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_native_rgb_with_cancel(source, &bytes, &|| false).is_err());
        assert!(
            decode_native_rgb_with_cancel(
                NativeRgbSource {
                    height: 2,
                    stride: usize::MAX,
                    ..source
                },
                &bytes,
                &|| false
            )
            .is_err()
        );
        assert!(
            decode_native_rgb_with_cancel(
                NativeRgbSource {
                    width: u32::MAX,
                    ..source
                },
                &bytes,
                &|| false
            )
            .is_err()
        );
        bytes[..4].copy_from_slice(&0.25f32.to_le_bytes());
        bytes[12..].copy_from_slice(&0.5f32.to_le_bytes());
        assert!(
            decode_native_rgb_with_cancel(
                NativeRgbSource {
                    opaque_alpha_only: true,
                    ..source
                },
                &bytes,
                &|| false
            )
            .is_err()
        );
        bytes[..4].copy_from_slice(&2f32.to_le_bytes());
        bytes[12..].copy_from_slice(&1f32.to_le_bytes());
        assert!(
            decode_native_rgb_with_cancel(
                NativeRgbSource {
                    color: NativeRgbColor {
                        transfer: NativeRgbTransfer::Pq,
                        ..source.color
                    },
                    ..source
                },
                &bytes,
                &|| false
            )
            .is_err()
        );
    }
}
