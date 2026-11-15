//! Immutable-snapshot admission for one uncompressed, chunky RGB8/RGBA8 TIFF.

use anyhow::{Context, Result, ensure};

const MAX_ENCODED_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const MAX_STRIPS: usize = 4096;
const MAX_METADATA_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

fn region(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    let end = start.checked_add(length).context("TIFF range overflow")?;
    bytes.get(start..end).context("Truncated TIFF range")
}

impl ByteOrder {
    fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        let value = region(bytes, offset, 2)?.try_into()?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(value),
            Self::Big => u16::from_be_bytes(value),
        })
    }

    fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let value = region(bytes, offset, 4)?.try_into()?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(value),
            Self::Big => u32::from_be_bytes(value),
        })
    }
}

#[derive(Clone, Copy)]
struct Tag<'a> {
    id: u16,
    kind: u16,
    count: usize,
    bytes: &'a [u8],
}

impl Tag<'_> {
    fn value(self, order: ByteOrder, index: usize) -> Result<u32> {
        ensure!(index < self.count, "Invalid TIFF value index");
        match self.kind {
            3 => Ok(u32::from(order.u16(
                self.bytes,
                index.checked_mul(2).context("TIFF value offset overflow")?,
            )?)),
            4 => order.u32(
                self.bytes,
                index.checked_mul(4).context("TIFF value offset overflow")?,
            ),
            _ => anyhow::bail!("TIFF value is not an unsigned integer"),
        }
    }
}

fn required<'a>(tags: &[Tag<'a>], id: u16) -> Result<Tag<'a>> {
    tags.iter()
        .find(|tag| tag.id == id)
        .copied()
        .context("TIFF is missing a required tag")
}

fn field_width(id: u16, kind: u16, count: usize) -> Result<usize> {
    let valid = match id {
        256 | 257 | 278 => matches!(kind, 3 | 4) && count == 1,
        258 | 339 => kind == 3 && matches!(count, 3 | 4),
        259 | 262 | 274 | 277 | 284 | 296 | 317 | 338 => kind == 3 && count == 1,
        273 | 279 => matches!(kind, 3 | 4) && (1..=MAX_STRIPS).contains(&count),
        282 | 283 => kind == 5 && count == 1,
        _ => false,
    };
    ensure!(valid, "Unsupported TIFF tag, type or count");
    Ok(match kind {
        3 => 2,
        4 => 4,
        5 => 8,
        _ => unreachable!(),
    })
}

