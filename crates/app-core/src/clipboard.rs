//! In-memory clipboard images. Keep the clipboard alive for clipboard ownership on Linux.

use std::borrow::Cow;

use anyhow::{Context, Result};

const MAX_EDITOR_FILE_BYTES: u64 = 32 * 1024 * 1024;

// JPEG reads its input in full before applying decoder limits. Freeze the readable
// extent so a growing file cannot bypass the encoded-byte admission check.
struct EditorFile {
    file: std::fs::File,
    length: u64,
    position: u64,
}

impl std::io::Read for EditorFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let available = self
            .length
            .saturating_sub(self.position)
            .min(buffer.len() as u64) as usize;
        let read = self.file.read(&mut buffer[..available])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl std::io::Seek for EditorFile {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        let from = match from {
            std::io::SeekFrom::End(offset) => {
                let position = i128::from(self.length) + i128::from(offset);
                std::io::SeekFrom::Start(u64::try_from(position).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid image seek")
                })?)
            }
            other => other,
        };
        self.position = self.file.seek(from)?;
        Ok(self.position)
    }
}

/// Explicit editor input only; run decoding and orientation on a worker.
pub fn load_editor_image(path: &std::path::Path) -> Result<image::RgbaImage> {
    load_editor_image_with_limit(path, crate::MAX_CAPTURE_BYTES)
}

/// Admit another image within a composition's remaining decoded-source budget.
pub fn load_editor_image_with_limit(
    path: &std::path::Path,
    max_decoded_bytes: u64,
) -> Result<image::RgbaImage> {
    load_image_input(path, max_decoded_bytes, false)
}

pub(crate) fn load_export_png(path: &std::path::Path) -> Result<image::RgbaImage> {
    load_image_input(path, crate::MAX_CAPTURE_BYTES, true)
}

fn load_image_input(
    path: &std::path::Path,
    max_decoded_bytes: u64,
    png_only: bool,
) -> Result<image::RgbaImage> {
    anyhow::ensure!(
        max_decoded_bytes > 0 && max_decoded_bytes <= crate::MAX_CAPTURE_BYTES,
        "Invalid decoded image budget"
    );
    anyhow::ensure!(
        path.metadata()?.is_file(),
        "Image input must be a regular file"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "Image input must be a regular file");
    let length = metadata.len();
    anyhow::ensure!(
        length <= MAX_EDITOR_FILE_BYTES,
        "Image file exceeds the 32 MiB input limit"
    );
    let mut reader = image::ImageReader::new(std::io::BufReader::new(EditorFile {
        file,
        length,
        position: 0,
    }))
    .with_guessed_format()?;
    anyhow::ensure!(
        !png_only || reader.format() == Some(image::ImageFormat::Png),
        "History metadata export requires PNG input"
    );
    anyhow::ensure!(
        matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
        ),
        "Choose a PNG, JPEG or WebP image"
    );
    if reader.format() == Some(image::ImageFormat::WebP) {
        use std::io::{Read as _, Seek as _};
        // The WebP decoder trusts metadata/frame lengths before applying image limits.
        // Validate and decode the same bounded bytes, including under in-place file edits.
        let mut input = reader.into_inner();
        input.seek(std::io::SeekFrom::Start(0))?;
        let mut bytes = vec![0; usize::try_from(length)?];
        input.read_exact(&mut bytes)?;
        let orientation = validate_webp(&bytes, max_decoded_bytes)?;
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::WebP);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
        reader.limits(limits);
        return decode_editor_pixels_with_limit(
            reader.into_decoder()?,
            max_decoded_bytes,
            Some(orientation),
        );
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    decode_editor_pixels_with_limit(reader.into_decoder()?, max_decoded_bytes, None)
}

const MAX_WEBP_CHUNKS: usize = 4096;

fn webp_dimensions(width: u32, height: u32, budget: u64) -> Result<(u32, u32)> {
    crate::validate_image_size(width, height)?;
    anyhow::ensure!(
        u64::from(width) * u64::from(height) * 4 <= budget,
        "WebP frame exceeds the decoded image budget"
    );
    Ok((width, height))
}

