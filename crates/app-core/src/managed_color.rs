//! Explicit editor imports become straight-alpha, 8-bit SDR sRGB once.
//! Saved/library PNG decoding and native capture use their separate contracts.

use std::io::{BufRead, Read, Seek, SeekFrom};

use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbaImage, metadata::Orientation};
use moxcms::{ColorProfile, Layout, ParsingOptions, RenderingIntent, TransformOptions};

const MAX_INPUT: usize = 32 * 1024 * 1024;
const MAX_PROFILE: usize = 256 * 1024;
const MAX_TAGS: usize = 64;
const MAX_TRC: usize = 4096;
const MAX_CHUNKS: usize = 4096;
const WORK_BYTES: usize = 64 * 1024;
const BATCH_PIXELS: usize = 512;
// moxcms uses 65536-entry f32 source/destination tables: at most 1.5 MiB for RGB.
const COLOR_SCRATCH: u64 = 4 * 1024 * 1024;

#[cfg(test)]
std::thread_local! {
    static INPUT_READ_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INPUT_INDEX_ALLOCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy)]
struct Segment {
    source: usize,
    length: usize,
    logical: usize,
}

/// A borrowed PNG/JPEG view prevents the codec from expanding discarded metadata.
/// The index is bounded by the admitted container's chunk count, not its byte size.
struct Input<'a, C: Fn() -> bool> {
    bytes: &'a [u8],
    segments: Vec<Segment>,
    length: usize,
    position: usize,
    cancelled: &'a C,
}
impl<'a, C: Fn() -> bool> Input<'a, C> {
    fn new(bytes: &'a [u8], cancelled: &'a C) -> Self {
        #[cfg(test)]
        INPUT_INDEX_ALLOCS.with(|calls| calls.set(calls.get() + 1));
        Self {
            bytes,
            segments: Vec::with_capacity(MAX_CHUNKS + 1),
            length: 0,
            position: 0,
            cancelled,
        }
    }
    fn push(&mut self, start: usize, end: usize) -> Result<()> {
        ensure!(
            start <= end && end <= self.bytes.len(),
            "Invalid image input segment"
        );
        if start == end {
            return Ok(());
        }
        ensure!(
            self.segments.len() <= MAX_CHUNKS,
            "Image has too many retained segments"
        );
        self.segments.push(Segment {
            source: start,
            length: end - start,
            logical: self.length,
        });
        self.length = self
            .length
            .checked_add(end - start)
            .context("Image view size overflow")?;
        Ok(())
    }
    fn check(&self) -> std::io::Result<()> {
        if (self.cancelled)() {
            return Err(std::io::Error::other(crate::CaptureError::Canceled));
        }
        Ok(())
    }
}
impl<C: Fn() -> bool> BufRead for Input<'_, C> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.check()?;
        if self.position >= self.length {
            return Ok(&[]);
        }
        let index = self
            .segments
            .partition_point(|segment| segment.logical <= self.position)
            - 1;
        let segment = self.segments[index];
        let offset = self.position - segment.logical;
        let count = (segment.length - offset).min(WORK_BYTES);
        Ok(&self.bytes[segment.source + offset..segment.source + offset + count])
    }
    fn consume(&mut self, amount: usize) {
        self.position = self.position.saturating_add(amount).min(self.length);
    }
}
impl<C: Fn() -> bool> Read for Input<'_, C> {
    fn read(&mut self, destination: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        INPUT_READ_CALLS.with(|calls| calls.set(calls.get() + 1));
        let source = self.fill_buf()?;
        let count = destination.len().min(source.len());
        destination[..count].copy_from_slice(&source[..count]);
        self.consume(count);
        Ok(count)
    }
}
impl<C: Fn() -> bool> Seek for Input<'_, C> {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        let value = match from {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::Current(value) => self.position as i128 + i128::from(value),
            SeekFrom::End(value) => self.length as i128 + i128::from(value),
        };
        self.position =
            usize::try_from(value).map_err(|_| std::io::Error::other("Invalid image view seek"))?;
        Ok(self.position as u64)
    }
}

fn region(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(length)
                    .context("Image metadata extent overflow")?,
        )
        .context("Truncated image metadata")
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(region(bytes, offset, 4)?.try_into()?))
}
fn check(cancelled: &impl Fn() -> bool) -> Result<()> {
    ensure!(!cancelled(), crate::CaptureError::Canceled);
    Ok(())
}
fn append(destination: &mut Vec<u8>, source: &[u8], cancelled: &impl Fn() -> bool) -> Result<()> {
    for chunk in source.chunks(WORK_BYTES) {
        check(cancelled)?;
        destination.extend_from_slice(chunk);
    }
    Ok(())
}

