//! Bounded WebP container inventory; compressed pixels and metadata stay opaque.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const MAX_CHUNKS: usize = 4096;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);

fn check(started: Instant) -> Result<()> {
    ensure!(
        started.elapsed() < DEADLINE,
        "WebP inspection exceeded the 30-second deadline"
    );
    Ok(())
}

fn u24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn dimensions(width: u32, height: u32) -> Result<(u32, u32)> {
    app_core::validate_image_size(width, height)?;
    Ok((width, height))
}

struct Chunks {
    count: usize,
    started: Instant,
}

impl Chunks {
    fn next<'a>(&mut self, bytes: &'a [u8], offset: &mut usize) -> Result<([u8; 4], &'a [u8])> {
        check(self.started)?;
        ensure!(self.count < MAX_CHUNKS, "WebP exceeds the 4096-chunk limit");
        self.count += 1;
        let header = bytes
            .get(*offset..)
            .and_then(|rest| rest.get(..8))
            .context("Truncated WebP chunk header")?;
        let kind: [u8; 4] = header[..4].try_into()?;
        ensure!(
            kind.iter().all(|byte| (0x20..=0x7e).contains(byte)),
            "WebP chunk type is not printable ASCII"
        );
        let length = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        let start = offset
            .checked_add(8)
            .context("WebP chunk offset overflow")?;
        let end = start
            .checked_add(length)
            .context("WebP chunk size overflow")?;
        let padded = end
            .checked_add(length & 1)
            .context("WebP chunk padding overflow")?;
        ensure!(
            padded <= bytes.len(),
            "WebP chunk exceeds its encoded extent"
        );
        ensure!(
            length & 1 == 0 || bytes[end] == 0,
            "WebP chunk has invalid padding"
        );
        *offset = padded;
        Ok((kind, &bytes[start..end]))
    }
}