pub(crate) fn validate_tiff(bytes: &[u8], max_decoded_bytes: u64) -> Result<()> {
    ensure!(
        max_decoded_bytes > 0 && max_decoded_bytes <= crate::MAX_CAPTURE_BYTES,
        "Invalid TIFF decoded image budget"
    );
    ensure!(
        (8..=MAX_ENCODED_BYTES).contains(&bytes.len()),
        "TIFF input must contain a complete header within 32 MiB"
    );
    let order = match region(bytes, 0, 2)? {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => anyhow::bail!("Invalid TIFF byte order"),
    };
    ensure!(order.u16(bytes, 2)? == 42, "Only classic TIFF is supported");
    let ifd = usize::try_from(order.u32(bytes, 4)?)?;
    ensure!(ifd >= 8, "Invalid TIFF directory offset");
    let count = usize::from(order.u16(bytes, ifd)?);
    ensure!(
        (1..=MAX_ENTRIES).contains(&count),
        "TIFF directory must contain 1–32 entries"
    );
    let ifd_length = count
        .checked_mul(12)
        .and_then(|length| length.checked_add(6))
        .context("TIFF directory size overflow")?;
    let directory = region(bytes, ifd, ifd_length)?;
    ensure!(
        order.u32(directory, ifd_length - 4)? == 0,
        "TIFF must contain exactly one image directory"
    );

    let mut tags = Vec::with_capacity(count);
    // At most 4096 raster ranges, 32 external values, one IFD and one header.
    let mut ranges = Vec::with_capacity(MAX_STRIPS + MAX_ENTRIES + 2);
    ranges.push((0, 8));
    ranges.push((ifd, ifd + ifd_length));
    let mut metadata_bytes = ifd_length;
    let mut previous_id = None;
    for index in 0..count {
        let offset = index
            .checked_mul(12)
            .and_then(|offset| offset.checked_add(2))
            .context("TIFF entry offset overflow")?;
        let entry = region(directory, offset, 12)?;
        let id = order.u16(entry, 0)?;
        ensure!(
            previous_id.is_none_or(|previous| previous < id),
            "TIFF tags must be unique and strictly increasing"
        );
        previous_id = Some(id);
        let kind = order.u16(entry, 2)?;
        let count = usize::try_from(order.u32(entry, 4)?)?;
        let length = count
            .checked_mul(field_width(id, kind, count)?)
            .context("TIFF tag size overflow")?;
        let data = if length <= 4 {
            region(entry, 8, length)?
        } else {
            let start = usize::try_from(order.u32(entry, 8)?)?;
            ensure!(start >= 8, "Invalid TIFF external value offset");
            let value = region(bytes, start, length)?;
            metadata_bytes = metadata_bytes
                .checked_add(length)
                .context("TIFF metadata size overflow")?;
            ensure!(
                metadata_bytes <= MAX_METADATA_BYTES,
                "TIFF metadata exceeds 256 KiB"
            );
            ranges.push((start, start + length));
            value
        };
        tags.push(Tag {
            id,
            kind,
            count,
            bytes: data,
        });
    }

    let width = required(&tags, 256)?.value(order, 0)?;
    let height = required(&tags, 257)?.value(order, 0)?;
    crate::validate_image_size(width, height)?;
    let rgba_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .context("TIFF decoded size overflow")?;
    ensure!(
        rgba_bytes <= max_decoded_bytes,
        "TIFF dimensions exceed the decoded image budget"
    );
    ensure!(
        required(&tags, 259)?.value(order, 0)? == 1,
        "TIFF compression is unsupported"
    );
    ensure!(
        required(&tags, 262)?.value(order, 0)? == 2,
        "TIFF requires RGB pixels"
    );
    let samples = required(&tags, 277)?.value(order, 0)?;
    ensure!(
        matches!(samples, 3 | 4),
        "TIFF requires RGB8 or straight RGBA8 pixels"
    );
    let bits = required(&tags, 258)?;
    ensure!(
        bits.count == samples as usize,
        "TIFF sample counts do not match"
    );
    for index in 0..bits.count {
        ensure!(
            bits.value(order, index)? == 8,
            "TIFF requires 8-bit samples"
        );
    }
    let extra = tags.iter().find(|tag| tag.id == 338);
    ensure!(
        match (samples, extra) {
            (3, None) => true,
            (4, Some(tag)) => tag.value(order, 0)? == 2,
            _ => false,
        },
        "TIFF alpha must be explicitly unassociated"
    );
    for tag in &tags {
        match tag.id {
            274 | 284 | 317 => ensure!(
                tag.value(order, 0)? == 1,
                "Unsupported TIFF orientation, planar layout or predictor"
            ),
            296 => ensure!(
                (1..=3).contains(&tag.value(order, 0)?),
                "Invalid TIFF resolution unit"
            ),
            282 | 283 => ensure!(
                order.u32(tag.bytes, 4)? != 0,
                "Invalid TIFF resolution denominator"
            ),
            339 => {
                ensure!(
                    tag.count == samples as usize,
                    "TIFF sample format counts do not match"
                );
                for index in 0..tag.count {
                    ensure!(
                        tag.value(order, index)? == 1,
                        "TIFF requires unsigned integer samples"
                    );
                }
            }
            _ => {}
        }
    }

    let rows = required(&tags, 278)?.value(order, 0)?;
    ensure!(rows != 0, "TIFF rows per strip must be nonzero");
    let strips = (height - 1) / rows + 1;
    ensure!(
        (1..=MAX_STRIPS as u32).contains(&strips),
        "TIFF requires at most 4096 strips"
    );
    let offsets = required(&tags, 273)?;
    let lengths = required(&tags, 279)?;
    ensure!(
        offsets.count == strips as usize && lengths.count == strips as usize,
        "TIFF strip counts do not match"
    );
    let mut previous_offset = None;
    for index in 0..strips {
        let start_row = index.checked_mul(rows).context("TIFF strip row overflow")?;
        let strip_rows = rows.min(
            height
                .checked_sub(start_row)
                .context("Invalid TIFF strip row")?,
        );
        let expected = u64::from(width)
            .checked_mul(u64::from(samples))
            .and_then(|row| row.checked_mul(u64::from(strip_rows)))
            .context("TIFF strip size overflow")?;
        let length = lengths.value(order, index as usize)?;
        ensure!(
            u64::from(length) == expected,
            "TIFF strip byte count does not match its rows"
        );
        let start = usize::try_from(offsets.value(order, index as usize)?)?;
        ensure!(
            start >= 8 && previous_offset.is_none_or(|previous| previous < start),
            "TIFF strip offsets must increase"
        );
        previous_offset = Some(start);
        let length = usize::try_from(length)?;
        region(bytes, start, length)?;
        ranges.push((start, start + length));
    }
    ranges.sort_unstable();
    ensure!(
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "TIFF file regions overlap"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[derive(Clone)]
    struct Entry {
        id: u16,
        kind: u16,
        values: Vec<u32>,
    }

    struct Fixture {
        order: ByteOrder,
        entries: Vec<Entry>,
        pixels: RgbaImage,
        samples: u32,
        rows: u32,
    }

    impl ByteOrder {
        fn put16(self, bytes: &mut [u8], at: usize, value: u16) {
            bytes[at..at + 2].copy_from_slice(&match self {
                Self::Little => value.to_le_bytes(),
                Self::Big => value.to_be_bytes(),
            });
        }

        fn put32(self, bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&match self {
                Self::Little => value.to_le_bytes(),
                Self::Big => value.to_be_bytes(),
            });
        }
    }

    impl Fixture {
        fn new(order: ByteOrder, width: u32, height: u32, samples: u32, rows: u32) -> Self {
            let pixels = RgbaImage::from_fn(width, height, |x, y| {
                Rgba([
                    (x * 31) as u8,
                    (y * 47) as u8,
                    139,
                    if samples == 3 {
                        255
                    } else {
                        [0, 128, 255][(x + y) as usize % 3]
                    },
                ])
            });
            let strips = height.div_ceil(rows);
            let mut entries = Vec::new();
            for (id, kind, values) in [
                (256, 4, vec![width]),
                (257, 4, vec![height]),
                (258, 3, vec![8; samples as usize]),
                (259, 3, vec![1]),
                (262, 3, vec![2]),
                (273, 4, vec![0; strips as usize]),
                (277, 3, vec![samples]),
                (278, 4, vec![rows]),
                (
                    279,
                    4,
                    (0..strips)
                        .map(|index| width * samples * rows.min(height - index * rows))
                        .collect(),
                ),
            ] {
                entries.push(Entry { id, kind, values });
            }
            if samples == 4 {
                entries.push(Entry {
                    id: 338,
                    kind: 3,
                    values: vec![2],
                });
            }
            Self {
                order,
                entries,
                pixels,
                samples,
                rows,
            }
        }

        fn entry(&mut self, id: u16) -> &mut Entry {
            self.entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .unwrap()
        }

        fn add(&mut self, id: u16, kind: u16, values: Vec<u32>) {
            self.entries.push(Entry { id, kind, values });
        }

        fn bytes(mut self) -> Vec<u8> {
            self.entries.sort_by_key(|entry| entry.id);
            let count = self.entries.len();
            let mut bytes = vec![0; 8 + 2 + count * 12 + 4];
            bytes[..2].copy_from_slice(match self.order {
                ByteOrder::Little => b"II",
                ByteOrder::Big => b"MM",
            });
            self.order.put16(&mut bytes, 2, 42);
            self.order.put32(&mut bytes, 4, 8);
            self.order.put16(&mut bytes, 8, count as u16);
            for (index, entry) in self.entries.iter().enumerate() {
                let at = 10 + index * 12;
                let elements = if entry.kind == 5 {
                    entry.values.len() / 2
                } else {
                    entry.values.len()
                };
                let width = if entry.kind == 3 { 2 } else { 4 };
                let length = entry.values.len() * width;
                self.order.put16(&mut bytes, at, entry.id);
                self.order.put16(&mut bytes, at + 2, entry.kind);
                self.order.put32(&mut bytes, at + 4, elements as u32);
                let start = if length <= 4 {
                    at + 8
                } else {
                    let start = bytes.len();
                    self.order.put32(&mut bytes, at + 8, start as u32);
                    bytes.resize(start + length, 0);
                    start
                };
                for (index, value) in entry.values.iter().enumerate() {
                    if entry.kind == 3 {
                        self.order
                            .put16(&mut bytes, start + index * width, *value as u16);
                    } else {
                        self.order.put32(&mut bytes, start + index * width, *value);
                    }
                }
            }
            let offsets = value_at(&bytes, self.order, 273);
            let strips = self.pixels.height().div_ceil(self.rows);
            let offset_width = if self
                .entries
                .iter()
                .find(|entry| entry.id == 273)
                .unwrap()
                .kind
                == 3
            {
                2
            } else {
                4
            };
            for index in 0..strips {
                let start = bytes.len();
                if offset_width == 2 {
                    self.order
                        .put16(&mut bytes, offsets + index as usize * 2, start as u16);
                } else {
                    self.order
                        .put32(&mut bytes, offsets + index as usize * 4, start as u32);
                }
                let first = index * self.rows;
                for y in first..self.pixels.height().min(first.saturating_add(self.rows)) {
                    for x in 0..self.pixels.width() {
                        bytes.extend_from_slice(
                            &self.pixels.get_pixel(x, y).0[..self.samples as usize],
                        );
                    }
                }
            }
            bytes
        }
    }

    fn entry_at(bytes: &[u8], order: ByteOrder, id: u16) -> usize {
        let ifd = order.u32(bytes, 4).unwrap() as usize;
        let count = order.u16(bytes, ifd).unwrap() as usize;
        (0..count)
            .map(|index| ifd + 2 + index * 12)
            .find(|at| order.u16(bytes, *at).unwrap() == id)
            .unwrap()
    }

    fn value_at(bytes: &[u8], order: ByteOrder, id: u16) -> usize {
        let at = entry_at(bytes, order, id);
        let width = match order.u16(bytes, at + 2).unwrap() {
            3 => 2,
            4 => 4,
            5 => 8,
            _ => panic!("fixture field type"),
        };
        if order.u32(bytes, at + 4).unwrap() as usize * width <= 4 {
            at + 8
        } else {
            order.u32(bytes, at + 8).unwrap() as usize
        }
    }

    #[test]
    fn both_byte_orders_rgb_and_straight_rgba_accept_exact_pixels_and_budgets() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            for samples in [3, 4] {
                for rows in [2, u32::MAX] {
                    let mut fixture = Fixture::new(order, 3, 5, samples, rows);
                    fixture.entry(256).kind = 3;
                    fixture.entry(257).kind = 3;
                    for (id, kind, values) in [
                        (274, 3, vec![1]),
                        (282, 5, vec![72, 1]),
                        (283, 5, vec![96, 1]),
                        (284, 3, vec![1]),
                        (296, 3, vec![2]),
                        (317, 3, vec![1]),
                        (339, 3, vec![1; samples as usize]),
                    ] {
                        fixture.add(id, kind, values);
                    }
                    let expected = fixture.pixels.clone();
                    let bytes = fixture.bytes();
                    validate_tiff(&bytes, 60).unwrap();
                    assert!(validate_tiff(&bytes, 59).is_err());
                    assert_eq!(
                        image::load_from_memory(&bytes).unwrap().into_rgba8(),
                        expected
                    );
                }
            }
        }
        let mut fixture = Fixture::new(ByteOrder::Big, 3, 5, 4, 2);
        fixture.entry(273).kind = 3;
        fixture.entry(279).kind = 3;
        validate_tiff(&fixture.bytes(), 60).unwrap();
    }

    #[test]
    fn current_tiff_export_tags_are_admitted_without_losing_straight_alpha() {
        let pixels = Fixture::new(ByteOrder::Little, 3, 5, 4, 2).pixels;
        let mut output = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut output).unwrap();
            let mut image = encoder
                .new_image::<tiff::encoder::colortype::RGBA8>(3, 5)
                .unwrap();
            image
                .encoder()
                .write_tag(tiff::tags::Tag::ExtraSamples, &[2u16][..])
                .unwrap();
            image.write_data(pixels.as_raw()).unwrap();
        }
        let bytes = output.into_inner();
        validate_tiff(&bytes, 60).unwrap();
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().into_rgba8(),
            pixels
        );
    }

    #[test]
    fn shared_loader_honors_the_caller_budget_and_preserves_png_only_loading() {
        struct TemporaryFile(std::path::PathBuf);
        impl Drop for TemporaryFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let fixture = Fixture::new(ByteOrder::Big, 3, 5, 4, 2);
        let pixels = fixture.pixels.clone();
        let bytes = fixture.bytes();
        let file = TemporaryFile(std::env::temp_dir().join(format!(
            "screenshots-tiff-input-{}.png",
            uuid::Uuid::new_v4()
        )));
        std::fs::write(&file.0, &bytes).unwrap();
        assert_eq!(
            crate::load_editor_image_with_limit(&file.0, 60).unwrap(),
            pixels
        );
        assert!(crate::load_editor_image_with_limit(&file.0, 59).is_err());
        assert!(crate::clipboard::load_export_png(&file.0).is_err());
        assert_eq!(std::fs::read(&file.0).unwrap(), bytes);
    }

    #[test]
    fn header_directory_and_all_truncations_fail_without_panicking() {
        let order = ByteOrder::Little;
        let base = Fixture::new(order, 3, 5, 4, 2).bytes();
        for length in 0..base.len() {
            assert!(
                validate_tiff(&base[..length], 60).is_err(),
                "length {length}"
            );
        }
        for (at, value) in [(2, 43), (8, 0), (8, 33), (10, 999)] {
            let mut invalid = base.clone();
            order.put16(&mut invalid, at, value);
            assert!(validate_tiff(&invalid, 60).is_err());
        }
        for offset in [0, 4, u32::MAX] {
            let mut invalid = base.clone();
            order.put32(&mut invalid, 4, offset);
            assert!(validate_tiff(&invalid, 60).is_err());
        }
        let mut invalid = base.clone();
        invalid[0] = b'Z';
        assert!(validate_tiff(&invalid, 60).is_err());
        let mut invalid = base.clone();
        let next = 10 + order.u16(&base, 8).unwrap() as usize * 12;
        for value in [8, u32::MAX] {
            order.put32(&mut invalid, next, value);
            assert!(validate_tiff(&invalid, 60).is_err());
        }
        for value in [256, 1] {
            let mut invalid = base.clone();
            order.put16(&mut invalid, 22, value);
            assert!(validate_tiff(&invalid, 60).is_err());
        }
        assert!(validate_tiff(&base, 0).is_err());
        assert!(validate_tiff(&base, crate::MAX_CAPTURE_BYTES + 1).is_err());
    }

    #[test]
    fn unsupported_tags_types_samples_and_layouts_are_rejected() {
        let order = ByteOrder::Little;
        for id in [256, 257, 258, 259, 262, 273, 277, 278, 279, 338] {
            let mut fixture = Fixture::new(order, 3, 5, 4, 2);
            fixture.entries.retain(|entry| entry.id != id);
            // The independent writer requires strip-offset storage.
            if id == 273 {
                let mut bytes = Fixture::new(order, 3, 5, 4, 2).bytes();
                let at = entry_at(&bytes, order, id);
                order.put16(&mut bytes, at, 272);
                assert!(validate_tiff(&bytes, 60).is_err());
            } else {
                assert!(validate_tiff(&fixture.bytes(), 60).is_err(), "missing {id}");
            }
        }
        for (id, value) in [
            (256, 0),
            (256, u32::MAX),
            (257, 0),
            (257, u32::MAX),
            (258, 16),
            (259, 5),
            (262, 1),
            (277, 2),
            (278, 0),
            (338, 0),
            (338, 1),
        ] {
            let mut bytes = Fixture::new(order, 3, 5, 4, 2).bytes();
            let at = value_at(&bytes, order, id);
            if matches!(id, 256 | 257 | 278) {
                order.put32(&mut bytes, at, value);
            } else {
                order.put16(&mut bytes, at, value as u16);
            }
            assert!(validate_tiff(&bytes, 60).is_err(), "{id}:{value}");
        }
        for (id, kind, values) in [
            (274, 3, vec![2]),
            (282, 5, vec![1, 0]),
            (284, 3, vec![2]),
            (296, 3, vec![0]),
            (317, 3, vec![2]),
            (339, 3, vec![2; 4]),
            (339, 3, vec![1; 3]),
            (34675, 4, vec![1]),
        ] {
            let mut fixture = Fixture::new(order, 3, 5, 4, 2);
            fixture.add(id, kind, values);
            assert!(validate_tiff(&fixture.bytes(), 60).is_err(), "tag {id}");
        }
        let mut rgb = Fixture::new(order, 3, 5, 3, 2);
        rgb.add(338, 3, vec![2]);
        assert!(validate_tiff(&rgb.bytes(), 60).is_err());
        let base = Fixture::new(order, 3, 5, 4, 2).bytes();
        for (id, kind, count) in [
            (256, 1, 1),
            (256, 3, 2),
            (258, 3, 2),
            (273, 4, u32::MAX),
            (279, 5, 3),
        ] {
            let mut invalid = base.clone();
            let at = entry_at(&invalid, order, id);
            order.put16(&mut invalid, at + 2, kind);
            order.put32(&mut invalid, at + 4, count);
            assert!(validate_tiff(&invalid, 60).is_err());
        }
    }

    #[test]
    fn strips_and_metadata_must_have_distinct_complete_increasing_extents() {
        let order = ByteOrder::Little;
        let base = Fixture::new(order, 3, 5, 4, 2).bytes();
        let offsets = value_at(&base, order, 273);
        let counts = value_at(&base, order, 279);
        let first = order.u32(&base, offsets).unwrap();
        let second = order.u32(&base, offsets + 4).unwrap();
        for value in [0, 4, 8, first, first + 1, second - 1, u32::MAX] {
            let mut invalid = base.clone();
            order.put32(&mut invalid, offsets + 4, value);
            assert!(validate_tiff(&invalid, 60).is_err(), "offset {value}");
        }
        for value in [0, 1, 23, 25, u32::MAX] {
            let mut invalid = base.clone();
            order.put32(&mut invalid, counts, value);
            assert!(validate_tiff(&invalid, 60).is_err(), "length {value}");
        }
        for id in [258, 273, 279] {
            for value in [0, 4, 8, first, u32::MAX] {
                let mut invalid = base.clone();
                let entry = entry_at(&invalid, order, id);
                order.put32(&mut invalid, entry + 8, value);
                assert!(validate_tiff(&invalid, 60).is_err());
            }
        }
        let mut invalid = base.clone();
        let count_entry = entry_at(&invalid, order, 279);
        order.put32(&mut invalid, count_entry + 8, offsets as u32);
        assert!(validate_tiff(&invalid, 60).is_err());
        let mut reordered = base.clone();
        order.put32(&mut reordered, offsets, second);
        order.put32(&mut reordered, offsets + 4, first);
        assert!(validate_tiff(&reordered, 60).is_err());

        let mut fixture = Fixture::new(order, 3, 5, 4, 2);
        fixture.add(282, 5, vec![72, 1]);
        fixture.add(283, 5, vec![72, 1]);
        let mut shared_metadata = fixture.bytes();
        let x_resolution = value_at(&shared_metadata, order, 282);
        let y_entry = entry_at(&shared_metadata, order, 283);
        order.put32(&mut shared_metadata, y_entry + 8, x_resolution as u32);
        assert!(validate_tiff(&shared_metadata, 60).is_err());

        let mut with_gap = base.clone();
        with_gap.splice(first as usize..first as usize, [0xa5; 17]);
        for index in 0..3 {
            let offset = order.u32(&base, offsets + index * 4).unwrap();
            order.put32(&mut with_gap, offsets + index * 4, offset + 17);
        }
        with_gap.extend_from_slice(&[0xa5; 17]);
        validate_tiff(&with_gap, 60).unwrap();
    }

    #[test]
    fn encoded_and_strip_bounds_are_exact_and_bookkeeping_stays_small() {
        let order = ByteOrder::Little;
        let mut bytes = Fixture::new(order, 1, 1, 3, 1).bytes();
        bytes.resize(MAX_ENCODED_BYTES, 0xa5);
        validate_tiff(&bytes, 4).unwrap();
        bytes.push(0);
        assert!(validate_tiff(&bytes, 4).is_err());
        drop(bytes);
        let bytes = Fixture::new(order, 1, MAX_STRIPS as u32, 4, 1).bytes();
        validate_tiff(&bytes, MAX_STRIPS as u64 * 4).unwrap();
        let mut invalid = bytes;
        let at = value_at(&invalid, order, 257);
        order.put32(&mut invalid, at, MAX_STRIPS as u32 + 1);
        assert!(validate_tiff(&invalid, crate::MAX_CAPTURE_BYTES).is_err());
        assert!(
            (MAX_STRIPS + MAX_ENTRIES + 2) * std::mem::size_of::<(usize, usize)>()
                + MAX_ENTRIES * std::mem::size_of::<Tag<'_>>()
                < MAX_METADATA_BYTES
        );
    }
}