fn webp_u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

/// Returns one checked chunk without trusting its declared size for an allocation.
fn webp_chunk<'a>(
    bytes: &'a [u8],
    position: &mut usize,
    count: &mut usize,
) -> Result<([u8; 4], &'a [u8])> {
    *count += 1;
    anyhow::ensure!(
        *count <= MAX_WEBP_CHUNKS,
        "WebP exceeds the 4096-chunk limit"
    );
    let header = bytes
        .get(*position..)
        .and_then(|tail| tail.get(..8))
        .context("Truncated WebP chunk header")?;
    let kind = header[..4].try_into()?;
    let size = u32::from_le_bytes(header[4..8].try_into()?) as usize;
    let start = position
        .checked_add(8)
        .context("WebP chunk offset overflow")?;
    let end = start
        .checked_add(size)
        .context("WebP chunk size overflow")?;
    let padded = end
        .checked_add(size & 1)
        .context("WebP chunk padding overflow")?;
    anyhow::ensure!(
        padded <= bytes.len(),
        "WebP chunk exceeds its encoded extent"
    );
    *position = padded;
    Ok((kind, &bytes[start..end]))
}

fn webp_bitstream(kind: [u8; 4], payload: &[u8], budget: u64) -> Result<(u32, u32)> {
    match &kind {
        b"VP8 " => {
            anyhow::ensure!(payload.len() >= 10, "Truncated VP8 frame header");
            anyhow::ensure!(
                payload[0] & 1 == 0 && payload[3..6] == [0x9d, 0x01, 0x2a],
                "Invalid VP8 keyframe header"
            );
            webp_dimensions(
                u32::from(u16::from_le_bytes(payload[6..8].try_into()?) & 0x3fff),
                u32::from(u16::from_le_bytes(payload[8..10].try_into()?) & 0x3fff),
                budget,
            )
        }
        b"VP8L" => {
            anyhow::ensure!(
                payload.len() >= 5 && payload[0] == 0x2f,
                "Invalid VP8L frame header"
            );
            let header = u32::from_le_bytes(payload[1..5].try_into()?);
            anyhow::ensure!(header >> 29 == 0, "Unsupported VP8L version");
            webp_dimensions((header & 0x3fff) + 1, ((header >> 14) & 0x3fff) + 1, budget)
        }
        _ => anyhow::bail!("Unsupported WebP frame bitstream"),
    }
}

