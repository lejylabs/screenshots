//! Explicit, worker-side clipboard import. No clipboard watcher or read-back on copy.
use anyhow::{Context, Result, ensure};
use windows_sys::Win32::System::{
    DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    },
    Memory::{GlobalLock, GlobalSize, GlobalUnlock},
};

const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
const MAX_DIB_BYTES: usize = crate::MAX_CAPTURE_BYTES as usize + 64 * 1024;

fn validate_payload_size(length: usize, png: bool) -> Result<()> {
    let limit = if png { MAX_PNG_BYTES } else { MAX_DIB_BYTES };
    ensure!(
        length > 0 && length <= limit,
        "Clipboard image exceeds the input limit or is empty"
    );
    Ok(())
}

struct OpenClipboardGuard;
impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

/// Call only in response to an explicit user action, off the foreground executor.
pub fn read_clipboard_image() -> Result<image::RgbaImage> {
    let (bytes, png) = read_payload()?;
    decode_payload(&bytes, png)
}

fn read_payload() -> Result<(Vec<u8>, bool)> {
    // The clipboard stays open until the bounded payload has been copied; the
    // owner cannot replace/free its HGLOBAL while we inspect and lock it.
    unsafe {
        ensure!(
            OpenClipboard(std::ptr::null_mut()) != 0,
            "Clipboard is busy or unavailable. Try again."
        );
        let _clipboard = OpenClipboardGuard;
        let png_format =
            RegisterClipboardFormatW([b'P' as u16, b'N' as u16, b'G' as u16, 0].as_ptr());
        let (format, png) = if png_format != 0 && IsClipboardFormatAvailable(png_format) != 0 {
            (png_format, true)
        } else if IsClipboardFormatAvailable(CF_DIBV5) != 0 {
            (CF_DIBV5, false)
        } else {
            ensure!(
                IsClipboardFormatAvailable(CF_DIB) != 0,
                "Clipboard does not contain a supported image (PNG or DIB)."
            );
            (CF_DIB, false)
        };
        let handle = GetClipboardData(format);
        ensure!(!handle.is_null(), "Could not read clipboard image");
        let length = GlobalSize(handle);
        validate_payload_size(length, png)?;
        let pointer = GlobalLock(handle);
        ensure!(!pointer.is_null(), "Could not access clipboard image");
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), length).to_vec();
        GlobalUnlock(handle);
        Ok((bytes, png))
    }
}

