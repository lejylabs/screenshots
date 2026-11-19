//! Bounded saved capture/Library inputs. Default exports preserve encoded bytes.

use crate::{CaptureFormatPolicy, ImageExportFormat, LibraryExportPolicy};
use anyhow::{Context, Result, ensure};
use image::metadata::Orientation;
use serde::Serialize;
use std::{
    fs::File,
    io::{BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SavedImageInfo {
    pub format: ImageExportFormat,
    /// PNG keeps its historical raw orientation. JPEG/WebP use oriented dimensions.
    pub width: u32,
    pub height: u32,
    pub encoded_bytes: u64,
}

impl SavedImageInfo {
    pub fn mime_type(self) -> &'static str {
        crate::capture_format::image_mime_type(self.format)
    }
}

fn check(cancelled: &impl Fn() -> bool, start: Instant) -> Result<()> {
    ensure!(!cancelled(), crate::CaptureError::Canceled);
    ensure!(
        start.elapsed() < Duration::from_secs(60),
        "Saved image operation exceeded 60 seconds"
    );
    Ok(())
}

/// The legacy extent adapter pins readable bytes even if the held object grows.
/// This wrapper adds operation checks and bounded reads before the codec sees it.
struct SavedPngReader<'a, C: Fn() -> bool> {
    inner: crate::clipboard::EditorFile<&'a File>,
    length: u64,
    cancelled: &'a C,
    start: Instant,
}

impl<'a, C: Fn() -> bool> SavedPngReader<'a, C> {
    fn new(file: &'a File, length: u64, cancelled: &'a C, start: Instant) -> std::io::Result<Self> {
        if length > crate::capture_format::MAX_SAVED_IMAGE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Saved PNG admitted extent exceeds 272 MiB",
            ));
        }
        check(cancelled, start).map_err(|error| std::io::Error::other(error.to_string()))?;
        let result = Self {
            inner: crate::clipboard::EditorFile::with_extent(file, length)?,
            length,
            cancelled,
            start,
        };
        result.check()?;
        Ok(result)
    }
    fn check(&self) -> std::io::Result<()> {
        if (self.cancelled)() {
            return Err(std::io::Error::other(crate::CaptureError::Canceled));
        }
        if self.start.elapsed() >= Duration::from_secs(60) {
            return Err(std::io::Error::other(
                "Saved image operation exceeded 60 seconds",
            ));
        }
        Ok(())
    }
}

impl<C: Fn() -> bool> Read for SavedPngReader<'_, C> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.check()?;
        let maximum = bytes.len().min(64 * 1024);
        let count = self.inner.read(&mut bytes[..maximum])?;
        self.check()?;
        Ok(count)
    }
}

impl<C: Fn() -> bool> Seek for SavedPngReader<'_, C> {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        let position = match from {
            std::io::SeekFrom::Start(value) => i128::from(value),
            std::io::SeekFrom::End(offset) => i128::from(self.length) + i128::from(offset),
            std::io::SeekFrom::Current(offset) => {
                i128::from(self.inner.stream_position()?) + i128::from(offset)
            }
        };
        if !(0..=i128::from(self.length)).contains(&position) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Saved PNG seek exceeds its admitted extent",
            ));
        }
        let position = self.inner.seek(std::io::SeekFrom::Start(position as u64))?;
        self.check()?;
        Ok(position)
    }
}

fn oriented(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (height, width)
    } else {
        (width, height)
    }
}

fn open(path: &Path) -> Result<(File, crate::FileIdentity)> {
    // Preflight also prevents opening a Windows named pipe; Unix adds O_NONBLOCK.
    ensure!(
        path.metadata()?.is_file(),
        "Saved image must be a regular file"
    );
    let opened = crate::FileIdentity::open(path)?;
    ensure!(
        (8..=crate::capture_format::MAX_SAVED_IMAGE_BYTES).contains(&opened.0.metadata()?.len()),
        "Saved image must contain 8 bytes to 272 MiB"
    );
    Ok(opened)
}