/// Validate every frame, not just the advertised VP8X canvas, before the decoder runs.
fn validate_webp(bytes: &[u8], budget: u64) -> Result<image::metadata::Orientation> {
    anyhow::ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "Invalid WebP RIFF header"
    );
    let size = u32::from_le_bytes(bytes[4..8].try_into()?) as u64 + 8;
    anyhow::ensure!(
        size == bytes.len() as u64,
        "WebP RIFF extent does not match its input"
    );
    let mut count = 0;
    let mut position = 12;
    let (first, payload) = webp_chunk(bytes, &mut position, &mut count)?;
    let (canvas, animated) = if first == *b"VP8X" {
        anyhow::ensure!(payload.len() == 10, "Invalid VP8X header size");
        (
            webp_dimensions(
                webp_u24(&payload[4..7]) + 1,
                webp_u24(&payload[7..10]) + 1,
                budget,
            )?,
            payload[0] & 2 != 0,
        )
    } else {
        (webp_bitstream(first, payload, budget)?, false)
    };
    let mut image_count = usize::from(first != *b"VP8X");
    let mut frame_count = 0;
    let mut orientation = image::metadata::Orientation::NoTransforms;
    let mut seen_exif = false;
    while position < bytes.len() {
        let (kind, payload) = webp_chunk(bytes, &mut position, &mut count)?;
        match &kind {
            b"EXIF" if first == *b"VP8X" && !seen_exif => {
                // Borrow the admitted metadata instead of making the decoder copy it.
                orientation = image::metadata::Orientation::from_exif_chunk(payload)
                    .unwrap_or(image::metadata::Orientation::NoTransforms);
                seen_exif = true;
            }
            b"VP8X" => anyhow::bail!("Duplicate WebP extended header"),
            b"VP8 " | b"VP8L" => {
                anyhow::ensure!(
                    !animated && image_count == 0,
                    "Ambiguous WebP image bitstreams"
                );
                anyhow::ensure!(
                    webp_bitstream(kind, payload, budget)? == canvas,
                    "WebP bitstream dimensions do not match its canvas"
                );
                image_count += 1;
            }
            b"ANMF" => {
                anyhow::ensure!(
                    animated && payload.len() >= 16,
                    "Invalid WebP animation frame"
                );
                let x = webp_u24(&payload[..3]) * 2;
                let y = webp_u24(&payload[3..6]) * 2;
                let frame = webp_dimensions(
                    webp_u24(&payload[6..9]) + 1,
                    webp_u24(&payload[9..12]) + 1,
                    budget,
                )?;
                anyhow::ensure!(
                    x.checked_add(frame.0).is_some_and(|end| end <= canvas.0)
                        && y.checked_add(frame.1).is_some_and(|end| end <= canvas.1),
                    "WebP animation frame is outside its canvas"
                );
                let mut nested_position = 16;
                let (mut nested, mut data) = webp_chunk(payload, &mut nested_position, &mut count)?;
                if nested == *b"ALPH" {
                    (nested, data) = webp_chunk(payload, &mut nested_position, &mut count)?;
                    anyhow::ensure!(
                        nested == *b"VP8 ",
                        "WebP alpha frame must be followed by a VP8 bitstream"
                    );
                }
                anyhow::ensure!(
                    webp_bitstream(nested, data, budget)? == frame,
                    "WebP bitstream dimensions do not match its animation frame"
                );
                while nested_position < payload.len() {
                    let (extra, _) = webp_chunk(payload, &mut nested_position, &mut count)?;
                    anyhow::ensure!(
                        !matches!(
                            &extra,
                            b"VP8 "
                                | b"VP8L"
                                | b"ALPH"
                                | b"ANMF"
                                | b"VP8X"
                                | b"ANIM"
                                | b"ICCP"
                                | b"EXIF"
                                | b"XMP "
                        ),
                        "WebP animation frame has extra known subchunks"
                    );
                }
                frame_count += 1;
            }
            _ => {}
        }
    }
    anyhow::ensure!(
        if animated {
            frame_count > 0
        } else {
            image_count == 1
        },
        "WebP has no image bitstream"
    );
    Ok(orientation)
}

#[cfg(target_os = "windows")]
pub(crate) fn decode_editor_pixels(decoder: impl image::ImageDecoder) -> Result<image::RgbaImage> {
    decode_editor_pixels_with_limit(decoder, crate::MAX_CAPTURE_BYTES, None)
}

fn decode_editor_pixels_with_limit(
    mut decoder: impl image::ImageDecoder,
    max_decoded_bytes: u64,
    orientation: Option<image::metadata::Orientation>,
) -> Result<image::RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    decoder.set_limits(limits)?;
    let (width, height) = decoder.dimensions();
    crate::validate_image_size(width, height)?;
    anyhow::ensure!(
        decoder.total_bytes() <= max_decoded_bytes
            && u64::from(width) * u64::from(height) * 4 <= max_decoded_bytes,
        "Decoded image exceeds the memory limit"
    );
    let orientation = match orientation {
        Some(orientation) => orientation,
        None => decoder.orientation()?,
    };
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    Ok(decoded.into_rgba8())
}

pub fn load_saved_png(path: &std::path::Path) -> Result<image::RgbaImage> {
    let mut reader = image::ImageReader::with_format(
        std::io::BufReader::new(std::fs::File::open(path)?),
        image::ImageFormat::Png,
    );
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode()?;
    crate::validate_image_size(decoded.width(), decoded.height())?;
    Ok(decoded.into_rgba8())
}