fn decode_payload(bytes: &[u8], png: bool) -> Result<image::RgbaImage> {
    validate_payload_size(bytes.len(), png)?;
    if png {
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
        reader.limits(limits);
        crate::clipboard::decode_editor_pixels(reader.into_decoder()?)
    } else {
        ensure!(bytes.len() >= 40, "Truncated clipboard bitmap header");
        let field = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let header = field(0) as usize;
        let depth = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
        let compression = field(16);
        ensure!(
            matches!(header, 40 | 108 | 124) && bytes.len() >= header,
            "Unsupported clipboard bitmap header"
        );
        ensure!(
            matches!(depth, 24 | 32)
                && (compression == 0 || (compression == 3 && depth == 32))
                && field(32) == 0,
            "Unsupported clipboard bitmap layout. Copy as PNG or a 24/32-bit bitmap."
        );
        if header == 124 {
            ensure!(
                field(112) == 0 && field(116) == 0,
                "Clipboard bitmap color profiles are unsupported. Copy as PNG."
            );
        }
        let offset = header
            + if header == 40 && compression == 3 {
                12
            } else {
                0
            };
        ensure!(bytes.len() >= offset, "Truncated clipboard bitmap masks");
        // The pinned BMP decoder misplaces V4/V5 bitfield pixels in its DIB mode.
        // An explicit BMP offset preserves the native layout without a new decoder.
        let mut bmp = Vec::with_capacity(14 + bytes.len());
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&((14 + bytes.len()) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&((14 + offset) as u32).to_le_bytes());
        bmp.extend_from_slice(bytes);
        let decoder = image::codecs::bmp::BmpDecoder::new(std::io::Cursor::new(bmp))
            .context("Unsupported or invalid clipboard bitmap")?;
        crate::clipboard::decode_editor_pixels(decoder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v5_bitmap() -> Vec<u8> {
        let mut bytes = vec![0; 124];
        for (offset, value) in [
            (0, 124_u32),
            (4, 2),
            (8, 2),
            (16, 3),
            (40, 0x00ff0000),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[12..14].copy_from_slice(&1_u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&32_u16.to_le_bytes());
        // Bottom row, then top row, in BGRA order, with distinct alpha values.
        bytes.extend_from_slice(&[
            90, 80, 70, 60, 130, 120, 110, 100, 30, 20, 10, 0, 60, 50, 40, 255,
        ]);
        bytes
    }

    #[test]
    fn clipboard_png_and_dib_preserve_pixels_alpha_and_orientation() -> Result<()> {
        let source = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 70) as u8, (y * 90) as u8, 40, 75])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png)?;
        assert_eq!(
            decode_payload(png.get_ref(), true).context("PNG fixture")?,
            source
        );

        let rgb =
            image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(source).into_rgb8());
        let mut bmp = std::io::Cursor::new(Vec::new());
        rgb.write_to(&mut bmp, image::ImageFormat::Bmp)?;
        assert_eq!(
            decode_payload(&bmp.get_ref()[14..], false).context("RGB bitmap fixture")?,
            rgb.into_rgba8()
        );

        let bytes = v5_bitmap();
        let expected = image::RgbaImage::from_raw(
            2,
            2,
            vec![
                10, 20, 30, 0, 40, 50, 60, 255, 70, 80, 90, 60, 110, 120, 130, 100,
            ],
        )
        .unwrap();
        assert_eq!(
            decode_payload(&bytes, false).context("V5 bitmap fixture")?,
            expected
        );
        let mut top_down = bytes.clone();
        top_down[8..12].copy_from_slice(&(-2_i32).to_le_bytes());
        top_down[124..132].copy_from_slice(&bytes[132..140]);
        top_down[132..140].copy_from_slice(&bytes[124..132]);
        assert_eq!(
            decode_payload(&top_down, false).context("Top-down bitmap fixture")?,
            expected
        );
        assert_eq!(bytes, v5_bitmap());
        Ok(())
    }

    #[test]
    fn clipboard_decode_rejects_invalid_truncated_and_oversized_images() {
        for png in [true, false] {
            let limit = if png { MAX_PNG_BYTES } else { MAX_DIB_BYTES };
            assert!(validate_payload_size(0, png).is_err());
            assert!(validate_payload_size(limit, png).is_ok());
            assert!(validate_payload_size(limit + 1, png).is_err());
            for bytes in [b"".as_slice(), b"not an image", b"\x89PNG\r\n\x1a\n"] {
                assert!(decode_payload(bytes, png).is_err());
            }
        }
        let bytes = v5_bitmap();
        for end in [0, 4, 40, 123, 124, 139] {
            assert!(decode_payload(&bytes[..end], false).is_err(), "{end}");
        }
        let mut oversized = bytes.clone();
        oversized[4..8].copy_from_slice(&100_000_u32.to_le_bytes());
        oversized[8..12].copy_from_slice(&100_000_u32.to_le_bytes());
        assert!(decode_payload(&oversized, false).is_err());
        let mut invalid = bytes;
        invalid[12..14].copy_from_slice(&2_u16.to_le_bytes());
        assert!(decode_payload(&invalid, false).is_err());
    }

    #[test]
    #[ignore = "short optimized 4K PNG/DIB decode comparison; synthetic data, no native clipboard access"]
    fn clipboard_4k_decode_cost() -> Result<()> {
        let mut dib = v5_bitmap();
        dib.truncate(124);
        dib[4..8].copy_from_slice(&3840_u32.to_le_bytes());
        dib[8..12].copy_from_slice(&2160_u32.to_le_bytes());
        dib.resize(124 + 3840 * 2160 * 4, 0);
        for pixel in dib[124..].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[30, 20, 10, 255]);
        }
        let source = image::RgbaImage::from_pixel(3840, 2160, image::Rgba([10, 20, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png)?;
        drop(source);
        let mut timings = [Vec::new(), Vec::new()];
        for _ in 0..5 {
            for (index, (bytes, png)) in [(png.get_ref().as_slice(), true), (dib.as_slice(), false)]
                .into_iter()
                .enumerate()
            {
                let started = std::time::Instant::now();
                let decoded = decode_payload(bytes, png)?;
                timings[index].push(started.elapsed());
                assert_eq!(decoded.dimensions(), (3840, 2160));
                assert_eq!(decoded.get_pixel(0, 0).0, [10, 20, 30, 255]);
            }
        }
        for values in &mut timings {
            values.sort_unstable();
        }
        eprintln!(
            "4K clipboard synthetic decode: PNG median={:?}; DIB median={:?}; 5 warm samples; RGBA=33177600 bytes; excludes native clipboard and presentation",
            timings[0][2], timings[1][2]
        );
        Ok(())
    }
}
