//! Explicit image structure inventory without decoding pixels or exposing metadata values.

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    ffi::OsString,
    fs::{Metadata, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    time::{Duration, Instant},
};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CHUNKS: usize = 4096;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Serialize)]
struct Chunk {
    #[serde(rename = "type")]
    kind: String,
    bytes: usize,
}

#[derive(Serialize)]
struct Inventory {
    source_file_bytes: usize,
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    metadata_present: bool,
    animated: bool,
    chunks: Vec<Chunk>,
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(args.len() == 1, "Usage: screenshots image metadata INPUT");
    let started = Instant::now();
    let path = Path::new(&args[0]);
    validate_file(&path.metadata().context("Could not inspect image input")?)?;
    let mut options = OpenOptions::new();
    options.read(true);
    // Avoid blocking if a FIFO replaces the regular file after preflight.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).context("Could not open image input")?;
    let before = file
        .metadata()
        .context("Could not inspect opened image input")?;
    validate_file(&before)?;
    let result = {
        let bytes = read_snapshot(&file, before.len(), started)?;
        let after = file.metadata().context("Could not recheck image input")?;
        ensure!(
            before.len() == after.len() && before.modified().ok() == after.modified().ok(),
            "Image input changed while reading"
        );
        if bytes.starts_with(b"\xff\xd8") {
            crate::metadata_jpeg::inventory(&bytes, started)?
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            crate::metadata_webp::inventory(&bytes, started)?
        } else {
            serde_json::to_value(inventory(&bytes, started)?)?
        }
    };
    // The encoded snapshot is released before serializing the bounded inventory.
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &result)
        .context("Could not write image metadata inventory")?;
    writeln!(output).context("Could not write image metadata inventory")?;
    output
        .flush()
        .context("Could not flush image metadata inventory")
}

fn validate_file(metadata: &Metadata) -> Result<()> {
    ensure!(metadata.is_file(), "Image input must be a regular file");
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "Image input exceeds the 32 MiB limit"
    );
    Ok(())
}

fn check_deadline(started: Instant) -> Result<()> {
    ensure!(
        started.elapsed() < DEADLINE,
        "Image inspection exceeded the 30-second deadline"
    );
    Ok(())
}

fn read_snapshot(reader: impl std::io::Read, length: u64, started: Instant) -> Result<Vec<u8>> {
    ensure!(
        length <= MAX_FILE_BYTES,
        "Image input exceeds the 32 MiB limit"
    );
    check_deadline(started)?;
    let mut bytes = vec![0; usize::try_from(length)?];
    let mut reader = reader.take(length);
    let mut position = 0;
    while position < bytes.len() {
        check_deadline(started)?;
        let end = (position + 64 * 1024).min(bytes.len());
        let read = match reader.read(&mut bytes[position..end]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.context("Could not read image input")?,
        };
        ensure!(read > 0, "Image input ended before its reported length");
        position += read;
    }
    check_deadline(started)?;
    Ok(bytes)
}

fn integer(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("validated four-byte PNG field"))
}