fn kind(header: &[u8]) -> Result<ImageExportFormat> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(ImageExportFormat::Png);
    }
    if header.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(ImageExportFormat::Jpeg);
    }
    if header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        return Ok(ImageExportFormat::WebP);
    }
    anyhow::bail!("Saved image must contain PNG, JPEG or WebP data")
}

/// Inspect the retained file, independent of its extension. This admits dimensions,
/// not every compressed pixel; malformed image data can still fail explicit decode.
pub(crate) fn inspect_saved_image_file(
    mut file: &File,
    cancelled: &impl Fn() -> bool,
) -> Result<SavedImageInfo> {
    let start = Instant::now();
    check(cancelled, start)?;
    let admitted = crate::FileIdentity::from_file(file)?;
    let length = file.metadata()?.len();
    ensure!(
        (8..=crate::capture_format::MAX_SAVED_IMAGE_BYTES).contains(&length),
        "Saved image exceeds its encoded bound"
    );
    file.rewind()?;
    let mut signature = [0; 12];
    let count = signature.len().min(usize::try_from(length)?);
    file.read_exact(&mut signature[..count])?;
    let format = kind(&signature[..count])?;
    ensure!(
        length <= crate::capture_format::encoded_limit(format),
        "JPEG/WebP saved images exceed 32 MiB"
    );
    let (width, height) = match format {
        ImageExportFormat::Png => {
            let mut header = [0; 33];
            header[..count].copy_from_slice(&signature[..count]);
            file.read_exact(&mut header[count..])?;
            ensure!(
                header[8..12] == [0, 0, 0, 13]
                    && &header[12..16] == b"IHDR"
                    && crc32fast::hash(&header[12..29])
                        == u32::from_be_bytes(header[29..33].try_into()?),
                "Invalid saved PNG header"
            );
            ensure!(
                matches!(
                    (header[25], header[24]),
                    (0, 1 | 2 | 4 | 8 | 16) | (2, 8 | 16) | (3, 1 | 2 | 4 | 8) | (4 | 6, 8 | 16)
                ) && header[26] == 0
                    && header[27] == 0
                    && header[28] <= 1,
                "Unsupported PNG header"
            );
            (
                u32::from_be_bytes(header[16..20].try_into()?),
                u32::from_be_bytes(header[20..24].try_into()?),
            )
        }
        ImageExportFormat::Jpeg => {
            file.rewind()?;
            jpeg_dimensions(
                BufReader::with_capacity(64 * 1024, (&mut file).take(length)),
                cancelled,
                start,
            )?
        }
        ImageExportFormat::WebP => {
            let bytes = snapshot(file, length, cancelled, start)?;
            let orientation = crate::clipboard::validate_webp(&bytes, crate::MAX_CAPTURE_BYTES)?;
            let payload = bytes.get(20..).context("Truncated WebP image header")?;
            let dimensions = if &bytes[12..16] == b"VP8X" {
                ensure!(payload.len() >= 10, "Truncated WebP canvas");
                (
                    u32::from_le_bytes([payload[4], payload[5], payload[6], 0]) + 1,
                    u32::from_le_bytes([payload[7], payload[8], payload[9], 0]) + 1,
                )
            } else {
                crate::clipboard::webp_bitstream(
                    bytes[12..16].try_into()?,
                    payload,
                    crate::MAX_CAPTURE_BYTES,
                )?
            };
            oriented(dimensions.0, dimensions.1, orientation)
        }
    };
    crate::validate_image_size(width, height)?;
    admitted.verify_file(file)?;
    check(cancelled, start)?;
    file.rewind()?;
    Ok(SavedImageInfo {
        format,
        width,
        height,
        encoded_bytes: length,
    })
}

