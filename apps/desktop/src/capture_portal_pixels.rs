//! Packed native RGB admission before constructing any slice or output raster.
use app_core::{
    CaptureError, NativeRgbColor, NativeRgbPacking, NativeRgbPrimaries, NativeRgbSource,
    NativeRgbTransfer, validate_image_size,
};
use pipewire::spa;
use spa::param::video::{VideoFormat, VideoInfoRaw};

pub(super) const MAX_MAPPED_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Format {
    pub width: u32,
    pub height: u32,
    pub bgr: bool,
    pub alpha: bool,
    pub premultiplied: bool,
    pub packing: NativeRgbPacking,
    pub color: Option<NativeRgbColor>,
    pub metadata: [u32; 4],
}
pub(super) fn unsupported() -> CaptureError {
    CaptureError::Unsupported(
        "Portal frame needs admitted packed RGB CPU memory and explicit supported color metadata",
    )
}
impl Format {
    pub fn parse(info: VideoInfoRaw) -> Result<Self, CaptureError> {
        let size = info.size();
        validate_image_size(size.width, size.height)?;
        // Reject modifier/unknown flags without enabling a newer optional SPA ABI.
        if info.flags().bits() & !3 != 0
            || info.interlace_mode().as_raw() != 0
            || info.views() > 1
            || info.multiview_mode() != 0
            || info.multiview_flags() != 0
        {
            return Err(unsupported());
        }
        let (packing, bgr, alpha) = match info.format() {
            VideoFormat::RGBx => (NativeRgbPacking::Rgbx8, false, false),
            VideoFormat::BGRx => (NativeRgbPacking::Bgrx8, true, false),
            VideoFormat::RGBA => (NativeRgbPacking::Rgba8, false, true),
            VideoFormat::BGRA => (NativeRgbPacking::Bgra8, true, true),
            VideoFormat::RGBA_F16 if cfg!(target_endian = "little") => {
                (NativeRgbPacking::RgbaF16Le, false, true)
            }
            VideoFormat::RGBA_F32 if cfg!(target_endian = "little") => {
                (NativeRgbPacking::RgbaF32Le, false, true)
            }
            VideoFormat::xRGB_210LE => (NativeRgbPacking::Xrgb210Le, false, false),
            VideoFormat::xBGR_210LE => (NativeRgbPacking::Xbgr210Le, true, false),
            VideoFormat::RGBx_102LE => (NativeRgbPacking::Rgbx102Le, false, false),
            VideoFormat::BGRx_102LE => (NativeRgbPacking::Bgrx102Le, true, false),
            VideoFormat::ARGB_210LE => (NativeRgbPacking::Argb210Le, false, true),
            VideoFormat::ABGR_210LE => (NativeRgbPacking::Abgr210Le, true, true),
            VideoFormat::RGBA_102LE => (NativeRgbPacking::Rgba102Le, false, true),
            VideoFormat::BGRA_102LE => (NativeRgbPacking::Bgra102Le, true, true),
            _ => return Err(unsupported()),
        };
        let metadata = [
            info.color_range(),
            info.color_matrix(),
            info.color_primaries(),
            info.transfer_function(),
        ];
        let rgb8 = matches!(
            packing,
            NativeRgbPacking::Rgba8
                | NativeRgbPacking::Bgra8
                | NativeRgbPacking::Rgbx8
                | NativeRgbPacking::Bgrx8
        );
        // Preserve the established untagged/explicit sRGB8 path byte-for-byte.
        // Every other format requires all four real negotiated color properties.
        let legacy = rgb8
            && matches!(metadata[0], 0 | spa::sys::SPA_VIDEO_COLOR_RANGE_0_255)
            && matches!(metadata[1], 0 | spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB)
            && matches!(metadata[2], 0 | spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709)
            && matches!(metadata[3], 0 | spa::sys::SPA_VIDEO_TRANSFER_SRGB);
        let color = if legacy {
            None
        } else {
            if metadata[0] != spa::sys::SPA_VIDEO_COLOR_RANGE_0_255
                || metadata[1] != spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB
            {
                return Err(unsupported());
            }
            let primaries = match metadata[2] {
                spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709 => NativeRgbPrimaries::Bt709,
                spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT2020 => NativeRgbPrimaries::Bt2020,
                spa::sys::SPA_VIDEO_COLOR_PRIMARIES_SMPTEEG432 => NativeRgbPrimaries::DisplayP3,
                spa::sys::SPA_VIDEO_COLOR_PRIMARIES_ADOBERGB => NativeRgbPrimaries::AdobeRgb,
                _ => return Err(unsupported()),
            };
            let transfer = match metadata[3] {
                spa::sys::SPA_VIDEO_TRANSFER_GAMMA10 => NativeRgbTransfer::Linear,
                spa::sys::SPA_VIDEO_TRANSFER_SRGB => NativeRgbTransfer::Srgb,
                spa::sys::SPA_VIDEO_TRANSFER_BT709
                | spa::sys::SPA_VIDEO_TRANSFER_BT2020_10
                | spa::sys::SPA_VIDEO_TRANSFER_BT601 => NativeRgbTransfer::Bt709,
                spa::sys::SPA_VIDEO_TRANSFER_BT2020_12 => NativeRgbTransfer::Bt2020Twelve,
                spa::sys::SPA_VIDEO_TRANSFER_GAMMA22 => NativeRgbTransfer::Gamma22,
                spa::sys::SPA_VIDEO_TRANSFER_ADOBERGB => NativeRgbTransfer::AdobeRgb,
                spa::sys::SPA_VIDEO_TRANSFER_SMPTE2084 if !rgb8 => NativeRgbTransfer::Pq,
                spa::sys::SPA_VIDEO_TRANSFER_ARIB_STD_B67 if !rgb8 => NativeRgbTransfer::Hlg,
                _ => return Err(unsupported()),
            };
            Some(NativeRgbColor {
                transfer,
                primaries,
            })
        };
        let source =
            u64::from(size.width) * u64::from(size.height) * packing.bytes_per_pixel() as u64;
        if source > app_core::MAX_CAPTURE_BYTES {
            return Err(CaptureError::MemoryLimit);
        }
        Ok(Self {
            width: size.width,
            height: size.height,
            bgr,
            alpha,
            premultiplied: info.flags().bits() & 2 != 0,
            packing,
            color,
            metadata,
        })
    }
    pub fn finite_buffer_limit(self) -> usize {
        if self.packing.bytes_per_pixel() == 16 {
            32 * 1024 * 1024
        } else {
            16 * 1024 * 1024
        }
    }
    pub fn extent(
        self,
        mapped: usize,
        offset: u32,
        size: u32,
        stride: i32,
    ) -> Result<std::ops::Range<usize>, CaptureError> {
        validate_image_size(self.width, self.height)?;
        let row = self.width as usize * self.packing.bytes_per_pixel();
        if mapped == 0 || mapped > MAX_MAPPED_BYTES || stride <= 0 || (stride as usize) < row {
            return Err(unsupported());
        }
        let needed = (self.height as usize - 1)
            .checked_mul(stride as usize)
            .and_then(|value| value.checked_add(row))
            .ok_or(CaptureError::MemoryLimit)?;
        let end = (offset as usize)
            .checked_add(size as usize)
            .ok_or(CaptureError::MemoryLimit)?;
        if needed > size as usize || end > mapped {
            return Err(unsupported());
        }
        Ok(offset as usize..offset as usize + needed)
    }
    pub fn decode(self, bytes: &[u8], stride: usize) -> Result<image::RgbaImage, CaptureError> {
        self.extent(
            bytes.len(),
            0,
            bytes
                .len()
                .try_into()
                .map_err(|_| CaptureError::MemoryLimit)?,
            stride.try_into().map_err(|_| unsupported())?,
        )?;
        if let Some(color) = self.color {
            return app_core::decode_native_rgb_with_cancel(
                NativeRgbSource {
                    width: self.width,
                    height: self.height,
                    stride,
                    packing: self.packing,
                    premultiplied: self.premultiplied,
                    opaque_alpha_only: true,
                    color,
                },
                bytes,
                &|| false,
            );
        }
        let mut output = image::RgbaImage::new(self.width, self.height);
        let samples: &mut [u8] = output.as_mut();
        for (source, target) in bytes
            .chunks(stride)
            .zip(samples.chunks_mut(self.width as usize * 4))
        {
            for (pixel, destination) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
                let alpha = if self.alpha { pixel[3] } else { 255 };
                let expand = |value: u8| -> u8 {
                    if !self.alpha || !self.premultiplied {
                        value
                    } else if alpha == 0 {
                        0
                    } else {
                        ((value as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
                    }
                };
                destination.copy_from_slice(&[
                    expand(pixel[if self.bgr { 2 } else { 0 }]),
                    expand(pixel[1]),
                    expand(pixel[if self.bgr { 0 } else { 2 }]),
                    alpha,
                ]);
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(format: VideoFormat) -> VideoInfoRaw {
        let mut info = VideoInfoRaw::new();
        info.set_format(format);
        info.set_size(spa::utils::Rectangle {
            width: 2,
            height: 2,
        });
        info
    }
    #[test]
    fn precision_requires_real_complete_metadata_and_changes_remain_visible() {
        for video in [
            VideoFormat::RGBA_F16,
            VideoFormat::RGBA_F32,
            VideoFormat::xBGR_210LE,
        ] {
            let mut raw = info(video);
            assert!(Format::parse(raw).is_err());
            raw.set_color_range(spa::sys::SPA_VIDEO_COLOR_RANGE_0_255);
            raw.set_color_matrix(spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB);
            raw.set_color_primaries(spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT2020);
            raw.set_transfer_function(spa::sys::SPA_VIDEO_TRANSFER_SMPTE2084);
            let admitted = Format::parse(raw).unwrap();
            assert_eq!(
                admitted.packing.bytes_per_pixel(),
                if video == VideoFormat::RGBA_F16 {
                    8
                } else if video == VideoFormat::RGBA_F32 {
                    16
                } else {
                    4
                }
            );
            raw.set_transfer_function(spa::sys::SPA_VIDEO_TRANSFER_ARIB_STD_B67);
            assert_ne!(Format::parse(raw).unwrap(), admitted);
            raw.set_color_matrix(0);
            assert!(Format::parse(raw).is_err());
        }
    }
    #[test]
    fn float_depth_and_native_row_pitch_are_charged_before_pixels() {
        let mut raw = info(VideoFormat::RGBA_F32);
        raw.set_color_range(spa::sys::SPA_VIDEO_COLOR_RANGE_0_255);
        raw.set_color_matrix(spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB);
        raw.set_color_primaries(spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709);
        raw.set_transfer_function(spa::sys::SPA_VIDEO_TRANSFER_GAMMA10);
        let format = Format::parse(raw).unwrap();
        assert!(format.extent(64, 0, 64, 31).is_err());
        assert_eq!(format.extent(64, 0, 64, 32).unwrap(), 0..64);
        assert_eq!(format.finite_buffer_limit(), 32 * 1024 * 1024);
        raw.set_size(spa::utils::Rectangle {
            width: 8192,
            height: 8192,
        });
        assert!(Format::parse(raw).is_err());
    }
    #[test]
    fn all_packed_orders_padding_offsets_and_hidden_alpha_are_exact() {
        for video in [
            VideoFormat::RGBA,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::BGRx,
        ] {
            let format = Format::parse(info(video)).unwrap();
            let mut bytes = vec![199; 31];
            for (at, rgb) in [
                (3, [11, 17, 23, 0]),
                (7, [29, 31, 37, 127]),
                (15, [41, 43, 47, 255]),
                (19, [53, 59, 61, 91]),
            ] {
                let mut pixel = rgb;
                if format.bgr {
                    pixel.swap(0, 2);
                }
                bytes[at..at + 4].copy_from_slice(&pixel);
            }
            let extent = format.extent(bytes.len(), 3, 20, 12).unwrap();
            let result = format.decode(&bytes[extent], 12).unwrap();
            let mut expected = vec![
                11, 17, 23, 0, 29, 31, 37, 127, 41, 43, 47, 255, 53, 59, 61, 91,
            ];
            if !format.alpha {
                for pixel in expected.chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            }
            assert_eq!(result.as_raw(), &expected);
        }
    }
    #[test]
    fn alpha_encoding_is_explicit_and_premultiplied_values_are_expanded() {
        let mut raw = info(VideoFormat::RGBA);
        raw.set_size(spa::utils::Rectangle {
            width: 2,
            height: 1,
        });
        raw.set_flags(spa::param::video::VideoFlags::PREMULTIPLIED_ALPHA);
        let result = Format::parse(raw)
            .unwrap()
            .decode(&[32, 16, 8, 64, 0, 0, 0, 0], 8)
            .unwrap();
        assert_eq!(result.as_raw(), &[128, 64, 32, 64, 0, 0, 0, 0]);
    }
    #[test]
    fn unsupported_color_hdr_layout_and_extent_fail_before_allocation() {
        for video in [VideoFormat::RGBA_F16, VideoFormat::NV12, VideoFormat::RGB] {
            assert!(Format::parse(info(video)).is_err());
        }
        for (primaries, transfer, range, matrix) in [
            (7, 0, 0, 0),
            (0, 14, 0, 0),
            (0, 15, 0, 0),
            (0, 0, 2, 0),
            (0, 0, 0, 3),
        ] {
            let mut raw = info(VideoFormat::RGBA);
            raw.set_color_primaries(primaries);
            raw.set_transfer_function(transfer);
            raw.set_color_range(range);
            raw.set_color_matrix(matrix);
            assert!(Format::parse(raw).is_err());
        }
        let format = Format::parse(info(VideoFormat::RGBA)).unwrap();
        for (mapped, offset, size, stride) in [
            (16, 0, 16, -8),
            (16, 0, 16, 7),
            (16, 1, 16, 8),
            (16, 0, 15, 8),
            (MAX_MAPPED_BYTES + 1, 0, 16, 8),
            (16, u32::MAX, 16, 8),
            (16, 0, 16, i32::MAX),
        ] {
            assert!(format.extent(mapped, offset, size, stride).is_err());
        }
        let mut raw = info(VideoFormat::RGBA);
        raw.set_size(spa::utils::Rectangle {
            width: u32::MAX,
            height: u32::MAX,
        });
        assert!(Format::parse(raw).is_err());
        let mut raw = info(VideoFormat::RGBA);
        raw.set_size(spa::utils::Rectangle {
            width: 8192,
            height: 8192,
        });
        let format = Format::parse(raw).unwrap();
        assert_eq!(
            format
                .extent(MAX_MAPPED_BYTES, 0, MAX_MAPPED_BYTES as u32, 32768)
                .unwrap(),
            0..MAX_MAPPED_BYTES
        );
    }
}