fn trc(bytes: &[u8]) -> Result<()> {
    match region(bytes, 0, 4)? {
        b"curv" => {
            let count = be32(bytes, 8)? as usize;
            ensure!(
                count <= MAX_TRC && bytes.len() == 12 + count * 2,
                "ICC TRC exceeds 4096 entries or has an invalid extent"
            );
            let mut previous = 0;
            for value in bytes[12..].chunks_exact(2) {
                let value = u16::from_be_bytes(value.try_into()?);
                ensure!(count < 2 || value >= previous, "ICC TRC must be monotonic");
                previous = value;
            }
            if count == 1 {
                let gamma = f64::from(previous) / 256.0;
                ensure!(gamma > 0.0 && gamma <= 10.0, "Unsupported ICC gamma");
            }
        }
        b"para" => {
            let kind = u16::from_be_bytes(region(bytes, 8, 2)?.try_into()?);
            ensure!(
                region(bytes, 10, 2)? == [0, 0],
                "Nonzero ICC parametric reserved bytes"
            );
            let count = *[1usize, 3, 4, 5, 7]
                .get(usize::from(kind))
                .context("Unsupported ICC parametric curve")?;
            ensure!(
                bytes.len() == 12 + count * 4,
                "Invalid ICC parametric curve extent"
            );
            let mut p = [0.0f64; 7];
            for (index, value) in bytes[12..].chunks_exact(4).enumerate() {
                p[index] = f64::from(i32::from_be_bytes(value.try_into()?)) / 65536.0;
            }
            ensure!(
                p[..count]
                    .iter()
                    .all(|value| f64::from(*value as f32) == *value),
                "ICC parametric coefficients exceed the supported CMS precision"
            );
            ensure!(
                p[0] > 0.0 && p[0] <= 10.0 && (kind == 0 || p[1] > 0.0),
                "Unsupported ICC parametric coefficients"
            );
            parametric_trc(kind, p)?;
        }
        _ => anyhow::bail!("Only ICC curve and parametric RGB TRCs are supported"),
    }
    Ok(())
}

fn parametric_trc(kind: u16, p: [f64; 7]) -> Result<()> {
    // ICC s15Fixed16 rounding can move a legitimate junction by a few units.
    // Domain checks are exact: even a tiny negative fractional-power base is invalid.
    const ROUNDING: f64 = 8.0 / 65536.0;
    let in_range = |value: f64| {
        ensure!(
            value.is_finite() && (-ROUNDING..=1.0 + ROUNDING).contains(&value),
            "Unsupported nonfinite or extended ICC TRC range"
        );
        Ok::<_, anyhow::Error>(())
    };
    if kind == 0 {
        return Ok(()); // Positive gamma is finite and increasing throughout [0, 1].
    }
    if kind <= 2 {
        let junction = -p[2] / p[1];
        let offset = if kind == 2 { p[3] } else { 0.0 };
        if junction > 0.0 {
            in_range(offset)?;
        }
        if junction <= 1.0 {
            // At -b/a the mathematical base is exactly zero; avoid division roundoff.
            let low = if junction >= 0.0 { 0.0 } else { p[2] };
            let high = p[1] + p[2];
            ensure!(low >= 0.0 && high >= 0.0, "Invalid ICC power-branch domain");
            in_range(low.powf(p[0]) + offset)?;
            in_range(high.powf(p[0]) + offset)?;
        }
        let cms_junction = -(p[2] as f32) / p[1] as f32;
        if cms_junction <= 1.0 {
            // moxcms represents the derived branch threshold in f32. Its rounding
            // must not activate a negative-base interval below the exact threshold.
            ensure!(
                (p[1] as f32) * cms_junction.max(0.0) + p[2] as f32 >= 0.0,
                "ICC derived power boundary exceeds the supported CMS precision"
            );
        }
        return Ok(());
    }
    let junction = p[4];
    let power_offset = if kind == 4 { p[5] } else { 0.0 };
    let linear_offset = if kind == 4 { p[6] } else { 0.0 };
    let low_active = junction > 0.0;
    let high_active = junction <= 1.0;
    if low_active {
        ensure!(p[3] >= 0.0, "Nonmonotonic ICC linear branch");
        in_range(linear_offset)?;
        in_range(p[3] * junction.min(1.0) + linear_offset)?;
    }
    if high_active {
        let low_base = p[1] * junction.max(0.0) + p[2];
        let high_base = p[1] + p[2];
        ensure!(
            low_base >= 0.0 && high_base >= 0.0,
            "Invalid ICC power-branch domain"
        );
        let low = low_base.powf(p[0]) + power_offset;
        in_range(low)?;
        in_range(high_base.powf(p[0]) + power_offset)?;
        if low_active {
            let left = p[3] * junction + linear_offset;
            ensure!(
                (low - left).abs() <= ROUNDING,
                "Unsupported discontinuous or nonmonotonic ICC TRC junction"
            );
        }
    }
    Ok(())
}