fn bitstream(kind: [u8; 4], bytes: &[u8]) -> Result<(u32, u32)> {
    match &kind {
        b"VP8 " => {
            ensure!(bytes.len() >= 10, "Truncated WebP VP8 header");
            ensure!(
                bytes[0] & 1 == 0 && bytes[3..6] == [0x9d, 0x01, 0x2a],
                "Invalid WebP VP8 keyframe header"
            );
            dimensions(
                u32::from(u16::from_le_bytes(bytes[6..8].try_into()?) & 0x3fff),
                u32::from(u16::from_le_bytes(bytes[8..10].try_into()?) & 0x3fff),
            )
        }
        b"VP8L" => {
            ensure!(
                bytes.len() >= 5 && bytes[0] == 0x2f,
                "Invalid WebP VP8L header"
            );
            let bits = u32::from_le_bytes(bytes[1..5].try_into()?);
            ensure!(bits >> 29 == 0, "Unsupported WebP VP8L version");
            dimensions((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        _ => anyhow::bail!("WebP is missing an image bitstream"),
    }
}

fn alpha(bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty(), "Truncated WebP alpha header");
    ensure!(
        bytes[0] & 0xc0 == 0 && bytes[0] & 3 <= 1 && (bytes[0] >> 4) & 3 <= 1,
        "Invalid WebP alpha header"
    );
    Ok(())
}

fn frame(bytes: &[u8], canvas: (u32, u32), chunks: &mut Chunks) -> Result<()> {
    ensure!(bytes.len() >= 16, "Truncated WebP animation frame header");
    ensure!(bytes[15] & 0xfc == 0, "Invalid WebP animation frame flags");
    let x = u24(&bytes[..3]) * 2;
    let y = u24(&bytes[3..6]) * 2;
    let size = dimensions(u24(&bytes[6..9]) + 1, u24(&bytes[9..12]) + 1)?;
    ensure!(
        x.checked_add(size.0).is_some_and(|end| end <= canvas.0)
            && y.checked_add(size.1).is_some_and(|end| end <= canvas.1),
        "WebP animation frame is outside its canvas"
    );
    let mut offset = 16;
    let mut image = false;
    let mut alph = false;
    while offset < bytes.len() {
        let (kind, payload) = chunks.next(bytes, &mut offset)?;
        match &kind {
            b"ALPH" => {
                ensure!(
                    !alph && !image,
                    "WebP animation alpha chunk is out of order"
                );
                alpha(payload)?;
                alph = true;
            }
            b"VP8 " | b"VP8L" => {
                ensure!(
                    !image && (!alph || kind == *b"VP8 "),
                    "Ambiguous WebP animation bitstream"
                );
                ensure!(
                    bitstream(kind, payload)? == size,
                    "WebP animation bitstream dimensions do not match its frame"
                );
                image = true;
            }
            b"VP8X" | b"ANIM" | b"ANMF" | b"ICCP" | b"EXIF" | b"XMP " => {
                anyhow::bail!("Invalid known chunk inside WebP animation frame")
            }
            _ => {}
        }
    }
    ensure!(image, "WebP animation frame has no image bitstream");
    Ok(())
}

pub(crate) fn inventory(bytes: &[u8], started: Instant) -> Result<Value> {
    check(started)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "WebP input exceeds the 32 MiB limit"
    );
    ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "Invalid WebP RIFF header"
    );
    let declared = u64::from(u32::from_le_bytes(bytes[4..8].try_into()?)) + 8;
    ensure!(
        declared == bytes.len() as u64,
        "WebP RIFF extent does not match its input"
    );
    let mut reader = Chunks { count: 0, started };
    let mut offset = 12;
    let (first, data) = reader.next(bytes, &mut offset)?;
    let extended = first == *b"VP8X";
    let (canvas, animated, flags) = if extended {
        ensure!(data.len() == 10, "Invalid WebP VP8X header size");
        ensure!(
            data[0] & 0xc1 == 0 && data[1..4] == [0, 0, 0],
            "Invalid WebP VP8X reserved flags"
        );
        (
            dimensions(u24(&data[4..7]) + 1, u24(&data[7..10]) + 1)?,
            data[0] & 2 != 0,
            data[0],
        )
    } else {
        (bitstream(first, data)?, false, 0)
    };
    let mut summaries = vec![json!({"type": std::str::from_utf8(&first)?, "bytes": data.len()})];
    let mut image = !extended;
    let mut alph = false;
    let mut animation_header = false;
    let mut frames = 0;
    let mut metadata = false;
    while offset < bytes.len() {
        let (kind, payload) = reader.next(bytes, &mut offset)?;
        match &kind {
            b"VP8X" => anyhow::bail!("Duplicate WebP extended header"),
            b"VP8 " | b"VP8L" => {
                ensure!(
                    !animated && !image && (!alph || kind == *b"VP8 "),
                    "Ambiguous WebP image bitstreams"
                );
                ensure!(
                    bitstream(kind, payload)? == canvas,
                    "WebP image dimensions do not match its canvas"
                );
                image = true;
            }
            b"ALPH" => {
                ensure!(
                    extended && !animated && !image && !alph && flags & 0x10 != 0,
                    "WebP alpha chunk is out of order"
                );
                alpha(payload)?;
                alph = true;
            }
            b"ANIM" if animated => {
                ensure!(
                    !animation_header && frames == 0 && payload.len() == 6,
                    "Invalid WebP animation control chunk"
                );
                animation_header = true;
            }
            // The format specifies that ANIM is ignored when the animation flag is absent.
            b"ANIM" => {}
            b"ANMF" => {
                ensure!(
                    animated && animation_header,
                    "WebP animation frame precedes its control chunk"
                );
                frame(payload, canvas, &mut reader)?;
                frames += 1;
            }
            b"ICCP" => {
                ensure!(
                    extended && !image && frames == 0 && !animation_header,
                    "WebP color profile is out of order"
                );
                metadata = true;
            }
            b"EXIF" | b"XMP " => {
                ensure!(extended, "WebP metadata requires an extended header");
                metadata = true;
            }
            _ => {}
        }
        summaries.push(json!({"type": std::str::from_utf8(&kind)?, "bytes": payload.len()}));
    }
    ensure!(
        (animated && frames > 0) || (!animated && image),
        "WebP has no image bitstream"
    );
    check(started)?;
    Ok(json!({
        "format":"webp", "source_file_bytes":bytes.len(), "width":canvas.0, "height":canvas.1,
        "metadata_present":metadata, "animated":animated, "chunks":summaries
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn chunk(kind: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
        let mut out = kind.to_vec();
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
        if bytes.len() % 2 == 1 {
            out.push(0);
        }
        out
    }
    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
        for chunk in chunks {
            out.extend_from_slice(chunk);
        }
        let len = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&len.to_le_bytes());
        out
    }
    fn vp8l(width: u32, height: u32) -> Vec<u8> {
        let mut data = vec![0x2f];
        data.extend_from_slice(&((width - 1) | ((height - 1) << 14)).to_le_bytes());
        chunk(b"VP8L", &data)
    }
    fn extended(flags: u8, width: u32, height: u32) -> Vec<u8> {
        let mut data = vec![flags, 0, 0, 0];
        data.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        data.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        chunk(b"VP8X", &data)
    }

    #[test]
    fn encoder_output_and_opaque_metadata_are_inventoried() {
        let mut encoded = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut encoded)
            .write_image(&[17; 24], 3, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        let value = inventory(&encoded, Instant::now()).unwrap();
        assert_eq!(value["width"], 3);
        assert_eq!(value["height"], 2);
        assert_eq!(value["metadata_present"], false);
        assert_eq!(value["animated"], false);
        let secret = b"PRIVATE-GPS-AND-SECRET";
        let bytes = riff(&[
            extended(0x2c, 3, 2),
            chunk(b"ICCP", secret),
            vp8l(3, 2),
            chunk(b"EXIF", secret),
            chunk(b"XMP ", secret),
            chunk(b"TEST", b"opaque"),
        ]);
        let value = inventory(&bytes, Instant::now()).unwrap();
        assert_eq!(value["metadata_present"], true);
        assert_eq!(value["chunks"].as_array().unwrap().len(), 6);
        assert!(!value.to_string().contains("PRIVATE"));
    }

    #[test]
    fn riff_padding_and_chunk_limits_fail_without_exposing_payloads() {
        let valid = riff(&[vp8l(3, 2)]);
        let mut wrong_size = valid.clone();
        wrong_size[4] = 0;
        let mut padding = valid.clone();
        *padding.last_mut().unwrap() = 1;
        let invalid_type = riff(&[vp8l(3, 2), chunk(b"\xffABC", b"PRIVATE")]);
        for invalid in [
            &wrong_size[..],
            &valid[..valid.len() - 1],
            &padding[..],
            &invalid_type[..],
            b"RIFF\0\0\0\0WEBP",
        ] {
            let error = inventory(invalid, Instant::now()).unwrap_err();
            assert!(!error.to_string().contains("PRIVATE"));
        }
        let mut chunks = vec![vp8l(1, 1)];
        chunks.extend((0..4095).map(|_| chunk(b"TEST", &[])));
        assert!(inventory(&riff(&chunks), Instant::now()).is_ok());
        chunks.push(chunk(b"TEST", &[]));
        assert!(inventory(&riff(&chunks), Instant::now()).is_err());
        let mut truncated = riff(&[vp8l(1, 1)]);
        truncated.extend_from_slice(b"TEST");
        let size = (truncated.len() - 8) as u32;
        truncated[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(inventory(&truncated, Instant::now()).is_err());
        let mut oversized = valid.clone();
        oversized[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(inventory(&oversized, Instant::now()).is_err());
        assert!(inventory(&valid, Instant::now() - DEADLINE).is_err());
    }

    #[test]
    fn bitstream_and_extended_headers_have_bounded_dimensions() {
        let mut vp8 = vec![0, 0, 0, 0x9d, 1, 0x2a];
        vp8.extend_from_slice(&3u16.to_le_bytes());
        vp8.extend_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            inventory(&riff(&[chunk(b"VP8 ", &vp8)]), Instant::now()).unwrap()["width"],
            3
        );
        for chunks in [
            vec![extended(0xc0, 1, 1), vp8l(1, 1)],
            vec![chunk(b"VP8X", &[0; 9]), vp8l(1, 1)],
            vec![extended(0, 3, 2), vp8l(2, 2)],
            vec![extended(0, 16384, 16384), vp8l(1, 1)],
            vec![chunk(b"VP8L", &[0x2f, 0, 0, 0, 0xe0])],
            vec![chunk(b"VP8 ", &[0; 10])],
        ] {
            assert!(inventory(&riff(&chunks), Instant::now()).is_err());
        }
    }

    #[test]
    fn animation_frames_validate_container_order_flags_and_bounds() {
        let mut data = vec![0; 16];
        data.extend_from_slice(&vp8l(1, 1));
        let valid = riff(&[
            extended(2, 2, 2),
            chunk(b"ANIM", &[0; 6]),
            chunk(b"ANMF", &data),
        ]);
        let value = inventory(&valid, Instant::now()).unwrap();
        assert_eq!(value["animated"], true);
        assert_eq!(value["chunks"].as_array().unwrap().len(), 3);
        assert!(
            inventory(
                &riff(&[extended(2, 2, 2), chunk(b"ANMF", &data)]),
                Instant::now()
            )
            .is_err()
        );
        let mut nested = vec![0; 16];
        nested.extend_from_slice(&vp8l(1, 1));
        for _ in 0..4092 {
            nested.extend_from_slice(&chunk(b"TEST", &[]));
        }
        let mut chunks = vec![
            extended(2, 2, 2),
            chunk(b"ANIM", &[0; 6]),
            chunk(b"ANMF", &nested),
        ];
        assert!(inventory(&riff(&chunks), Instant::now()).is_ok());
        chunks.push(chunk(b"TEST", &[]));
        assert!(inventory(&riff(&chunks), Instant::now()).is_err());
        for (index, value) in [(0, 2), (15, 4)] {
            let mut bad = data.clone();
            bad[index] = value;
            assert!(
                inventory(
                    &riff(&[
                        extended(2, 2, 2),
                        chunk(b"ANIM", &[0; 6]),
                        chunk(b"ANMF", &bad)
                    ]),
                    Instant::now()
                )
                .is_err()
            );
        }
        assert!(
            inventory(
                &riff(&[extended(2, 2, 2), chunk(b"ANIM", &[0; 6])]),
                Instant::now()
            )
            .is_err()
        );
    }
}
