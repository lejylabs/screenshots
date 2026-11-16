//! Checked CGImage byte-order/alpha decoding, also tested without a macOS framework.
use crate::validate_image_size;
use anyhow::{Result, ensure};

pub(crate) const MAX_VIDEO_PROFILE_BYTES: usize = 4 * 1024 * 1024;

/// CoreGraphics owns profile validity/conversion; this admits its copied RGB
/// metadata without inferring primaries from a transfer-function label alone.
pub(crate) fn validate_video_colour(transfer: Option<&str>, profile: Option<&[u8]>) -> Result<()> {
    let profile =
        profile.ok_or_else(|| anyhow::anyhow!("Native video has no explicit RGB color profile"))?;
    ensure!(
        (132..=MAX_VIDEO_PROFILE_BYTES).contains(&profile.len())
            && &profile[16..20] == b"RGB "
            && &profile[36..40] == b"acsp"
            && usize::try_from(u32::from_be_bytes(profile[..4].try_into().unwrap()))?
                == profile.len(),
        "Native RGB color profile is missing, malformed or exceeds its metadata bound"
    );
    ensure!(
        transfer.is_none_or(|value| [
            "sRGB",
            "ITU_R_709_2",
            "SMPTE_240M_1995",
            "UseGamma",
            "Linear"
        ]
        .contains(&value)),
        "HDR/unknown native transfer function is unsupported"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub order: u32,
    pub alpha: u32,
}
impl Layout {
    pub fn validate(self, length: usize) -> Result<()> {
        validate_image_size(self.width, self.height)?;
        ensure!(
            matches!(self.order, 0x2000 | 0x4000) && matches!(self.alpha, 1..=6),
            "Unsupported native CGImage byte order/alpha"
        );
        let row = self.width as usize * 4;
        let extent = self
            .stride
            .checked_mul(self.height as usize)
            .filter(|extent| *extent <= crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024);
        ensure!(
            self.stride >= row
                && extent.is_some_and(|extent| extent <= length)
                && length <= crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024,
            "Invalid/oversized native CGImage backing"
        );
        Ok(())
    }
    pub fn decode(self, bytes: &[u8]) -> Result<image::RgbaImage> {
        self.validate(bytes.len())?;
        let mut output = image::RgbaImage::new(self.width, self.height);
        let target: &mut [u8] = output.as_mut();
        for (source, target) in bytes
            .chunks(self.stride)
            .take(self.height as usize)
            .zip(target.chunks_mut(self.width as usize * 4))
        {
            for (pixel, target) in source[..self.width as usize * 4]
                .chunks_exact(4)
                .zip(target.chunks_exact_mut(4))
            {
                let (mut rgb, a) = match (self.order, self.alpha) {
                    (0x2000, 2 | 4 | 6) => ([pixel[2], pixel[1], pixel[0]], pixel[3]),
                    (0x4000, 2 | 4 | 6) => ([pixel[1], pixel[2], pixel[3]], pixel[0]),
                    (0x2000, 1 | 3 | 5) => ([pixel[3], pixel[2], pixel[1]], pixel[0]),
                    (0x4000, 1 | 3 | 5) => ([pixel[0], pixel[1], pixel[2]], pixel[3]),
                    _ => unreachable!(),
                };
                let a = if matches!(self.alpha, 5 | 6) { 255 } else { a };
                if matches!(self.alpha, 1 | 2) {
                    for channel in &mut rgb {
                        *channel = if a == 0 {
                            0
                        } else {
                            ((u32::from(*channel) * 255 + u32::from(a) / 2) / u32::from(a)).min(255)
                                as u8
                        };
                    }
                }
                target.copy_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
            }
        }
        Ok(output)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_colour_never_infers_a_profile_from_transfer_or_pixel_format() {
        for transfer in [None, Some("sRGB"), Some("Linear")] {
            assert!(validate_video_colour(transfer, None).is_err());
            assert!(validate_video_colour(transfer, Some(&[])).is_err());
        }
        // A metadata fixture tests admission only; native tests use real profiles.
        let mut profile = vec![0u8; 132];
        profile[..4].copy_from_slice(&132u32.to_be_bytes());
        profile[16..20].copy_from_slice(b"RGB ");
        profile[36..40].copy_from_slice(b"acsp");
        for transfer in [None, Some("sRGB"), Some("ITU_R_709_2"), Some("Linear")] {
            validate_video_colour(transfer, Some(&profile)).unwrap();
        }
        for transfer in ["", "PQ", "HLG", "SMPTE_ST_2084_PQ", "Unknown"] {
            assert!(validate_video_colour(Some(transfer), Some(&profile)).is_err());
        }
        for changed in [0, 16, 36] {
            let mut invalid = profile.clone();
            invalid[changed] ^= 1;
            assert!(validate_video_colour(None, Some(&invalid)).is_err());
        }
        assert!(validate_video_colour(None, Some(&profile[..131])).is_err());
        let mut excessive = vec![0u8; MAX_VIDEO_PROFILE_BYTES + 1];
        excessive[..4].copy_from_slice(&((MAX_VIDEO_PROFILE_BYTES + 1) as u32).to_be_bytes());
        excessive[16..40].copy_from_slice(&profile[16..40]);
        assert!(validate_video_colour(None, Some(&excessive)).is_err());
    }
    #[test]
    fn packed_orders_skip_alpha_padding_and_straight_hidden_rgb_are_exact() {
        for order in [0x2000, 0x4000] {
            for alpha in [3, 4, 5, 6] {
                let pixel = match (order, alpha) {
                    (0x2000, 4 | 6) => [23, 17, 11, 0],
                    (0x4000, 4 | 6) => [0, 11, 17, 23],
                    (0x2000, 3 | 5) => [0, 23, 17, 11],
                    _ => [11, 17, 23, 0],
                };
                let mut bytes = vec![199u8; 24];
                bytes[..4].copy_from_slice(&pixel);
                bytes[12..16].copy_from_slice(&pixel);
                let layout = Layout {
                    width: 1,
                    height: 2,
                    stride: 12,
                    order,
                    alpha,
                };
                let result = layout.decode(&bytes).unwrap();
                let a = if alpha >= 5 { 255 } else { 0 };
                assert_eq!(result.as_raw(), &[11, 17, 23, a, 11, 17, 23, a]);
            }
        }
    }
    #[test]
    fn premultiplied_native_alpha_is_expanded_without_guessing() {
        let layout = Layout {
            width: 2,
            height: 1,
            stride: 8,
            order: 0x2000,
            alpha: 2,
        };
        assert_eq!(
            layout
                .decode(&[8, 16, 32, 64, 0, 0, 0, 0])
                .unwrap()
                .as_raw(),
            &[128, 64, 32, 64, 0, 0, 0, 0]
        );
    }
    #[test]
    fn unsupported_order_alpha_shape_and_extent_fail_before_output() {
        let valid = Layout {
            width: 1,
            height: 1,
            stride: 4,
            order: 0x2000,
            alpha: 2,
        };
        for layout in [
            Layout { stride: 3, ..valid },
            Layout { order: 0, ..valid },
            Layout { alpha: 7, ..valid },
            Layout {
                width: u32::MAX,
                ..valid
            },
            Layout {
                stride: usize::MAX,
                ..valid
            },
        ] {
            assert!(layout.validate(4).is_err());
        }
        assert!(valid.validate(3).is_err());
        assert!(
            valid
                .validate(crate::MAX_CAPTURE_BYTES as usize + 32 * 1024 * 1024 + 1)
                .is_err()
        );
    }
}
