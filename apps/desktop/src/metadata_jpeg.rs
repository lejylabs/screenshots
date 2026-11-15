//! Bounded JPEG marker inventory; no entropy decoding or metadata payload output.

use anyhow::{Result, bail, ensure};
use std::time::{Duration, Instant};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_MARKERS: usize = 4096;

fn deadline(started: Instant) -> Result<()> {
    ensure!(
        started.elapsed() < Duration::from_secs(30),
        "JPEG inspection exceeded the 30-second deadline"
    );
    Ok(())
}

fn marker_name(marker: u8) -> &'static str {
    match marker {
        0xc0 => "SOF0",
        0xc1 => "SOF1",
        0xc2 => "SOF2",
        0xc4 => "DHT",
        0xd8 => "SOI",
        0xd9 => "EOI",
        0xda => "SOS",
        0xdb => "DQT",
        0xdd => "DRI",
        0xd0..=0xd7 => "RST",
        0xe0 => "APP0",
        0xe1 => "APP1",
        0xe2 => "APP2",
        0xe3 => "APP3",
        0xe4 => "APP4",
        0xe5 => "APP5",
        0xe6 => "APP6",
        0xe7 => "APP7",
        0xe8 => "APP8",
        0xe9 => "APP9",
        0xea => "APP10",
        0xeb => "APP11",
        0xec => "APP12",
        0xed => "APP13",
        0xee => "APP14",
        0xef => "APP15",
        0xfe => "COM",
        _ => "UNKNOWN",
    }
}

fn add(segments: &mut Vec<serde_json::Value>, marker: u8, length: usize) -> Result<()> {
    ensure!(
        segments.len() < MAX_MARKERS,
        "JPEG exceeds the 4096-marker limit"
    );
    segments.push(serde_json::json!({"type":marker_name(marker),"bytes":length}));
    Ok(())
}