fn icc(bytes: &[u8], cancelled: &impl Fn() -> bool) -> Result<ColorProfile> {
    check(cancelled)?;
    ensure!(
        (132..=MAX_PROFILE).contains(&bytes.len()) && be32(bytes, 0)? as usize == bytes.len(),
        "ICC profile must have an exact extent within 256 KiB"
    );
    ensure!(
        region(bytes, 36, 4)? == b"acsp" && matches!(bytes[8], 2 | 4),
        "Unsupported ICC signature or version"
    );
    ensure!(
        matches!(region(bytes, 12, 4)?, b"mntr" | b"scnr")
            && region(bytes, 16, 4)? == b"RGB "
            && region(bytes, 20, 4)? == b"XYZ ",
        "Only RGB display/input matrix ICC profiles with XYZ PCS are supported; CMYK/HDR/CLUT profiles are not converted"
    );
    let count = be32(bytes, 128)? as usize;
    ensure!((1..=MAX_TAGS).contains(&count), "ICC tag count exceeds 64");
    let table_end = 132 + count * 12;
    region(bytes, 0, table_end)?;
    let mut tags: Vec<([u8; 4], usize, usize)> = Vec::with_capacity(count);
    let mut retained = Vec::with_capacity(9);
    let mut required = 0u8;
    for entry in bytes[132..table_end].chunks_exact(12) {
        check(cancelled)?;
        let signature: [u8; 4] = entry[..4].try_into()?;
        let offset = be32(entry, 4)? as usize;
        let length = be32(entry, 8)? as usize;
        ensure!(
            offset >= table_end && offset.is_multiple_of(4) && length >= 8,
            "Invalid ICC tag extent"
        );
        let data = region(bytes, offset, length)?;
        for (old, start, size) in &tags {
            ensure!(*old != signature, "Duplicate ICC tag");
            ensure!(
                offset == *start && length == *size
                    || offset + length <= *start
                    || *start + *size <= offset,
                "Partially overlapping ICC tags"
            );
        }
        ensure!(
            !matches!(
                &signature,
                b"A2B0"
                    | b"A2B1"
                    | b"A2B2"
                    | b"B2A0"
                    | b"B2A1"
                    | b"B2A2"
                    | b"D2B0"
                    | b"D2B1"
                    | b"D2B2"
                    | b"D2B3"
                    | b"B2D0"
                    | b"B2D1"
                    | b"B2D2"
                    | b"B2D3"
                    | b"gamt"
                    | b"cicp"
            ) && !matches!(&data[..4], b"mft1" | b"mft2" | b"mAB " | b"mBA " | b"mpet"),
            "ICC CLUT/HDR/CICP combinations are unsupported"
        );
        let bit = match &signature {
            b"rXYZ" => 1,
            b"gXYZ" => 2,
            b"bXYZ" => 4,
            b"rTRC" => 8,
            b"gTRC" => 16,
            b"bTRC" => 32,
            b"wtpt" => 64,
            _ => 0,
        };
        if bit != 0 || matches!(&signature, b"chad" | b"bkpt") {
            ensure!(data[4..8] == [0; 4], "Nonzero ICC tag reserved bytes");
            if matches!(&signature, b"rTRC" | b"gTRC" | b"bTRC") {
                trc(data)?;
            } else if signature == *b"chad" {
                ensure!(
                    data.len() == 44 && &data[..4] == b"sf32",
                    "Invalid ICC chromatic adaptation"
                );
            } else {
                ensure!(
                    data.len() == 20 && &data[..4] == b"XYZ ",
                    "Invalid ICC XYZ colorant/white point"
                );
            }
            retained.push(entry);
            required |= bit;
        }
        tags.push((signature, offset, length));
    }
    ensure!(
        required == 127,
        "ICC matrix profile is missing colorants, white point or RGB TRCs"
    );
    // Parse only color-bearing tags. Ancillary mluc/desc counts never reach an allocator.
    let mut bounded = Vec::with_capacity(bytes.len());
    append(&mut bounded, bytes, cancelled)?;
    bounded[128..132].copy_from_slice(&(retained.len() as u32).to_be_bytes());
    for (index, entry) in retained.iter().enumerate() {
        bounded[132 + index * 12..144 + index * 12].copy_from_slice(entry);
    }
    let profile = ColorProfile::new_from_slice_with_options(
        &bounded,
        ParsingOptions {
            max_profile_size: MAX_PROFILE + 1,
            max_allowed_clut_size: 0,
            max_allowed_trc_size: MAX_TRC,
        },
    )
    .context("Could not parse bounded RGB ICC profile")?;
    let matrix = profile.colorant_matrix().v;
    let determinant = matrix[0][0] * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
        - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
        + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
    ensure!(
        matrix
            .iter()
            .flatten()
            .all(|value| value.is_finite() && (-2.0..=4.0).contains(value))
            && determinant.is_finite()
            && determinant.abs() > 0.000001,
        "Nonfinite, singular or unsupported ICC colorant matrix"
    );
    Ok(profile)
}

fn cicp(bytes: &[u8]) -> Result<Option<ColorProfile>> {
    ensure!(bytes.len() == 4, "Invalid PNG CICP extent");
    match bytes {
        [1, 13, 0, 1] => Ok(None),
        [12, 13, 0, 1] => Ok(Some(ColorProfile::new_display_p3())),
        _ => Err(crate::CaptureError::ImageColorPolicy.into()),
    }
}

