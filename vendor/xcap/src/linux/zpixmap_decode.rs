//! Checked TrueColor ZPixmap packing; channel quantization only, no color/HDR inference.

const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
type DecodeResult<T> = Result<T, &'static str>;

pub(super) fn request_extent(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> DecodeResult<(i16, i16, u16, u16)> {
    if width == 0 || height == 0 {
        return Err("X11 capture dimensions must be nonzero");
    }
    Ok((
        i16::try_from(x).map_err(|_| "X11 capture x exceeds INT16")?,
        i16::try_from(y).map_err(|_| "X11 capture y exceeds INT16")?,
        u16::try_from(width).map_err(|_| "X11 capture width exceeds CARD16")?,
        u16::try_from(height).map_err(|_| "X11 capture height exceeds CARD16")?,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Format {
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
    pub little_endian: bool,
    pub masks: [u32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    stride: usize,
    pixel_row: usize,
    rgba_row: usize,
    native_len: usize,
    wire_len: usize,
    rgba_len: usize,
    bytes_per_pixel: usize,
    format: Format,
}

impl Layout {
    pub(super) fn new(width: u32, height: u32, format: Format) -> DecodeResult<Self> {
        request_extent(0, 0, width, height)?;
        if format.depth == 0
            || format.depth > format.bits_per_pixel
            || !matches!(format.bits_per_pixel, 8 | 16 | 24 | 32)
            || !matches!(format.scanline_pad, 8 | 16 | 32)
        {
            return Err("Unsupported X11 depth or byte-aligned pixmap packing");
        }
        let depth_mask = u32::MAX >> (32 - u32::from(format.depth));
        let [r, g, b] = format.masks;
        if r & g != 0 || r & b != 0 || g & b != 0 {
            return Err("X11 RGB masks overlap");
        }
        for mask in format.masks {
            if mask == 0 || mask & !depth_mask != 0 {
                return Err("X11 RGB mask is empty or exceeds drawable depth");
            }
            let field = mask >> mask.trailing_zeros();
            if u64::from(field) & (u64::from(field) + 1) != 0 {
                return Err("X11 RGB masks must be contiguous");
            }
        }
        let width = usize::try_from(width).map_err(|_| "X11 width exceeds address space")?;
        let height = usize::try_from(height).map_err(|_| "X11 height exceeds address space")?;
        let bytes_per_pixel = usize::from(format.bits_per_pixel / 8);
        let pixel_row = width
            .checked_mul(bytes_per_pixel)
            .ok_or("X11 pixel row overflow")?;
        let pad = usize::from(format.scanline_pad / 8);
        let stride = pixel_row
            .checked_add(pad - 1)
            .ok_or("X11 row padding overflow")?
            / pad
            * pad;
        let native_len = stride
            .checked_mul(height)
            .ok_or("X11 native extent overflow")?;
        // xcb exposes the final GetImage protocol padding as part of reply.data().
        let wire_len = native_len
            .checked_add(3)
            .ok_or("X11 reply padding overflow")?
            / 4
            * 4;
        let rgba_row = width.checked_mul(4).ok_or("X11 RGBA row overflow")?;
        let rgba_len = rgba_row
            .checked_mul(height)
            .ok_or("X11 RGBA extent overflow")?;
        if wire_len > MAX_IMAGE_BYTES || rgba_len > MAX_IMAGE_BYTES {
            return Err("X11 native or RGBA image exceeds 256 MiB");
        }
        Ok(Self {
            stride,
            pixel_row,
            rgba_row,
            native_len,
            wire_len,
            rgba_len,
            bytes_per_pixel,
            format,
        })
    }

    pub(super) fn decode(self, bytes: &[u8]) -> DecodeResult<Vec<u8>> {
        if bytes.len() != self.wire_len {
            return Err("X11 image reply does not match checked native extent");
        }
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(self.rgba_len)
            .map_err(|_| "X11 RGBA allocation failed")?;
        rgba.resize(self.rgba_len, 0);
        let channels = self.format.masks.map(|mask| {
            let shift = mask.trailing_zeros();
            (mask, shift, u64::from(mask >> shift))
        });
        let rgb8 = channels
            .iter()
            .all(|&(_, shift, maximum)| maximum == 255 && shift % 8 == 0)
            .then(|| {
                channels.map(|(_, shift, _)| {
                    let byte = shift as usize / 8;
                    if self.format.little_endian {
                        byte
                    } else {
                        self.bytes_per_pixel - 1 - byte
                    }
                })
            });
        for (source, destination) in bytes[..self.native_len]
            .chunks_exact(self.stride)
            .zip(rgba.chunks_exact_mut(self.rgba_row))
        {
            if let Some([r, g, b]) = rgb8 {
                for (pixel, out) in source[..self.pixel_row]
                    .chunks_exact(self.bytes_per_pixel)
                    .zip(destination.chunks_exact_mut(4))
                {
                    out.copy_from_slice(&[pixel[r], pixel[g], pixel[b], 255]);
                }
                continue;
            }
            for (pixel, out) in source[..self.pixel_row]
                .chunks_exact(self.bytes_per_pixel)
                .zip(destination.chunks_exact_mut(4))
            {
                let mut value = 0u32;
                if self.format.little_endian {
                    for (index, byte) in pixel.iter().enumerate() {
                        value |= u32::from(*byte) << (index * 8);
                    }
                } else {
                    for byte in pixel {
                        value = (value << 8) | u32::from(*byte);
                    }
                }
                for (out, &(mask, shift, maximum)) in out[..3].iter_mut().zip(&channels) {
                    let sample = u64::from((value & mask) >> shift);
                    *out = ((sample * 255 + maximum / 2) / maximum) as u8;
                }
                // A TrueColor visual supplies RGB masks, never an alpha/transfer contract.
                out[3] = 255;
            }
        }
        Ok(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb565(little_endian: bool) -> Format {
        Format {
            depth: 16,
            bits_per_pixel: 16,
            scanline_pad: 32,
            little_endian,
            masks: [0xf800, 0x07e0, 0x001f],
        }
    }
    fn rgb888(bits_per_pixel: u8, little_endian: bool) -> Format {
        Format {
            depth: 24,
            bits_per_pixel,
            scanline_pad: 32,
            little_endian,
            masks: [0xff0000, 0xff00, 0xff],
        }
    }

    #[test]
    fn padded_rgb565_rows_and_integer_rounding_are_exact() {
        let expected = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 123, 125, 123, 255, 0, 0, 0, 255, 255,
            255, 255, 255,
        ];
        // Three 16-bit pixels per row; padding must not become the next row's pixel.
        let little = [
            0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xaa, 0xbb, 0xef, 0x7b, 0x00, 0x00, 0xff, 0xff,
            0xcc, 0xdd,
        ];
        let big = [
            0xf8, 0x00, 0x07, 0xe0, 0x00, 0x1f, 0xaa, 0xbb, 0x7b, 0xef, 0x00, 0x00, 0xff, 0xff,
            0xcc, 0xdd,
        ];
        assert_eq!(
            Layout::new(3, 2, rgb565(true))
                .unwrap()
                .decode(&little)
                .unwrap(),
            expected
        );
        assert_eq!(
            Layout::new(3, 2, rgb565(false))
                .unwrap()
                .decode(&big)
                .unwrap(),
            expected
        );
        // R=B=3/31 rounds up to 25; truncation would incorrectly produce 24.
        assert_eq!(
            Layout::new(1, 1, rgb565(true))
                .unwrap()
                .decode(&[0x23, 0x18, 0xaa, 0xbb])
                .unwrap(),
            [25, 4, 25, 255]
        );
    }

    #[test]
    fn packed_rgb888_rows_use_scanline_padding_in_both_image_byte_orders() {
        let expected = [0x12, 0x34, 0x56, 255, 0xab, 0xcd, 0xef, 255];
        assert_eq!(
            Layout::new(1, 2, rgb888(24, true))
                .unwrap()
                .decode(&[0x56, 0x34, 0x12, 0xaa, 0xef, 0xcd, 0xab, 0xbb])
                .unwrap(),
            expected
        );
        assert_eq!(
            Layout::new(1, 2, rgb888(24, false))
                .unwrap()
                .decode(&[0x12, 0x34, 0x56, 0xaa, 0xab, 0xcd, 0xef, 0xbb])
                .unwrap(),
            expected
        );
    }

    #[test]
    fn zpixmap_image_order_is_independent_of_bitmap_order_and_high_padding_is_opaque() {
        // The adapter passes image_byte_order only; there is deliberately no bitmap-order argument.
        assert_eq!(
            Layout::new(1, 1, rgb888(32, false))
                .unwrap()
                .decode(&[0xa5, 0x12, 0x34, 0x56])
                .unwrap(),
            [0x12, 0x34, 0x56, 255]
        );
        assert_eq!(
            Layout::new(1, 1, rgb888(32, true))
                .unwrap()
                .decode(&[0x56, 0x34, 0x12, 0xa5])
                .unwrap(),
            [0x12, 0x34, 0x56, 255]
        );
    }

    #[test]
    fn shifted_rgb101010_masks_are_scaled_without_transfer_or_alpha_inference() {
        // Red occupies the low ten bits, green the high ten and blue the middle ten.
        let format = Format {
            depth: 30,
            bits_per_pixel: 32,
            scanline_pad: 32,
            little_endian: true,
            masks: [0x3ff, 0x3ff00000, 0xffc00],
        };
        // R=1023, G=512, B=1, with both undefined high bits set.
        assert_eq!(
            Layout::new(1, 1, format)
                .unwrap()
                .decode(&[0xff, 0x07, 0x00, 0xe0])
                .unwrap(),
            [255, 128, 0, 255]
        );
        let format = Format {
            little_endian: false,
            ..format
        };
        assert_eq!(
            Layout::new(1, 1, format)
                .unwrap()
                .decode(&[0xe0, 0x00, 0x07, 0xff])
                .unwrap(),
            [255, 128, 0, 255]
        );
    }

    #[test]
    fn final_wire_padding_is_distinct_from_scanline_padding() {
        let format = Format {
            scanline_pad: 16,
            ..rgb565(true)
        };
        let layout = Layout::new(1, 1, format).unwrap();
        assert_eq!(
            layout.decode(&[0x1f, 0, 0xab, 0xcd]).unwrap(),
            [0, 0, 255, 255]
        );
        assert!(layout.decode(&[0x1f, 0]).is_err());
    }

    #[test]
    fn bad_masks_depth_packing_extents_and_reply_lengths_are_refused() {
        let good = rgb565(true);
        for masks in [
            [0, 0x7e0, 0x1f],
            [0xf800, 0x7e0, 0x7e0],
            [0xa800, 0x7e0, 0x1f],
            [0x1f0000, 0x7e0, 0x1f],
        ] {
            assert!(Layout::new(1, 1, Format { masks, ..good }).is_err());
        }
        for format in [
            Format { depth: 0, ..good },
            Format { depth: 17, ..good },
            Format {
                bits_per_pixel: 15,
                ..good
            },
            Format {
                scanline_pad: 64,
                ..good
            },
        ] {
            assert!(Layout::new(1, 1, format).is_err());
        }
        for (width, height) in [
            (0, 1),
            (1, 0),
            (65536, 1),
            (1, 65536),
            (65535, 65535),
            (u32::MAX, u32::MAX),
            (16385, 4096),
        ] {
            assert!(Layout::new(width, height, good).is_err());
        }
        let layout = Layout::new(3, 2, good).unwrap();
        assert!(layout.decode(&[0; 15]).is_err());
        assert!(layout.decode(&[0; 17]).is_err());
        // Admission at the ceiling requires no image allocation.
        assert!(Layout::new(16384, 4096, rgb888(32, true)).is_ok());
    }

    #[test]
    fn protocol_origins_and_dimensions_never_wrap_or_truncate() {
        assert_eq!(
            request_extent(-32768, 32767, 65535, 1).unwrap(),
            (-32768, 32767, 65535, 1)
        );
        for (x, y, w, h) in [
            (-32769, 0, 1, 1),
            (32768, 0, 1, 1),
            (0, -32769, 1, 1),
            (0, 32768, 1, 1),
            (0, 0, 0, 1),
            (0, 0, 1, 0),
            (0, 0, 65536, 1),
            (0, 0, 1, 65536),
        ] {
            assert!(request_extent(x, y, w, h).is_err());
        }
    }
}