fn inventory(bytes: &[u8], started: Instant) -> Result<Inventory> {
    ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Input is not a PNG file"
    );
    let mut result = Inventory {
        source_file_bytes: bytes.len(),
        width: 0,
        height: 0,
        bit_depth: 0,
        color_type: 0,
        metadata_present: false,
        animated: false,
        chunks: Vec::new(),
    };
    let mut position = 8usize;
    let mut palette = false;
    let mut image_data = false;
    let mut image_data_ended = false;
    while position < bytes.len() {
        check_deadline(started)?;
        ensure!(
            result.chunks.len() < MAX_CHUNKS,
            "PNG exceeds the 4096-chunk limit"
        );
        let remaining = &bytes[position..];
        ensure!(remaining.len() >= 12, "Truncated PNG chunk");
        let length = integer(&remaining[..4]) as usize;
        ensure!(
            length <= 0x7fff_ffff && length <= remaining.len() - 12,
            "Invalid PNG chunk length"
        );
        let kind = &remaining[4..8];
        ensure!(
            kind.iter().all(u8::is_ascii_alphabetic) && kind[2].is_ascii_uppercase(),
            "Invalid PNG chunk type"
        );
        let data = &remaining[8..8 + length];
        ensure!(
            crc32fast::hash(&remaining[4..8 + length])
                == integer(&remaining[8 + length..12 + length]),
            "PNG chunk CRC mismatch"
        );
        ensure!(
            !result.chunks.is_empty() || kind == b"IHDR",
            "PNG must begin with IHDR"
        );
        if image_data && kind != b"IDAT" {
            image_data_ended = true;
        }
        match kind {
            b"IHDR" => {
                ensure!(result.chunks.is_empty() && length == 13, "Invalid PNG IHDR");
                result.width = integer(&data[..4]);
                result.height = integer(&data[4..8]);
                result.bit_depth = data[8];
                result.color_type = data[9];
                let channels = match result.color_type {
                    0 if matches!(result.bit_depth, 1 | 2 | 4 | 8 | 16) => 1,
                    2 if matches!(result.bit_depth, 8 | 16) => 3,
                    3 if matches!(result.bit_depth, 1 | 2 | 4 | 8) => 1,
                    4 if matches!(result.bit_depth, 8 | 16) => 2,
                    6 if matches!(result.bit_depth, 8 | 16) => 4,
                    _ => anyhow::bail!("Invalid PNG color type or bit depth"),
                };
                ensure!(
                    result.width > 0
                        && result.height > 0
                        && result.width <= 0x7fff_ffff
                        && result.height <= 0x7fff_ffff,
                    "Invalid PNG dimensions"
                );
                let pixels = u64::from(result.width) * u64::from(result.height);
                let bytes_per_pixel = (channels * u64::from(result.bit_depth).div_ceil(8)).max(4);
                ensure!(
                    pixels <= app_core::MAX_CAPTURE_BYTES / bytes_per_pixel,
                    "PNG dimensions exceed the 256 MiB image limit"
                );
                ensure!(
                    data[10] == 0 && data[11] == 0 && data[12] <= 1,
                    "Unsupported PNG IHDR method"
                );
            }
            b"PLTE" => {
                ensure!(
                    !palette
                        && !image_data
                        && length > 0
                        && length <= 768
                        && length.is_multiple_of(3),
                    "Invalid PNG palette"
                );
                ensure!(
                    !matches!(result.color_type, 0 | 4),
                    "PNG grayscale image cannot have a palette"
                );
                if result.color_type == 3 {
                    ensure!(
                        length / 3 <= 1usize << result.bit_depth,
                        "PNG palette exceeds the bit depth"
                    );
                }
                palette = true;
            }
            b"IDAT" => {
                ensure!(
                    !image_data_ended,
                    "PNG image data chunks must be consecutive"
                );
                ensure!(
                    result.color_type != 3 || palette,
                    "Indexed PNG requires a palette before image data"
                );
                image_data = true;
            }
            b"IEND" => {
                ensure!(
                    length == 0 && image_data,
                    "Invalid PNG IEND or missing image data"
                );
                ensure!(
                    position + 12 == bytes.len(),
                    "PNG has trailing bytes after IEND"
                );
            }
            b"acTL" => {
                ensure!(
                    !result.animated && !image_data && length == 8,
                    "Invalid PNG animation control chunk"
                );
                ensure!(
                    integer(&data[..4]) > 0,
                    "PNG animation must declare at least one frame"
                );
                result.animated = true;
            }
            b"fcTL" => ensure!(
                result.animated && length == 26,
                "Invalid PNG frame control chunk"
            ),
            b"fdAT" => ensure!(
                result.animated && image_data && length >= 4,
                "Invalid PNG frame data chunk"
            ),
            _ => ensure!(kind[0].is_ascii_lowercase(), "Unknown critical PNG chunk"),
        }
        result.metadata_present |= kind[0].is_ascii_lowercase();
        result.chunks.push(Chunk {
            kind: std::str::from_utf8(kind)?.to_owned(),
            bytes: length,
        });
        position += length + 12;
        if kind == b"IEND" {
            return Ok(result);
        }
    }
    anyhow::bail!("PNG is missing IEND")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_never_reads_past_the_admitted_extent_and_rejects_truncation() {
        assert_eq!(
            read_snapshot(&b"abcignored"[..], 3, Instant::now()).unwrap(),
            b"abc"
        );
        assert!(read_snapshot(&b"ab"[..], 3, Instant::now()).is_err());
    }

    #[test]
    fn expired_or_oversized_snapshot_fails_before_reading() {
        struct Unreadable;
        impl std::io::Read for Unreadable {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                panic!("Rejected input must not be read");
            }
        }
        assert!(read_snapshot(Unreadable, 1, Instant::now() - DEADLINE).is_err());
        assert!(read_snapshot(Unreadable, MAX_FILE_BYTES + 1, Instant::now()).is_err());
    }
}