struct Metadata {
    profile: Option<ColorProfile>,
    orientation: Orientation,
}
fn png<'a, C: Fn() -> bool>(bytes: &'a [u8], cancelled: &'a C) -> Result<(Input<'a, C>, Metadata)> {
    ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Invalid PNG signature"
    );
    let mut input = Input::new(bytes, cancelled);
    input.push(0, 8)?;
    let mut offset = 8usize;
    let mut profile = None;
    let mut orientation = Orientation::NoTransforms;
    let (mut seen_icc, mut seen_cicp, mut seen_srgb, mut seen_exif) = (false, false, false, false);
    let (mut seen_gamma, mut seen_chrm, mut seen_data, mut ended) = (false, false, false, false);
    for _ in 0..MAX_CHUNKS {
        check(cancelled)?;
        let length = be32(bytes, offset)? as usize;
        let kind = region(bytes, offset + 4, 4)?;
        let data = region(bytes, offset + 8, length)?;
        let end = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .context("PNG extent overflow")?;
        region(bytes, offset, end - offset)?;
        let color = matches!(
            kind,
            b"iCCP" | b"cICP" | b"sRGB" | b"gAMA" | b"cHRM" | b"cLLI" | b"mDCV"
        );
        if color || kind == b"eXIf" {
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            for part in data.chunks(WORK_BYTES) {
                check(cancelled)?;
                crc.update(part);
            }
            ensure!(
                crc.finalize() == be32(bytes, end - 4)?,
                "PNG color/EXIF metadata has an invalid CRC"
            );
        }
        ensure!(
            !color || !seen_data,
            "PNG color metadata follows pixel data"
        );
        match kind {
            b"iCCP" => {
                ensure!(
                    !seen_icc && !seen_cicp && !seen_srgb,
                    "Duplicate or conflicting PNG ICC/CICP/sRGB metadata"
                );
                seen_icc = true;
                let name = data
                    .iter()
                    .take(80)
                    .position(|byte| *byte == 0)
                    .context("Invalid PNG ICC name")?;
                ensure!(
                    (1..=79).contains(&name) && data.get(name + 1) == Some(&0),
                    "Invalid PNG ICC name/compression"
                );
                let compressed = region(data, name + 2, data.len().saturating_sub(name + 2))?;
                let mut compressed_input = Input::new(compressed, cancelled);
                compressed_input.push(0, compressed.len())?;
                let mut inflater = flate2::bufread::ZlibDecoder::new(compressed_input);
                let mut expanded = Vec::with_capacity(MAX_PROFILE);
                let mut chunk = [0u8; 8192];
                loop {
                    check(cancelled)?;
                    let count = inflater.read(&mut chunk)?;
                    if count == 0 {
                        break;
                    }
                    ensure!(
                        expanded.len() + count <= MAX_PROFILE,
                        "PNG ICC profile exceeds 256 KiB after decompression"
                    );
                    expanded.extend_from_slice(&chunk[..count]);
                }
                ensure!(
                    inflater.total_in() as usize == compressed.len(),
                    "PNG ICC compressed stream has trailing data"
                );
                profile = Some(icc(&expanded, cancelled)?);
            }
            b"cICP" => {
                ensure!(
                    !seen_cicp && !seen_icc && !seen_srgb,
                    "Duplicate or conflicting PNG ICC/CICP/sRGB metadata"
                );
                seen_cicp = true;
                profile = cicp(data)?;
            }
            b"sRGB" => {
                ensure!(
                    !seen_srgb && !seen_icc && !seen_cicp && data.len() == 1 && data[0] <= 3,
                    "Invalid, duplicate or conflicting PNG sRGB metadata"
                );
                seen_srgb = true;
            }
            b"gAMA" => {
                ensure!(
                    !seen_gamma && data.len() == 4 && be32(data, 0)? == 45455,
                    "Unsupported or duplicate PNG gamma; supply a supported ICC/CICP profile"
                );
                seen_gamma = true;
            }
            b"cHRM" => {
                ensure!(
                    !seen_chrm && data.len() == 32,
                    "Invalid or duplicate PNG chromaticity"
                );
                let canonical = [31270u32, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
                ensure!(
                    data.chunks_exact(4)
                        .zip(canonical)
                        .all(|(v, expected)| u32::from_be_bytes(v.try_into().unwrap()) == expected),
                    "Unsupported PNG chromaticity; supply a supported ICC/CICP profile"
                );
                seen_chrm = true;
            }
            b"cLLI" | b"mDCV" => return Err(crate::CaptureError::ImageColorPolicy.into()),
            b"eXIf" => {
                ensure!(
                    !seen_exif && data.len() <= WORK_BYTES,
                    "Duplicate or oversized PNG EXIF"
                );
                seen_exif = true;
                orientation =
                    Orientation::from_exif_chunk(data).unwrap_or(Orientation::NoTransforms);
            }
            b"IDAT" => seen_data = true,
            b"IEND" => {
                ensure!(length == 0 && end == bytes.len(), "Invalid PNG end extent");
                ended = true;
            }
            _ => {}
        }
        // Text is irrelevant to the editor raster; compressed text must not expand.
        if !color && !matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt") {
            input.push(offset, end)?;
        }
        offset = end;
        if ended {
            break;
        }
    }
    ensure!(
        ended && seen_data,
        "PNG has too many chunks or no complete pixel data"
    );
    ensure!(
        !(seen_gamma || seen_chrm) || seen_srgb || seen_icc || seen_cicp,
        "Legacy PNG gamma/chromaticity alone is unsupported; it is not silently treated as sRGB"
    );
    Ok((
        input,
        Metadata {
            profile,
            orientation,
        },
    ))
}