fn jpeg_dimensions(
    mut reader: impl Read,
    cancelled: &impl Fn() -> bool,
    start: Instant,
) -> Result<(u32, u32)> {
    let mut soi = [0; 2];
    reader.read_exact(&mut soi)?;
    ensure!(soi == [0xff, 0xd8], "Invalid JPEG start marker");
    let mut dimensions = None;
    let mut orientation = Orientation::NoTransforms;
    let mut seen_exif = false;
    let mut payload = Vec::with_capacity(65533);
    for _ in 0..4096 {
        check(cancelled, start)?;
        let mut marker = [0; 2];
        reader.read_exact(&mut marker)?;
        ensure!(marker[0] == 0xff && marker[1] != 0, "Invalid JPEG marker");
        let mut fill = 0;
        while marker[1] == 0xff {
            fill += 1;
            ensure!(fill <= 4096, "JPEG marker has excessive fill bytes");
            reader.read_exact(&mut marker[1..])?;
        }
        ensure!(marker[1] != 0, "Invalid JPEG marker stuffing before scan");
        if marker[1] == 0xda {
            let (width, height) = dimensions.context("JPEG has no supported image dimensions")?;
            return Ok(oriented(width, height, orientation));
        }
        ensure!(
            !matches!(marker[1], 0xd8 | 0xd9 | 0xd0..=0xd7),
            "Unexpected JPEG standalone marker"
        );
        if marker[1] == 0x01 {
            continue;
        }
        let mut size = [0; 2];
        reader.read_exact(&mut size)?;
        let length = usize::from(u16::from_be_bytes(size));
        ensure!(length >= 2, "Invalid JPEG segment length");
        payload.resize(length - 2, 0);
        reader.read_exact(&mut payload)?;
        match marker[1] {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                ensure!(
                    dimensions.is_none() && payload.len() >= 6,
                    "Ambiguous or truncated JPEG dimensions"
                );
                let width = u32::from(u16::from_be_bytes(payload[3..5].try_into()?));
                let height = u32::from(u16::from_be_bytes(payload[1..3].try_into()?));
                crate::validate_image_size(width, height)?;
                dimensions = Some((width, height));
            }
            0xe1 if payload.starts_with(b"Exif\0\0") => {
                ensure!(!seen_exif, "Ambiguous JPEG EXIF metadata");
                seen_exif = true;
                orientation = Orientation::from_exif_chunk(&payload[6..])
                    .unwrap_or(Orientation::NoTransforms);
            }
            _ => (),
        }
    }
    anyhow::bail!("JPEG exceeds 4096 header segments")
}

fn snapshot(
    mut file: &File,
    length: u64,
    cancelled: &impl Fn() -> bool,
    start: Instant,
) -> Result<Vec<u8>> {
    ensure!(
        length <= crate::capture_format::MAX_SAVED_JPEG_WEBP_BYTES,
        "JPEG/WebP saved input exceeds 32 MiB"
    );
    file.rewind()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(usize::try_from(length)?)?;
    ensure!(
        bytes.capacity() as u64 <= crate::capture_format::MAX_SAVED_JPEG_WEBP_BYTES,
        "Saved input backing exceeds 32 MiB"
    );
    let mut chunk = [0; 64 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        check(cancelled, start)?;
        let count = usize::try_from(remaining.min(chunk.len() as u64))?;
        file.read_exact(&mut chunk[..count])?;
        bytes.extend_from_slice(&chunk[..count]);
        remaining -= count as u64;
    }
    Ok(bytes)
}

pub fn inspect_saved_image(path: &Path) -> Result<SavedImageInfo> {
    let (file, _) = open(path)?;
    inspect_saved_image_file(&file, &|| false)
}

/// Shared native move/recovery inspection. The retained handle is checked before
/// and after hashing; dimensions and MIME come from bounded encoded contents.
pub(crate) fn fingerprint_saved_image_file(
    mut file: &File,
) -> Result<(std::fs::Metadata, String, SavedImageInfo)> {
    use sha2_10::{Digest, Sha256};
    let start = Instant::now();
    let admitted = crate::FileIdentity::from_file(file)?;
    let metadata = file.metadata()?;
    let stop = || start.elapsed() >= Duration::from_secs(30);
    let info = inspect_saved_image_file(file, &stop)?;
    let mut remaining = info.encoded_bytes;
    let mut bytes = [0; 64 * 1024];
    let mut hash = Sha256::new();
    while remaining > 0 {
        ensure!(!stop(), "History image inspection exceeded 30 seconds");
        let count = usize::try_from(remaining.min(bytes.len() as u64))?;
        file.read_exact(&mut bytes[..count])?;
        hash.update(&bytes[..count]);
        remaining -= count as u64;
    }
    admitted.verify_file(file)?;
    file.rewind()?;
    Ok((metadata, format!("{:x}", hash.finalize()), info))
}