pub(crate) fn inventory(bytes: &[u8], started: Instant) -> Result<serde_json::Value> {
    ensure!(
        bytes.len() <= MAX_BYTES,
        "JPEG input exceeds the 32 MiB limit"
    );
    ensure!(bytes.starts_with(&[0xff, 0xd8]), "Input is not a JPEG file");
    deadline(started)?;
    let mut segments = Vec::new();
    add(&mut segments, 0xd8, 0)?;
    let mut offset = 2;
    let mut frame = None;
    let mut ids = Vec::new();
    let mut scan = false;
    let mut entropy = false;
    let mut metadata = false;
    while offset < bytes.len() {
        deadline(started)?;
        if entropy {
            while offset < bytes.len() && bytes[offset] != 0xff {
                offset += 1;
                if offset % 65536 == 0 {
                    deadline(started)?;
                }
            }
            ensure!(offset < bytes.len(), "JPEG scan is missing an end marker");
        }
        ensure!(bytes[offset] == 0xff, "Invalid JPEG marker prefix");
        offset += 1;
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
            if offset % 65536 == 0 {
                deadline(started)?;
            }
        }
        let marker = *bytes
            .get(offset)
            .ok_or_else(|| anyhow::anyhow!("Truncated JPEG marker"))?;
        offset += 1;
        if marker == 0x00 {
            ensure!(entropy, "Stuffed JPEG byte outside scan data");
            continue;
        }
        if (0xd0..=0xd7).contains(&marker) {
            ensure!(entropy, "JPEG restart marker outside scan data");
            add(&mut segments, marker, 0)?;
            continue;
        }
        entropy = false;
        if marker == 0xd9 {
            ensure!(
                frame.is_some() && scan,
                "JPEG requires a frame and scan before EOI"
            );
            ensure!(offset == bytes.len(), "JPEG has trailing bytes after EOI");
            add(&mut segments, marker, 0)?;
            let (width, height, components, progressive) = frame.unwrap();
            return Ok(
                serde_json::json!({"format":"jpeg","source_file_bytes":bytes.len(),"width":width,"height":height,"bit_depth":8,"components":components,"metadata_present":metadata,"progressive":progressive,"segments":segments}),
            );
        }
        ensure!(
            marker != 0xd8 && marker != 0x01,
            "Unexpected standalone JPEG marker"
        );
        ensure!(offset + 2 <= bytes.len(), "Truncated JPEG segment length");
        let length = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        offset += 2;
        ensure!(
            length >= 2 && length - 2 <= bytes.len() - offset,
            "Invalid JPEG segment length"
        );
        let data = &bytes[offset..offset + length - 2];
        offset += length - 2;
        add(&mut segments, marker, data.len())?;
        if (0xe0..=0xef).contains(&marker) || marker == 0xfe {
            metadata = true;
        }
        match marker {
            0xc0..=0xcf if marker != 0xc4 => {
                ensure!(matches!(marker, 0xc0..=0xc2), "Unsupported JPEG frame type");
                ensure!(
                    frame.is_none() && data.len() >= 6,
                    "Invalid or repeated JPEG frame"
                );
                ensure!(data[0] == 8, "JPEG inventory supports only 8-bit samples");
                let height = u32::from(u16::from_be_bytes([data[1], data[2]]));
                let width = u32::from(u16::from_be_bytes([data[3], data[4]]));
                let components = data[5];
                ensure!(
                    matches!(components, 1 | 3 | 4) && data.len() == 6 + 3 * components as usize,
                    "Invalid JPEG frame components"
                );
                app_core::validate_image_size(width, height)?;
                for component in data[6..].chunks_exact(3) {
                    ensure!(
                        !ids.contains(&component[0])
                            && (1..=4).contains(&(component[1] >> 4))
                            && (1..=4).contains(&(component[1] & 15))
                            && component[2] <= 3,
                        "Invalid JPEG component description"
                    );
                    ids.push(component[0]);
                }
                frame = Some((width, height, components, marker == 0xc2));
            }
            0xda => {
                let (_, _, components, progressive) =
                    frame.ok_or_else(|| anyhow::anyhow!("JPEG scan precedes frame"))?;
                ensure!(!data.is_empty(), "Invalid JPEG scan");
                let count = data[0] as usize;
                ensure!(
                    count > 0 && count <= components as usize && data.len() == 1 + 2 * count + 3,
                    "Invalid JPEG scan components"
                );
                let mut scanned = Vec::new();
                for component in data[1..1 + 2 * count].chunks_exact(2) {
                    ensure!(
                        ids.contains(&component[0])
                            && !scanned.contains(&component[0])
                            && component[1] >> 4 <= 3
                            && component[1] & 15 <= 3,
                        "Invalid JPEG scan component"
                    );
                    scanned.push(component[0]);
                }
                let tail = &data[1 + 2 * count..];
                if progressive {
                    ensure!(
                        tail[0] <= tail[1]
                            && tail[1] <= 63
                            && (tail[0] != 0 || tail[1] == 0)
                            && (tail[0] == 0 || count == 1)
                            && tail[2] >> 4 <= 13
                            && tail[2] & 15 <= 13,
                        "Invalid progressive JPEG scan parameters"
                    );
                } else {
                    ensure!(
                        tail == [0, 63, 0],
                        "Invalid sequential JPEG scan parameters"
                    );
                }
                scan = true;
                entropy = true;
            }
            0xdd => ensure!(data.len() == 2, "Invalid JPEG restart interval"),
            0xdc => bail!("JPEG deferred image height is unsupported"),
            _ => {}
        }
    }
    bail!("JPEG is missing EOI")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        let image = image::RgbImage::from_pixel(3, 2, image::Rgb([10, 20, 30]));
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .encode_image(&image)
            .unwrap();
        bytes
    }
    fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
        let mut result = vec![0xff, marker];
        result.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        result.extend_from_slice(data);
        result
    }
    #[test]
    fn real_encoded_jpeg_reports_structure_without_metadata_values() {
        let mut bytes = jpeg();
        bytes.splice(2..2, segment(0xe1, b"Exif\0\0PRIVATE SECRET GPS"));
        let result = inventory(&bytes, Instant::now()).unwrap();
        assert_eq!(result["format"], "jpeg");
        assert_eq!(result["width"], 3);
        assert_eq!(result["height"], 2);
        assert_eq!(result["components"], 3);
        assert_eq!(result["progressive"], false);
        assert_eq!(result["metadata_present"], true);
        assert!(!result.to_string().contains("PRIVATE"));
        assert!(!result.to_string().contains("GPS"));
    }
    #[test]
    fn progressive_multiscan_and_entropy_escaping_are_traversed() {
        let mut bytes = vec![0xff, 0xd8];
        bytes.extend(segment(0xc2, &[8, 0, 2, 0, 3, 1, 1, 0x11, 0]));
        bytes.extend(segment(0xda, &[1, 1, 0, 0, 0, 0]));
        bytes.extend([12, 0xff, 0, 44, 0xff, 0xd0]);
        bytes.extend(segment(0xda, &[1, 1, 0, 1, 63, 0]));
        bytes.extend([55, 0xff, 0xff, 0xd9]);
        let result = inventory(&bytes, Instant::now()).unwrap();
        assert_eq!(result["progressive"], true);
        assert_eq!(
            result["segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["type"] == "SOS")
                .count(),
            2
        );
    }

    #[test]
    fn actual_progressive_jpeg_multiple_scans_are_inventoried() {
        // Pillow/libjpeg synthetic 2x1 fixture; no user pixels or external metadata.
        let hex = "ffd8ffe000104a46494600010100000100010000ffdb004300080606070605080707070909080a0c140d0c0b0b0c1912130f141d1a1f1e1d1a1c1c20242e2720222c231c1c2837292c30313434341f27393d38323c2e333432ffdb0043010909090c0b0c180d0d1832211c213232323232323232323232323232323232323232323232323232323232323232323232323232323232323232323232323232ffc20011080001000203012200021101031101ffc4001500010100000000000000000000000000000006ffc4001501010100000000000000000000000000000304ffda000c030100021003100000018e1417ffc40014100100000000000000000000000000000000ffda00080101000105027fffc40014110100000000000000000000000000000000ffda0008010301013f017fffc40014110100000000000000000000000000000000ffda0008010201013f017fffc40014100100000000000000000000000000000000ffda0008010100063f027fffc40014100100000000000000000000000000000000ffda0008010100013f217fffda000c0301000200030000001007ffc40014110100000000000000000000000000000000ffda0008010301013f107fffc40014110100000000000000000000000000000000ffda0008010201013f107fffc40014100100000000000000000000000000000000ffda0008010100013f107fffd9";
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let result = inventory(&bytes, Instant::now()).unwrap();
        assert_eq!(result["width"], 2);
        assert_eq!(result["height"], 1);
        assert_eq!(result["progressive"], true);
        assert_eq!(
            result["segments"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["type"] == "SOS")
                .count(),
            10
        );
    }
    #[test]
    fn malformed_truncated_overlimit_and_trailing_streams_fail() {
        for mut bytes in [
            vec![],
            vec![0xff, 0xd8, 0xff, 0xd9],
            vec![0xff, 0xd8, 0xff, 0xe1, 0, 1],
            vec![0xff, 0xd8, 0xff, 0xe1, 0, 9, 1],
            vec![0xff, 0xd8, 0xff, 0],
            jpeg(),
        ] {
            if bytes == jpeg() {
                bytes.push(0);
            }
            assert!(inventory(&bytes, Instant::now()).is_err());
        }
        let bytes = jpeg();
        for end in 0..bytes.len() {
            assert!(inventory(&bytes[..end], Instant::now()).is_err());
        }
        let mut many = vec![0xff, 0xd8];
        for _ in 0..4096 {
            many.extend(segment(0xfe, &[]));
        }
        assert!(inventory(&many, Instant::now()).is_err());
        assert!(inventory(&vec![0; MAX_BYTES + 1], Instant::now()).is_err());
        assert!(inventory(&jpeg(), Instant::now() - Duration::from_secs(30)).is_err());
    }
}