fn jpeg<'a, C: Fn() -> bool>(
    bytes: &'a [u8],
    backing: usize,
    max_decoded_bytes: u64,
    expected: Option<[u32; 2]>,
    cancelled: &'a C,
) -> Result<(Input<'a, C>, Metadata)> {
    ensure!(bytes.starts_with(&[0xff, 0xd8]), "Invalid JPEG signature");
    let mut input = Input::new(bytes, cancelled);
    input.push(0, 2)?;
    let mut offset = 2usize;
    let mut fragments: Vec<Option<&[u8]>> = vec![None; 255];
    let (mut total, mut profile_bytes, mut exif, mut orientation, mut frame, mut ended) =
        (None, 0usize, false, Orientation::NoTransforms, None, false);
    for _ in 0..MAX_CHUNKS {
        check(cancelled)?;
        let start = offset;
        ensure!(bytes.get(offset) == Some(&0xff), "Invalid JPEG marker");
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
            ensure!(
                offset - start <= WORK_BYTES,
                "Excessive JPEG marker padding"
            );
        }
        let marker = *bytes.get(offset).context("Truncated JPEG marker")?;
        offset += 1;
        if marker == 0xd9 {
            input.push(start, offset)?;
            ended = true;
            break;
        }
        ensure!(
            !matches!(marker, 0 | 0xd8 | 0xd0..=0xd7),
            "Unexpected JPEG standalone marker"
        );
        let length = u16::from_be_bytes(region(bytes, offset, 2)?.try_into()?) as usize;
        ensure!(length >= 2, "Invalid JPEG segment length");
        let end = offset
            .checked_add(length)
            .context("JPEG segment overflow")?;
        let payload = region(bytes, offset + 2, length - 2)?;
        if marker == 0xe2 && payload.starts_with(b"ICC_PROFILE\0") {
            ensure!(payload.len() >= 14, "Truncated JPEG ICC fragment");
            let sequence = usize::from(payload[12]);
            let count = usize::from(payload[13]);
            ensure!(
                count > 0
                    && sequence > 0
                    && sequence <= count
                    && total.is_none_or(|old| old == count),
                "Inconsistent JPEG ICC fragment count/order"
            );
            total = Some(count);
            ensure!(
                fragments[sequence - 1].is_none(),
                "Duplicate JPEG ICC fragment"
            );
            profile_bytes = profile_bytes
                .checked_add(payload.len() - 14)
                .context("JPEG ICC size overflow")?;
            ensure!(
                profile_bytes <= MAX_PROFILE,
                "JPEG ICC fragments exceed 256 KiB"
            );
            fragments[sequence - 1] = Some(&payload[14..]);
        }
        if marker == 0xe1 && payload.starts_with(b"Exif\0\0") {
            ensure!(!exif, "Duplicate JPEG EXIF metadata");
            exif = true;
            orientation =
                Orientation::from_exif_chunk(&payload[6..]).unwrap_or(Orientation::NoTransforms);
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            ensure!(
                frame.is_none()
                    && matches!(marker, 0xc0..=0xc2)
                    && payload.len() >= 6
                    && payload[0] == 8
                    && matches!(payload[5], 1 | 3),
                "Only 8-bit grayscale/RGB JPEG is supported; CMYK and other precision are not converted"
            );
            let height = u32::from(u16::from_be_bytes(payload[1..3].try_into()?));
            let width = u32::from(u16::from_be_bytes(payload[3..5].try_into()?));
            let channels = u64::from(payload[5]);
            ensure!(
                payload.len() == 6 + channels as usize * 3,
                "Invalid JPEG frame extent"
            );
            crate::validate_image_size(width, height)?;
            frame = Some((width, height, channels));
        }
        if !matches!(marker, 0xe1..=0xed | 0xef | 0xfe) {
            input.push(start, end)?;
        }
        offset = end;
        if marker == 0xda {
            let entropy = offset;
            let mut work = crate::image_cancel::ImageWork::new(cancelled)?;
            while offset < bytes.len() {
                work.charge(1)?;
                if bytes[offset] != 0xff {
                    offset += 1;
                    continue;
                }
                let mut next = offset + 1;
                while bytes.get(next) == Some(&0xff) {
                    next += 1;
                    work.charge(1)?;
                }
                let value = *bytes.get(next).context("Truncated JPEG entropy marker")?;
                if value == 0 || (0xd0..=0xd7).contains(&value) {
                    offset = next + 1;
                } else {
                    break;
                }
            }
            input.push(entropy, offset)?;
        }
    }
    ensure!(
        frame.is_some() && ended,
        "JPEG has too many segments or lacks a complete frame"
    );
    let (width, height, channels) = frame.context("JPEG frame header missing")?;
    let source_bytes = u64::from(width) * u64::from(height) * channels;
    let output_bytes = u64::from(width) * u64::from(height) * 4;
    ensure!(
        source_bytes <= max_decoded_bytes && output_bytes <= max_decoded_bytes,
        "Decoded image native precision or RGBA output exceeds the memory limit"
    );
    let dimensions = oriented_size(width, height, orientation);
    ensure!(
        expected.is_none_or(|size| size == [dimensions.0, dimensions.1]),
        "Oriented editor input dimensions differ from source_size"
    );
    // image 0.25.10 copies JPEG input in its constructor, before it applies limits.
    // Admit that known allocation from the immutable SOF header before any decoder read.
    admit_working_set(
        source_bytes,
        output_bytes,
        backing as u64 + input.length as u64 * 2,
        input.segments.capacity() * std::mem::size_of::<Segment>(),
    )?;
    let profile = if let Some(total) = total {
        let mut joined = Vec::with_capacity(profile_bytes);
        for fragment in &fragments[..total] {
            append(
                &mut joined,
                fragment.context("Missing JPEG ICC fragment")?,
                cancelled,
            )?;
        }
        Some(icc(&joined, cancelled)?)
    } else {
        None
    };
    Ok((
        input,
        Metadata {
            profile,
            orientation,
        },
    ))
}