/// Decode only for an explicit consumer. PNG retains legacy saved-PNG semantics;
/// JPEG/WebP use the existing bounded orientation/color-managed input pipeline.
pub fn load_saved_image(path: &Path, cancelled: impl Fn() -> bool) -> Result<image::RgbaImage> {
    let cancelled = crate::image_cancel::latch(cancelled);
    let start = Instant::now();
    check(&cancelled, start)?;
    let (file, admitted) = open(path)?;
    let info = inspect_saved_image_file(&file, &cancelled)?;
    let pixels = if info.format == ImageExportFormat::Png {
        crate::clipboard::decode_saved_png(
            BufReader::with_capacity(
                64 * 1024,
                SavedPngReader::new(&file, info.encoded_bytes, &cancelled, start)?,
            ),
            crate::MAX_CAPTURE_BYTES,
        )?
    } else {
        let bytes = snapshot(&file, info.encoded_bytes, &cancelled, start)?;
        admitted.verify_file(&file)?;
        crate::decode_editor_image_bytes_with_backing_and_cancel(
            &bytes,
            bytes.capacity(),
            &cancelled,
        )?
    };
    admitted.verify_file(&file)?;
    check(&cancelled, start)?;
    ensure!(
        pixels.dimensions() == (info.width, info.height),
        "Saved image pixels differ from admitted dimensions"
    );
    Ok(pixels)
}

pub fn load_image_thumbnail(path: &Path, cancelled: impl Fn() -> bool) -> Result<image::RgbaImage> {
    let pixels = load_saved_image(path, &cancelled)?;
    crate::history_thumbnail::history_thumbnail_rgba(&pixels, cancelled)
}