/// Explicit history preview only. Decode on a worker, retain only a bounded thumbnail.
pub fn load_png_thumbnail(
    path: &std::path::Path,
    cancelled: impl Fn() -> bool,
) -> Result<image::RgbaImage> {
    anyhow::ensure!(!cancelled(), "Preview canceled");
    let source = load_saved_png(path)?;
    anyhow::ensure!(!cancelled(), "Preview canceled");
    let (width, height) = (source.width().min(400), source.height().min(240));
    let thumbnail = image::DynamicImage::ImageRgba8(source)
        .thumbnail(width, height)
        .into_rgba8();
    anyhow::ensure!(!cancelled(), "Preview canceled");
    Ok(thumbnail)
}

#[derive(Default)]
pub struct ImageClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl ImageClipboard {
    #[cfg(target_os = "windows")]
    pub fn copy_file(&mut self, path: &std::path::Path) -> Result<()> {
        anyhow::ensure!(path.is_absolute(), "Saved file path must be absolute");
        anyhow::ensure!(
            std::fs::metadata(path)
                .context("Saved file is unavailable")?
                .is_file(),
            "Saved path is not a file"
        );
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("open clipboard")?);
        }
        self.clipboard
            .as_mut()
            .unwrap()
            .set()
            .file_list(&[path])
            .context("copy file to clipboard")
    }

    /// Explicit text output; retain ownership using the same native clipboard instance.
    pub fn copy_text(&mut self, text: String) -> Result<()> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("open clipboard")?);
        }
        self.clipboard
            .as_mut()
            .unwrap()
            .set_text(text)
            .context("copy text to clipboard")
    }

    /// Explicit recopy only; the live capture/selection pipeline stays in memory.
    pub fn copy_saved_png(&mut self, path: &std::path::Path) -> Result<(u32, u32)> {
        self.copy_image(&load_saved_png(path)?)
    }

    pub fn copy_image(&mut self, image: &image::RgbaImage) -> Result<(u32, u32)> {
        let (width, height) = image.dimensions();
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().context("open clipboard")?);
        }
        let clipboard = self.clipboard.as_mut().unwrap();
        clipboard
            .set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Borrowed(image.as_raw()),
            })
            .context("copy image to clipboard")?;
        Ok((width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn editor_input_rejects_fifo_without_opening_it() {
        use std::os::unix::ffi::OsStrExt;
        let path = std::env::temp_dir().join(format!("screenshots-fifo-{}", uuid::Uuid::new_v4()));
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let result = load_editor_image(&path);
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap_err().to_string().contains("regular file"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn file_copy_rejects_invalid_sources_before_opening_the_clipboard() -> Result<()> {
        let mut clipboard = ImageClipboard::default();
        let missing = std::env::temp_dir().join(format!("missing-{}.png", uuid::Uuid::new_v4()));
        for path in [
            std::path::Path::new("relative.png"),
            missing.as_path(),
            std::env::temp_dir().as_path(),
        ] {
            assert!(clipboard.copy_file(path).is_err());
            assert!(clipboard.clipboard.is_none());
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "uses the system clipboard and replaces its contents with a synthetic file reference"]
    fn copied_file_list_round_trip() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-file-clipboard-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root)?;
        let path = root.join("Grüße image.png");
        std::fs::write(&path, b"synthetic file reference, not decoded")?;
        let mut clipboard = ImageClipboard::default();
        let result = (|| -> Result<()> {
            clipboard.copy_file(&path)?;
            let paths = clipboard.clipboard.as_mut().unwrap().get().file_list()?;
            assert_eq!(paths.len(), 1);
            assert_eq!(
                std::fs::canonicalize(&paths[0])?,
                std::fs::canonicalize(&path)?
            );
            assert_eq!(
                std::fs::read(&path)?,
                b"synthetic file reference, not decoded"
            );
            Ok(())
        })();
        std::fs::remove_file(path)?;
        std::fs::remove_dir(root)?;
        result
    }

    #[test]
    #[ignore = "uses the system clipboard and replaces its contents with a synthetic path"]
    fn file_path_text_round_trip() -> Result<()> {
        let mut clipboard = ImageClipboard::default();
        let text = "C:\\Saved captures\\Grüße image.png".to_string();
        clipboard.copy_text(text.clone())?;
        assert_eq!(clipboard.clipboard.as_mut().unwrap().get_text()?, text);
        Ok(())
    }

    #[test]
    fn editor_input_extent_is_bounded_even_when_the_file_grows() -> Result<()> {
        use std::io::{Read as _, Seek as _, Write as _};
        let path = std::env::temp_dir().join(format!(
            "screenshots-editor-extent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"abcd")?;
        let mut input = EditorFile {
            file: std::fs::File::open(&path)?,
            length: 4,
            position: 0,
        };
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"private trailing data")?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"abcd");
        assert_eq!(input.seek(std::io::SeekFrom::End(-2))?, 2);
        bytes.clear();
        input.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"cd");
        assert!(input.seek(std::io::SeekFrom::End(-5)).is_err());
        input.seek(std::io::SeekFrom::Start(1))?;
        assert_eq!(input.seek(std::io::SeekFrom::Current(1))?, 2);
        input.seek(std::io::SeekFrom::Start(100))?;
        assert_eq!(input.read(&mut [0; 1])?, 0);
        drop(input);
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        file.set_len(MAX_EDITOR_FILE_BYTES + 1)?;
        drop(file);
        assert!(
            load_editor_image(&path)
                .unwrap_err()
                .to_string()
                .contains("32 MiB")
        );
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn editor_images_detect_content_preserve_files_and_apply_orientation() -> Result<()> {
        use image::ImageEncoder as _;
        let root =
            std::env::temp_dir().join(format!("screenshots-editor-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("misleading.extension");
        let source = image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78]));
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
        ] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            let image = image::DynamicImage::ImageRgba8(source.clone());
            if format == image::ImageFormat::Jpeg {
                image.to_rgb8().write_to(&mut encoded, format)?;
            } else {
                image.write_to(&mut encoded, format)?;
            }
            let bytes = encoded.into_inner();
            std::fs::write(&path, &bytes)?;
            let loaded = load_editor_image(&path)?;
            assert_eq!(loaded.dimensions(), (3, 2));
            if format != image::ImageFormat::Jpeg {
                assert_eq!(loaded, source);
            }
            assert_eq!(std::fs::read(&path)?, bytes);
        }
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100);
        // TIFF orientation 6: rotate 90 degrees clockwise.
        encoder.set_exif_metadata(vec![
            73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ])?;
        let rgb = image::RgbImage::from_fn(3, 2, |x, y| {
            image::Rgb([(x * 80) as u8, (y * 120) as u8, 40])
        });
        encoder.write_image(rgb.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)?;
        std::fs::write(&path, &jpeg)?;
        let unrotated = image::load_from_memory(&jpeg)?.into_rgba8();
        assert_eq!(
            load_editor_image(&path)?,
            image::imageops::rotate90(&unrotated)
        );
        assert_eq!(std::fs::read(&path)?, jpeg);
        for bytes in [b"not an image".as_slice(), b"\x89PNG\r\n\x1a\n", b"GIF89a"] {
            std::fs::write(&path, bytes)?;
            assert!(load_editor_image(&path).is_err());
        }
        std::fs::remove_file(path)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }

    fn webp_test_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = kind.to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);
        if payload.len() & 1 != 0 {
            bytes.push(0);
        }
        bytes
    }

    fn webp_test_riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
        for chunk in chunks {
            bytes.extend_from_slice(chunk);
        }
        let size = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    fn webp_test_extended(width: u32, height: u32, flags: u8) -> Vec<u8> {
        let mut header = vec![flags, 0, 0, 0];
        header.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        header.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        webp_test_chunk(b"VP8X", &header)
    }

    #[test]
    fn webp_preflight_rejects_unbounded_metadata_inner_dimensions_and_chunk_counts() -> Result<()> {
        let mut enormous_exif = b"EXIF".to_vec();
        enormous_exif.extend_from_slice(&u32::MAX.to_le_bytes());
        let bytes = webp_test_riff(&[webp_test_extended(1, 1, 8), enormous_exif]);
        assert!(
            validate_webp(&bytes, 4)
                .unwrap_err()
                .to_string()
                .contains("encoded extent")
        );

        let mut frame = vec![0x10, 0, 0, 0x9d, 1, 0x2a];
        frame.extend_from_slice(&16383_u16.to_le_bytes());
        frame.extend_from_slice(&16383_u16.to_le_bytes());
        let bytes = webp_test_riff(&[
            webp_test_extended(1, 1, 0),
            webp_test_chunk(b"VP8 ", &frame),
        ]);
        assert!(validate_webp(&bytes, 4).is_err());
        frame[6..8].copy_from_slice(&2_u16.to_le_bytes());
        frame[8..10].copy_from_slice(&2_u16.to_le_bytes());
        let bytes = webp_test_riff(&[
            webp_test_extended(1, 1, 0),
            webp_test_chunk(b"VP8 ", &frame),
        ]);
        assert!(
            validate_webp(&bytes, 16)
                .unwrap_err()
                .to_string()
                .contains("canvas")
        );

        // The upstream ALPH animation branch decodes its next chunk as VP8;
        // forbid disguising a second unvalidated VP8 header behind an ALPH tag.
        let mut animation = vec![0; 16];
        animation.extend_from_slice(&webp_test_chunk(b"ALPH", &[0, 255]));
        animation.extend_from_slice(&webp_test_chunk(b"ALPH", &frame));
        animation.extend_from_slice(&webp_test_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0]));
        let bytes = webp_test_riff(&[
            webp_test_extended(1, 1, 18),
            webp_test_chunk(b"ANIM", &[0, 0, 0, 0, 1, 0]),
            webp_test_chunk(b"ANMF", &animation),
        ]);
        assert!(validate_webp(&bytes, 4).is_err());

        let mut chunks = vec![webp_test_chunk(b"VP8L", &[0x2f, 0, 0, 0, 0])];
        chunks.extend((0..MAX_WEBP_CHUNKS).map(|_| webp_test_chunk(b"JUNK", &[])));
        let bytes = webp_test_riff(&chunks);
        assert!(
            validate_webp(&bytes, 4)
                .unwrap_err()
                .to_string()
                .contains("4096")
        );
        assert!(validate_webp(&bytes[..bytes.len() - 1], 4).is_err());
        Ok(())
    }

    #[test]
    fn webp_preflight_preserves_lossy_alpha_lossless_exif_and_animated_still_inputs() -> Result<()>
    {
        use base64::Engine as _;
        use image::ImageEncoder as _;
        let root = std::env::temp_dir().join(format!(
            "screenshots-webp-admission-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)?;
        let path = root.join("input.webp");
        let lossy = base64::engine::general_purpose::STANDARD
            .decode("UklGRiIAAABXRUJQVlA4IBYAAAAwAQCdASoBAAEADsD+JaQAA3AAAAAA")?;
        validate_webp(&lossy, 4)?;
        std::fs::write(&path, &lossy)?;
        assert_eq!(load_editor_image_with_limit(&path, 4)?.dimensions(), (1, 1));

        let lossy_alpha = webp_test_riff(&[
            webp_test_extended(1, 1, 16),
            webp_test_chunk(b"ALPH", &[0, 78]),
            lossy[12..].to_vec(),
        ]);
        std::fs::write(&path, &lossy_alpha)?;
        assert_eq!(
            load_editor_image_with_limit(&path, 4)?.get_pixel(0, 0).0[3],
            78
        );

        let source = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 70, y as u8 * 90, 50, 78])
        });
        let mut lossless = std::io::Cursor::new(Vec::new());
        source.write_to(&mut lossless, image::ImageFormat::WebP)?;
        let lossless = lossless.into_inner();
        validate_webp(&lossless, 24)?;
        assert!(validate_webp(&lossless, 23).is_err());
        std::fs::write(&path, &lossless)?;
        assert_eq!(load_editor_image_with_limit(&path, 24)?, source);

        let mut oriented = Vec::new();
        let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut oriented);
        encoder.set_exif_metadata(vec![
            73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ])?;
        encoder.write_image(source.as_raw(), 3, 2, image::ExtendedColorType::Rgba8)?;
        std::fs::write(&path, &oriented)?;
        assert_eq!(
            load_editor_image(&path)?,
            image::imageops::rotate90(&source)
        );
        assert_eq!(std::fs::read(&path)?, oriented);

        let mut frame = vec![0; 16];
        frame[6..9].copy_from_slice(&2_u32.to_le_bytes()[..3]);
        frame[9..12].copy_from_slice(&1_u32.to_le_bytes()[..3]);
        frame[15] = 2;
        frame.extend_from_slice(&lossless[12..]);
        frame.extend_from_slice(&webp_test_chunk(b"JUNK", &[1, 2, 3]));
        let animated = webp_test_riff(&[
            webp_test_extended(3, 2, 18),
            webp_test_chunk(b"ANIM", &[0, 0, 0, 0, 1, 0]),
            webp_test_chunk(b"ANMF", &frame),
        ]);
        validate_webp(&animated, 24)?;
        std::fs::write(&path, &animated)?;
        assert_eq!(load_editor_image_with_limit(&path, 24)?, source);
        assert_eq!(std::fs::read(&path)?, animated);
        // Even an unused later frame must be admitted before the still decoder runs.
        frame[6..9].copy_from_slice(&1000_u32.to_le_bytes()[..3]);
        let invalid = webp_test_riff(&[
            webp_test_extended(3, 2, 18),
            webp_test_chunk(b"ANIM", &[0, 0, 0, 0, 1, 0]),
            webp_test_chunk(b"ANMF", &frame),
        ]);
        assert!(validate_webp(&invalid, 24).is_err());
        std::fs::remove_file(path)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "optimized synthetic 4K WebP worker comparison; no desktop or clipboard"]
    fn webp_admission_cost() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "screenshots-webp-cost-{}.webp",
            uuid::Uuid::new_v4()
        ));
        let mut seed = 1_u32;
        let source = image::RgbaImage::from_fn(3840, 2160, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgba([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, 255])
        });
        source.save(&path)?;
        let mut streaming = Vec::new();
        let mut checked = Vec::new();
        for sample in 0..6 {
            // Trusted fixture only: reproduce the former decoder path for comparison.
            let started = std::time::Instant::now();
            let mut reader = image::ImageReader::open(&path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(crate::MAX_CAPTURE_BYTES);
            reader.limits(limits);
            let pixels = decode_editor_pixels_with_limit(
                reader.into_decoder()?,
                crate::MAX_CAPTURE_BYTES,
                None,
            )?;
            let before = started.elapsed();
            assert_eq!(pixels, source);
            drop(pixels);
            let started = std::time::Instant::now();
            let pixels = load_editor_image(&path)?;
            let after = started.elapsed();
            assert_eq!(pixels, source);
            if sample > 0 {
                streaming.push(before);
                checked.push(after);
            }
        }
        streaming.sort();
        checked.sort();
        eprintln!(
            "4K WebP encoded={} bytes; warm medians n=5: trusted streaming={:?}, checked immutable snapshot={:?}; worker-only development comparison, not hardware acceptance",
            std::fs::metadata(&path)?.len(),
            streaming[2],
            checked[2]
        );
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn editor_image_rejects_oversized_header_before_pixel_decode() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "screenshots-editor-limit-{}.png",
            uuid::Uuid::new_v4()
        ));
        image::RgbaImage::new(1, 1).save(&path)?;
        let mut bytes = std::fs::read(&path)?;
        bytes[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&100_000_u32.to_be_bytes());
        let mut crc = u32::MAX;
        for byte in &bytes[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & (0_u32.wrapping_sub(crc & 1)));
            }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        std::fs::write(&path, bytes)?;
        let error = load_editor_image(&path).unwrap_err().to_string();
        assert!(error.contains("limit"), "{error}");
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn thumbnails_are_bounded_cancellable_and_keep_pngs_unchanged() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-preview-{}", uuid::Uuid::new_v4()));
        let path = root.join("image.png");
        assert!(
            load_png_thumbnail(&path, || true)
                .unwrap_err()
                .to_string()
                .contains("canceled")
        );
        assert!(!root.exists());
        std::fs::create_dir_all(&root)?;
        for (width, height, expected) in [
            (640, 480, (320, 240)),
            (1, 10000, (1, 240)),
            (10000, 1, (400, 1)),
            (3, 2, (3, 2)),
        ] {
            image::RgbaImage::from_pixel(width, height, image::Rgba([11, 22, 33, 90]))
                .save(&path)?;
            let before = std::fs::read(&path)?;
            let thumbnail = load_png_thumbnail(&path, || false)?;
            assert_eq!(thumbnail.dimensions(), expected);
            assert!(thumbnail.as_raw().len() <= 400 * 240 * 4);
            assert!(thumbnail.pixels().all(|pixel| pixel.0 == [11, 22, 33, 90]));
            assert_eq!(std::fs::read(&path)?, before);
        }
        for cancel_at in [2, 3] {
            let calls = std::cell::Cell::new(0);
            assert!(
                load_png_thumbnail(&path, || {
                    calls.set(calls.get() + 1);
                    calls.get() == cancel_at
                })
                .is_err()
            );
            assert_eq!(calls.get(), cancel_at);
        }
        std::fs::write(&path, b"invalid PNG")?;
        assert!(load_png_thumbnail(&path, || false).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "short optimized synthetic 4K history preview comparison; no desktop or clipboard"]
    fn history_preview_4k_cost() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "screenshots-preview-cost-{}.png",
            uuid::Uuid::new_v4()
        ));
        image::RgbaImage::from_fn(3840, 2160, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8, 255])
        })
        .save(&path)?;
        let mut decode = Vec::new();
        let mut previews = Vec::new();
        let mut editor = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            std::hint::black_box(load_saved_png(&path)?);
            decode.push(start.elapsed());
            let start = std::time::Instant::now();
            let preview = load_png_thumbnail(&path, || false)?;
            previews.push(start.elapsed());
            assert_eq!(preview.dimensions(), (400, 225));
            assert_eq!(preview.as_raw().len(), 360_000);
            let start = std::time::Instant::now();
            std::hint::black_box(load_editor_image(&path)?);
            editor.push(start.elapsed());
        }
        decode.sort_unstable();
        previews.sort_unstable();
        editor.sort_unstable();
        eprintln!(
            "4K editor image decode median={:?}; 5 synthetic warm samples",
            editor[2]
        );
        eprintln!(
            "4K PNG decode median={:?}; decode+thumbnail median={:?}; retained=360000 bytes; source RGBA=33177600 bytes; 5 synthetic warm samples",
            decode[2], previews[2]
        );
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    #[ignore = "uses the system clipboard and replaces its contents"]
    fn png_round_trip() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("clipboard-spike-{}.png", uuid::Uuid::new_v4()));
        let image = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 80) as u8, (y * 120) as u8, 47, 255])
        });
        image.save(&path)?;
        let mut clipboard = ImageClipboard::default();
        let result = clipboard.copy_image(&image);
        let blocked_parent =
            std::env::temp_dir().join(format!("save-failure-{}", uuid::Uuid::new_v4()));
        std::fs::write(&blocked_parent, b"not a directory")?;
        let shot = crate::CapturedImage {
            display_id: "test".into(),
            window_id: None,
            width: image.width(),
            height: image.height(),
            pixels: std::sync::Arc::new(image.clone()),
            png_path: blocked_parent.join("capture.png"),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        assert!(shot.save_png().is_err());
        std::fs::remove_file(blocked_parent)?;
        let pasted = clipboard
            .clipboard
            .as_mut()
            .unwrap()
            .get_image()
            .context("read clipboard image")?;
        anyhow::ensure!(
            pasted.width == image.width() as usize
                && pasted.height == image.height() as usize
                && pasted.bytes.as_ref() == image.as_raw().as_slice(),
            "clipboard round trip changed image dimensions or RGBA pixels"
        );

        std::fs::remove_file(path)?;
        assert_eq!(result?, (3, 2));
        Ok(())
    }
}