fn webp(bytes: &[u8], cancelled: &impl Fn() -> bool) -> Result<Metadata> {
    let mut profile = None;
    let mut offset = 12usize;
    let mut seen = false;
    let mut exif = false;
    let flagged =
        bytes.get(12..16) == Some(b"VP8X") && bytes.get(20).is_some_and(|flags| flags & 0x20 != 0);
    for _ in 0..MAX_CHUNKS {
        if offset == bytes.len() {
            ensure!(
                seen == flagged,
                "WebP ICC feature flag and profile presence disagree"
            );
            return Ok(Metadata {
                profile,
                orientation: Orientation::NoTransforms,
            });
        }
        check(cancelled)?;
        let kind = region(bytes, offset, 4)?;
        let length = u32::from_le_bytes(region(bytes, offset + 4, 4)?.try_into()?) as usize;
        let data = region(bytes, offset + 8, length)?;
        if kind == b"EXIF" {
            ensure!(
                !exif && data.len() <= WORK_BYTES,
                "Duplicate or oversized WebP EXIF"
            );
            exif = true;
        }
        if kind == b"ICCP" {
            ensure!(
                !seen && length <= MAX_PROFILE,
                "Duplicate or oversized WebP ICC profile"
            );
            seen = true;
            profile = Some(icc(data, cancelled)?);
        }
        offset = offset
            .checked_add(8)
            .and_then(|n| n.checked_add(length))
            .and_then(|n| n.checked_add(length & 1))
            .context("WebP metadata extent overflow")?;
    }
    anyhow::bail!("WebP exceeds its color metadata chunk limit")
}

fn oriented_size(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
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
fn destination(x: u32, y: u32, w: u32, h: u32, orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::NoTransforms => (x, y),
        Orientation::Rotate90 => (h - 1 - y, x),
        Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
        Orientation::Rotate270 => (y, w - 1 - x),
        Orientation::FlipHorizontal => (w - 1 - x, y),
        Orientation::FlipVertical => (x, h - 1 - y),
        Orientation::Rotate90FlipH => (y, x),
        Orientation::Rotate270FlipH => (h - 1 - y, w - 1 - x),
    }
}

enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
}
impl Samples<'_> {
    fn value(&self, index: usize) -> f32 {
        match self {
            Self::U8(v) => f32::from(v[index]) / 255.0,
            Self::U16(v) => f32::from(v[index]) / 65535.0,
        }
    }
    fn alpha(&self, index: usize) -> u8 {
        match self {
            Self::U8(v) => v[index],
            Self::U16(v) => ((u32::from(v[index]) + 128) / 257) as u8,
        }
    }
}
fn convert(
    decoded: DynamicImage,
    profile: Option<&ColorProfile>,
    orientation: Orientation,
    cancelled: &impl Fn() -> bool,
) -> Result<RgbaImage> {
    if profile.is_none()
        && orientation == Orientation::NoTransforms
        && matches!(&decoded, DynamicImage::ImageRgba8(_))
    {
        check(cancelled)?;
        return Ok(decoded.into_rgba8());
    }
    let (samples, channels) = match &decoded {
        DynamicImage::ImageLuma8(v) => (Samples::U8(v.as_raw()), 1),
        DynamicImage::ImageLumaA8(v) => (Samples::U8(v.as_raw()), 2),
        DynamicImage::ImageRgb8(v) => (Samples::U8(v.as_raw()), 3),
        DynamicImage::ImageRgba8(v) => (Samples::U8(v.as_raw()), 4),
        DynamicImage::ImageLuma16(v) => (Samples::U16(v.as_raw()), 1),
        DynamicImage::ImageLumaA16(v) => (Samples::U16(v.as_raw()), 2),
        DynamicImage::ImageRgb16(v) => (Samples::U16(v.as_raw()), 3),
        DynamicImage::ImageRgba16(v) => (Samples::U16(v.as_raw()), 4),
        _ => {
            anyhow::bail!("Floating point/HDR image input requires an explicit tone-mapping policy")
        }
    };
    ensure!(
        profile.is_none() || channels >= 3,
        "RGB ICC/CICP on grayscale source pixels is unsupported"
    );
    let transform = profile
        .map(|source| {
            source.create_transform_f32(
                Layout::Rgb,
                &ColorProfile::new_srgb(),
                Layout::Rgb,
                TransformOptions {
                    rendering_intent: RenderingIntent::RelativeColorimetric,
                    allow_use_cicp_transfer: false,
                    prefer_fixed_point: false,
                    ..Default::default()
                },
            )
        })
        .transpose()
        .context("Could not construct RGB ICC to SDR sRGB transform")?;
    check(cancelled)?;
    let width = decoded.width();
    let height = decoded.height();
    let (out_width, out_height) = oriented_size(width, height, orientation);
    let mut output = RgbaImage::new(out_width, out_height);
    let mut source = vec![
        0.0f32;
        if transform.is_some() {
            BATCH_PIXELS * 3
        } else {
            0
        }
    ];
    let mut target = vec![0.0f32; source.len()];
    for start in (0..width as usize * height as usize).step_by(BATCH_PIXELS) {
        check(cancelled)?;
        let count = BATCH_PIXELS.min(width as usize * height as usize - start);
        if transform.is_some() {
            for index in 0..count {
                let at = (start + index) * channels;
                for channel in 0..3 {
                    source[index * 3 + channel] = samples.value(at + channel);
                }
            }
        }
        if let Some(transform) = &transform {
            transform.transform(&source[..count * 3], &mut target[..count * 3])?;
        }
        let rgb = if transform.is_some() {
            &target
        } else {
            &source
        };
        for index in 0..count {
            let pixel = start + index;
            let at = pixel * channels;
            let (x, y) = destination(
                (pixel % width as usize) as u32,
                (pixel / width as usize) as u32,
                width,
                height,
                orientation,
            );
            let destination =
                &mut output.as_mut()[((y as usize * out_width as usize + x as usize) * 4)..][..4];
            for channel in 0..3 {
                destination[channel] = if transform.is_some() {
                    let value = rgb[index * 3 + channel];
                    ensure!(value.is_finite(), "ICC conversion produced nonfinite RGB");
                    (value.clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    samples.alpha(at + if channels < 3 { 0 } else { channel })
                };
            }
            destination[3] = if channels == 2 || channels == 4 {
                samples.alpha(at + channels - 1)
            } else {
                255
            };
        }
    }
    check(cancelled)?;
    Ok(output)
}

fn admit_working_set(source: u64, output: u64, encoded: u64, index: usize) -> Result<u64> {
    let allowance = source
        .checked_mul(2)
        .and_then(|value| value.checked_add(output * 2))
        .and_then(|value| value.checked_add(32 * 1024 * 1024))
        .context("Managed image working budget overflow")?;
    let reserved = source + output + encoded + COLOR_SCRATCH + index as u64;
    ensure!(
        reserved <= allowance && allowance - reserved >= source,
        "Encoded input and color conversion scratch exceed the active-image memory budget"
    );
    Ok(allowance - reserved)
}

/// Read only fixed headers/borrowed directory entries before allocating an index,
/// profile or decoder. Pixel count is enough for the encoded-overhead admission:
/// the source scratch terms cancel from `allowance - reserved >= source`.
fn header_dimensions(
    bytes: &[u8],
    format: ImageFormat,
    cancelled: &impl Fn() -> bool,
) -> Result<(u32, u32)> {
    match format {
        ImageFormat::Png => {
            ensure!(
                region(bytes, 8, 8)? == b"\0\0\0\rIHDR",
                "Invalid PNG first header"
            );
            Ok((be32(bytes, 16)?, be32(bytes, 20)?))
        }
        ImageFormat::Jpeg => {
            ensure!(bytes.starts_with(&[0xff, 0xd8]), "Invalid JPEG signature");
            let mut offset = 2;
            for _ in 0..MAX_CHUNKS {
                check(cancelled)?;
                let start = offset;
                ensure!(bytes.get(offset) == Some(&0xff), "Invalid JPEG marker");
                while bytes.get(offset) == Some(&0xff) {
                    offset += 1;
                    ensure!(
                        offset - start <= WORK_BYTES,
                        "Excessive JPEG marker padding"
                    );
                }
                let marker = *bytes.get(offset).context("Truncated JPEG marker")?;
                offset += 1;
                ensure!(
                    !matches!(marker, 0 | 0xd0..=0xda),
                    "JPEG frame must precede scan data"
                );
                let length = u16::from_be_bytes(region(bytes, offset, 2)?.try_into()?) as usize;
                ensure!(length >= 2, "Invalid JPEG segment length");
                let payload = region(bytes, offset + 2, length - 2)?;
                if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                    ensure!(
                        matches!(marker, 0xc0..=0xc2) && payload.len() >= 6,
                        "Unsupported JPEG frame header"
                    );
                    return Ok((
                        u32::from(u16::from_be_bytes(payload[3..5].try_into()?)),
                        u32::from(u16::from_be_bytes(payload[1..3].try_into()?)),
                    ));
                }
                offset = offset
                    .checked_add(length)
                    .context("JPEG segment overflow")?;
            }
            anyhow::bail!("JPEG exceeds its header segment limit")
        }
        ImageFormat::WebP => {
            let kind: [u8; 4] = region(bytes, 12, 4)?.try_into()?;
            let length = u32::from_le_bytes(region(bytes, 16, 4)?.try_into()?) as usize;
            let payload = region(bytes, 20, length)?;
            if kind == *b"VP8X" {
                ensure!(payload.len() == 10, "Invalid WebP extended header");
                let u24 = |part: &[u8]| u32::from_le_bytes([part[0], part[1], part[2], 0]);
                Ok((u24(&payload[4..7]) + 1, u24(&payload[7..10]) + 1))
            } else {
                crate::clipboard::webp_bitstream(kind, payload, crate::MAX_CAPTURE_BYTES)
            }
        }
        ImageFormat::Bmp => {
            let width = i32::from_le_bytes(region(bytes, 18, 4)?.try_into()?);
            let height = i32::from_le_bytes(region(bytes, 22, 4)?.try_into()?);
            ensure!(
                width > 0 && height != 0 && height != i32::MIN,
                "Invalid BMP dimensions"
            );
            Ok((width as u32, height.unsigned_abs()))
        }
        ImageFormat::Tiff => {
            let little = match region(bytes, 0, 4)? {
                b"II*\0" => true,
                b"MM\0*" => false,
                _ => anyhow::bail!("Only classic TIFF is supported"),
            };
            let u16_at = |offset| -> Result<u16> {
                let value = region(bytes, offset, 2)?.try_into()?;
                Ok(if little {
                    u16::from_le_bytes(value)
                } else {
                    u16::from_be_bytes(value)
                })
            };
            let u32_at = |offset| -> Result<u32> {
                let value = region(bytes, offset, 4)?.try_into()?;
                Ok(if little {
                    u32::from_le_bytes(value)
                } else {
                    u32::from_be_bytes(value)
                })
            };
            let directory = u32_at(4)? as usize;
            let count = usize::from(u16_at(directory)?);
            ensure!(
                (1..=32).contains(&count),
                "TIFF directory must contain 1–32 entries"
            );
            region(bytes, directory, count * 12 + 6)?;
            let (mut width, mut height) = (None, None);
            for index in 0..count {
                let at = directory + 2 + index * 12;
                let tag = u16_at(at)?;
                if matches!(tag, 256 | 257) {
                    ensure!(u32_at(at + 4)? == 1, "Invalid TIFF dimension count");
                    let value = match u16_at(at + 2)? {
                        3 => u32::from(u16_at(at + 8)?),
                        4 => u32_at(at + 8)?,
                        _ => anyhow::bail!("Invalid TIFF dimension type"),
                    };
                    if tag == 256 {
                        ensure!(width.replace(value).is_none(), "Duplicate TIFF width");
                    } else {
                        ensure!(height.replace(value).is_none(), "Duplicate TIFF height");
                    }
                }
            }
            Ok((
                width.context("TIFF width missing")?,
                height.context("TIFF height missing")?,
            ))
        }
        _ => anyhow::bail!(
            "Choose a PNG, JPEG, WebP, supported truecolor BMP or uncompressed TIFF image"
        ),
    }
}