/// Metadata-free upload representation, always PNG with the PNG upload MIME/name.
pub fn saved_image_png_without_metadata(
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<u8>> {
    let cancelled = crate::image_cancel::latch(cancelled);
    let start = Instant::now();
    let stop = || cancelled() || start.elapsed() >= Duration::from_secs(60);
    let (file, admitted) = open(path)?;
    inspect_saved_image_file(&file, &stop)?;
    let pixels = crate::load_editor_image_with_identity_and_limit_with_cancel(
        path,
        &admitted,
        crate::MAX_CAPTURE_BYTES,
        &stop,
    )?;
    admitted.verify_file(&file)?;
    let mut output = BoundedPngBytes(Vec::new());
    CaptureFormatPolicy::Png.encode_to(&pixels, &mut output, &stop)?;
    Ok(output.0)
}

struct BoundedPngBytes(Vec<u8>);
impl Write for BoundedPngBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let needed = self
            .0
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("PNG buffer size overflow"))?;
        let limit = crate::capture_format::MAX_SAVED_IMAGE_BYTES as usize;
        if needed > limit {
            return Err(std::io::Error::other(
                "PNG upload representation exceeds 272 MiB",
            ));
        }
        if needed > self.0.capacity() {
            let capacity = needed.div_ceil(65536).saturating_mul(65536).min(limit);
            self.0
                .try_reserve_exact(capacity - self.0.len())
                .map_err(std::io::Error::other)?;
        }
        if self.0.capacity() > limit {
            return Err(std::io::Error::other("PNG backing exceeds 272 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Original-size copies of the same kind preserve bytes. Changing kind or size
/// explicitly encodes decoded pixels once; the PNG-specific wrapper stays intact.
pub fn export_saved_image_as(
    source: &Path,
    destination: &Path,
    jpeg_quality: u8,
    size: crate::ExportSize,
) -> Result<ImageExportFormat> {
    let started = Instant::now();
    let stop = || started.elapsed() >= Duration::from_secs(60);
    let format = ImageExportFormat::from_path(destination)?;
    size.validate()?;
    let policy = match format {
        ImageExportFormat::Png => CaptureFormatPolicy::Png,
        ImageExportFormat::Jpeg => CaptureFormatPolicy::Jpeg {
            quality: crate::CaptureImageQuality::new(jpeg_quality)?,
        },
        ImageExportFormat::WebP => CaptureFormatPolicy::WebpLossless,
    };
    let (mut file, admitted) = open(source)?;
    let info = inspect_saved_image_file(&file, &stop)?;
    if size == crate::ExportSize::Original && info.format == format {
        crate::capture::write_output_checked(
            destination,
            |out| {
                copy_saved_image_to(&mut file, &admitted, info, out, &stop)
                    .map_err(|e| crate::CaptureError::Backend(format!("{e:#}")))
            },
            || {
                if stop() {
                    Err(crate::CaptureError::Backend(
                        "Save as exceeded 60 seconds".into(),
                    ))
                } else {
                    Ok(())
                }
            },
        )?;
    } else {
        let pixels = crate::load_editor_image_with_identity_and_limit_with_cancel(
            source,
            &admitted,
            crate::MAX_CAPTURE_BYTES,
            &stop,
        )?;
        admitted.verify_file(&file)?;
        let dimensions = size.dimensions(pixels.width(), pixels.height())?;
        policy.validate_dimensions(dimensions.0, dimensions.1)?;
        if format == ImageExportFormat::Jpeg {
            let mut work = crate::image_cancel::ImageWork::new(&stop)?;
            for pixel in pixels.pixels() {
                work.pixel()?;
                ensure!(
                    pixel[3] == 255,
                    "JPEG cannot preserve transparency. Choose PNG or WebP."
                );
            }
        }
        let scaled;
        let pixels = if dimensions == pixels.dimensions() {
            &pixels
        } else {
            scaled = crate::downscale_export_with_cancel(&pixels, size, &stop)?;
            &scaled
        };
        crate::capture::write_output_checked(
            destination,
            |out| {
                policy
                    .encode_to(pixels, out, &stop)
                    .map_err(|e| crate::CaptureError::Backend(format!("{e:#}")))
            },
            || {
                if stop() {
                    Err(crate::CaptureError::Backend(
                        "Save as exceeded 60 seconds".into(),
                    ))
                } else {
                    Ok(())
                }
            },
        )?;
    }
    Ok(format)
}

/// Bounded streaming copy from the exact admitted source, reusable by the journal.
pub(crate) fn copy_saved_image_to(
    file: &mut File,
    admitted: &crate::FileIdentity,
    info: SavedImageInfo,
    writer: &mut dyn Write,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    let start = Instant::now();
    check(cancelled, start)?;
    admitted.verify_file(file)?;
    ensure!(
        file.metadata()?.len() == info.encoded_bytes
            && info.encoded_bytes <= crate::capture_format::encoded_limit(info.format),
        "Saved source changed before copy"
    );
    file.rewind()?;
    let mut remaining = info.encoded_bytes;
    let mut buffer = [0; 64 * 1024];
    while remaining > 0 {
        check(cancelled, start)?;
        let count = usize::try_from(remaining.min(buffer.len() as u64))?;
        file.read_exact(&mut buffer[..count])?;
        writer.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    admitted.verify_file(file)?;
    check(cancelled, start)?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SavedImageExport {
    pub source: PathBuf,
    pub path: PathBuf,
    pub format: ImageExportFormat,
    pub reencoded: bool,
    pub lossy: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct SavedImageExportReport {
    pub outputs: Vec<SavedImageExport>,
    pub failures: Vec<(PathBuf, String)>,
    pub not_exported: usize,
}

/// Explicit selected-page export. Sanitized Preserve creates a lossless PNG;
/// configured conversion reports its actual format and any explicit lossy encode.
pub fn export_saved_images(
    sources: &[PathBuf],
    directory: &Path,
    policy: LibraryExportPolicy,
    strip_metadata: bool,
    cancelled: impl Fn() -> bool,
) -> Result<SavedImageExportReport> {
    let cancelled = crate::image_cancel::latch(cancelled);
    ensure!(
        !sources.is_empty() && sources.len() <= 20,
        "Select 1 to 20 images"
    );
    let mut report = SavedImageExportReport::default();
    if cancelled() {
        report.not_exported = sources.len();
        return Ok(report);
    }
    ensure!(
        directory.is_absolute() && directory.is_dir(),
        "Choose an existing absolute export folder"
    );
    for (index, source) in sources.iter().enumerate() {
        if cancelled() {
            report.not_exported = sources.len() - index;
            break;
        }
        let result = export_one(source, directory, policy, strip_metadata, &cancelled);
        match result {
            Ok(output) => report.outputs.push(output),
            Err(error)
                if cancelled()
                    || error
                        .downcast_ref::<crate::CaptureError>()
                        .is_some_and(|e| matches!(e, crate::CaptureError::Canceled)) =>
            {
                report.not_exported = sources.len() - index;
                break;
            }
            Err(error) => report.failures.push((source.clone(), format!("{error:#}"))),
        }
    }
    Ok(report)
}

fn export_one(
    source: &Path,
    directory: &Path,
    policy: LibraryExportPolicy,
    strip_metadata: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<SavedImageExport> {
    let start = Instant::now();
    check(cancelled, start)?;
    ensure!(source.is_absolute(), "Saved image path must be absolute");
    let (mut file, admitted) = open(source)?;
    let info = inspect_saved_image_file(&file, cancelled)?;
    let encoding = match policy {
        LibraryExportPolicy::Preserve if !strip_metadata => None,
        LibraryExportPolicy::Preserve => Some(CaptureFormatPolicy::Png),
        LibraryExportPolicy::Convert { encoding } => Some(encoding),
    };
    let format = encoding.map_or(info.format, CaptureFormatPolicy::format);
    let name = source.file_name().context("Invalid saved image filename")?;
    #[cfg(target_os = "windows")]
    ensure!(
        !name.to_string_lossy().contains(':'),
        "Alternate data streams cannot be exported"
    );
    let mut destination = directory.join(name);
    if encoding.is_some() || ImageExportFormat::from_path(&destination).ok() != Some(format) {
        destination.set_extension(format.extension());
    }
    if let Some(encoding) = encoding {
        admitted.verify_file(&file)?;
        let pixels = crate::load_editor_image_with_identity_and_limit_with_cancel(
            source,
            &admitted,
            crate::MAX_CAPTURE_BYTES,
            cancelled,
        )?;
        admitted.verify_file(&file)?;
        encoding.validate_pixels(&pixels, cancelled)?;
        crate::capture::write_output_checked(
            &destination,
            |out| {
                encoding
                    .encode_to(&pixels, out, cancelled)
                    .map_err(|e| crate::CaptureError::Backend(format!("{e:#}")))
            },
            || {
                check(cancelled, start).map_err(|e| {
                    if cancelled() {
                        crate::CaptureError::Canceled
                    } else {
                        crate::CaptureError::Backend(format!("{e:#}"))
                    }
                })
            },
        )?;
    } else {
        crate::capture::write_output_checked(
            &destination,
            |out| {
                copy_saved_image_to(&mut file, &admitted, info, out, cancelled).map_err(|e| {
                    if cancelled() {
                        crate::CaptureError::Canceled
                    } else {
                        crate::CaptureError::Backend(format!("{e:#}"))
                    }
                })
            },
            || {
                check(cancelled, start).map_err(|e| {
                    if cancelled() {
                        crate::CaptureError::Canceled
                    } else {
                        crate::CaptureError::Backend(format!("{e:#}"))
                    }
                })
            },
        )?;
    }
    Ok(SavedImageExport {
        source: source.to_owned(),
        path: destination,
        format,
        reencoded: encoding.is_some(),
        lossy: encoding.is_some_and(CaptureFormatPolicy::is_lossy),
    })
}

#[cfg(test)]
#[path = "saved_image_format_tests.rs"]
mod tests;