#[cfg(any(test, target_os = "windows"))]
pub(crate) fn decode(
    bytes: &[u8],
    format: ImageFormat,
    max_decoded_bytes: u64,
    expected: Option<[u32; 2]>,
    cancelled: &impl Fn() -> bool,
) -> Result<RgbaImage> {
    decode_with_backing(
        bytes,
        bytes.len(),
        format,
        max_decoded_bytes,
        expected,
        cancelled,
    )
}

pub(crate) fn decode_with_backing(
    bytes: &[u8],
    backing: usize,
    format: ImageFormat,
    max_decoded_bytes: u64,
    expected: Option<[u32; 2]>,
    cancelled: &impl Fn() -> bool,
) -> Result<RgbaImage> {
    check(cancelled)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_INPUT,
        "Encoded image must contain 1 byte to 32 MiB"
    );
    ensure!(
        max_decoded_bytes > 0 && max_decoded_bytes <= crate::MAX_CAPTURE_BYTES,
        "Invalid managed image decoded budget"
    );
    ensure!(
        backing >= bytes.len() && backing <= isize::MAX as usize,
        "Encoded owner backing has an invalid addressable extent"
    );
    let (width, height) = header_dimensions(bytes, format, cancelled)?;
    crate::validate_image_size(width, height)?;
    let output = u64::from(width) * u64::from(height) * 4;
    ensure!(
        output <= max_decoded_bytes,
        "Decoded image exceeds the memory limit"
    );
    admit_working_set(
        0,
        output,
        backing as u64,
        (MAX_CHUNKS + 1) * std::mem::size_of::<Segment>(),
    )?;
    let (input, mut metadata) = match format {
        ImageFormat::Png => png(bytes, cancelled)?,
        ImageFormat::Jpeg => jpeg(bytes, backing, max_decoded_bytes, expected, cancelled)?,
        ImageFormat::WebP => {
            let mut metadata = webp(bytes, cancelled)?;
            let orientation = crate::clipboard::validate_webp(bytes, max_decoded_bytes)?;
            metadata.orientation = orientation;
            let mut input = Input::new(bytes, cancelled);
            input.push(0, bytes.len())?;
            (input, metadata)
        }
        ImageFormat::Bmp => {
            crate::bmp_input::validate_bmp(bytes, max_decoded_bytes)?;
            let mut input = Input::new(bytes, cancelled);
            input.push(0, bytes.len())?;
            (
                input,
                Metadata {
                    profile: None,
                    orientation: Orientation::NoTransforms,
                },
            )
        }
        ImageFormat::Tiff => {
            crate::tiff_input::validate_tiff(bytes, max_decoded_bytes)?;
            let mut input = Input::new(bytes, cancelled);
            input.push(0, bytes.len())?;
            (
                input,
                Metadata {
                    profile: None,
                    orientation: Orientation::NoTransforms,
                },
            )
        }
        _ => anyhow::bail!(
            "Choose a PNG, JPEG, WebP, supported truecolor BMP or uncompressed TIFF image"
        ),
    };
    let index_bytes = input.segments.capacity() * std::mem::size_of::<Segment>();
    let encoded_bytes = backing as u64
        + if format == ImageFormat::Jpeg {
            input.length as u64 * 2
        } else {
            0
        };
    let mut reader = image::ImageReader::with_format(input, format);
    // The codec's allocation limit includes headers and intermediate buffers,
    // unlike the caller's native-pixel ceiling. Reserve bounded header precision
    // until the decoder supplies its exact native color type, then tighten below.
    let header_source = if format == ImageFormat::Png && bytes.get(24) == Some(&16) {
        output * 2
    } else {
        output
    };
    let constructor_budget = admit_working_set(header_source, output, encoded_bytes, index_bytes)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(constructor_budget);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    crate::validate_image_size(width, height)?;
    let source_bytes = decoder.total_bytes();
    let output_bytes = u64::from(width) * u64::from(height) * 4;
    ensure!(
        source_bytes <= max_decoded_bytes && output_bytes <= max_decoded_bytes,
        "Decoded image native precision or RGBA output exceeds the memory limit"
    );
    let dimensions = oriented_size(width, height, metadata.orientation);
    ensure!(
        expected.is_none_or(|size| size == [dimensions.0, dimensions.1]),
        "Oriented editor input dimensions differ from source_size"
    );
    let decoder_budget = admit_working_set(source_bytes, output_bytes, encoded_bytes, index_bytes)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(decoder_budget);
    decoder.set_limits(limits)?;
    check(cancelled)?;
    let decoded = DynamicImage::from_decoder(decoder)?;
    // Take the profile out explicitly so no source metadata is attached to output pixels.
    let profile = metadata.profile.take();
    convert(decoded, profile.as_ref(), metadata.orientation, cancelled)
}

#[cfg(test)]
#[path = "managed_color_tests.rs"]
mod tests;
