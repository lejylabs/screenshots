//! Exact vertical assembly of explicitly supplied images, not native scrolling.

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use serde_json::json;
use std::{
    collections::hash_map::DefaultHasher,
    ffi::OsString,
    hash::{Hash as _, Hasher as _},
    io::Write as _,
    path::Path,
};

const MAX_MATCH_BYTES: usize = 256 * 1024 * 1024;
const MAX_HEIGHT: u32 = 16_384;

fn row_hashes(image: &RgbaImage) -> Vec<u64> {
    image
        .as_raw()
        .chunks_exact(image.width() as usize * 4)
        .map(|row| {
            let mut hash = DefaultHasher::new();
            row.hash(&mut hash);
            hash.finish()
        })
        .collect()
}

fn compare_bytes(left: &[u8], right: &[u8], remaining: &mut usize) -> Result<bool> {
    ensure!(left.len() == right.len(), "Invalid overlap comparison");
    *remaining = remaining
        .checked_sub(left.len())
        .context("Exact overlap matching exceeds the 256 MiB comparison budget")?;
    Ok(left == right)
}

fn overlap(
    previous: &RgbaImage,
    next: &RgbaImage,
    previous_hashes: &[u64],
    next_hashes: &[u64],
    minimum: usize,
    remaining: &mut usize,
) -> Result<usize> {
    if previous.dimensions() == next.dimensions()
        && compare_bytes(previous.as_raw(), next.as_raw(), remaining)?
    {
        bail!("Adjacent images are identical and add no new rows");
    }
    let mut prefix = vec![0; next_hashes.len()];
    for index in 1..next_hashes.len() {
        let mut matched = prefix[index - 1];
        while matched > 0 && next_hashes[index] != next_hashes[matched] {
            matched = prefix[matched - 1];
        }
        if next_hashes[index] == next_hashes[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    let mut matched = 0;
    for row in previous_hashes {
        while matched > 0 && (matched == next_hashes.len() || *row != next_hashes[matched]) {
            matched = prefix[matched - 1];
        }
        if matched < next_hashes.len() && *row == next_hashes[matched] {
            matched += 1;
        }
    }
    let max_overlap = previous_hashes.len().min(next_hashes.len()) - 1;
    let stride = previous.width() as usize * 4;
    let mut found = None;
    while matched >= minimum {
        if matched <= max_overlap {
            let bytes = matched * stride;
            if compare_bytes(
                &previous.as_raw()[previous.as_raw().len() - bytes..],
                &next.as_raw()[..bytes],
                remaining,
            )? {
                ensure!(
                    found.is_none(),
                    "Adjacent images have ambiguous exact overlaps"
                );
                found = Some(matched);
            }
        }
        matched = prefix[matched - 1];
    }
    found.context("Adjacent images have no exact overlap at the requested minimum")
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        (4..=18).contains(&args.len()),
        "Usage: screenshots stitch-scroll OUTPUT MIN_OVERLAP INPUT INPUT [INPUT...] (2–16 inputs)"
    );
    let destination = Path::new(&args[0]);
    app_core::ImageExportFormat::from_path(destination)?;
    ensure!(
        std::fs::symlink_metadata(destination)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Output already exists or is unavailable"
    );
    let minimum: usize = args[1]
        .to_str()
        .context("Minimum overlap must be UTF-8")?
        .parse()
        .context("Minimum overlap must be a whole number")?;
    ensure!(
        (1..=4096).contains(&minimum),
        "Minimum overlap must be 1–4096 rows"
    );
    let mut sources = Vec::with_capacity(args.len() - 2);
    let mut hashes = Vec::with_capacity(args.len() - 2);
    let mut remaining_source = app_core::MAX_CAPTURE_BYTES;
    for input in &args[2..] {
        let image = app_core::load_editor_image_with_limit(Path::new(input), remaining_source)?;
        ensure!(
            (2..=MAX_HEIGHT).contains(&image.height()),
            "Stitch image height must be 2–16384 rows"
        );
        ensure!(
            minimum < image.height() as usize,
            "Minimum overlap must be smaller than each image height"
        );
        if let Some(first) = sources.first() {
            let first: &RgbaImage = first;
            ensure!(
                image.width() == first.width(),
                "Stitch images must have identical widths"
            );
        }
        remaining_source -= u64::try_from(image.as_raw().len())?;
        hashes.push(row_hashes(&image));
        sources.push(image);
    }
    let mut remaining_match = MAX_MATCH_BYTES;
    let mut overlaps = Vec::with_capacity(sources.len() - 1);
    let width = sources[0].width();
    let mut height = sources[0].height();
    for index in 1..sources.len() {
        let rows = overlap(
            &sources[index - 1],
            &sources[index],
            &hashes[index - 1],
            &hashes[index],
            minimum,
            &mut remaining_match,
        )
        .with_context(|| format!("Could not match image {} to image {}", index, index + 1))?;
        height = height
            .checked_add(sources[index].height() - rows as u32)
            .context("Stitched height overflow")?;
        app_core::validate_image_size(width, height)?;
        overlaps.push(rows);
    }
    drop(hashes);
    let count = sources.len();
    let mut output = RgbaImage::new(width, height);
    let stride = width as usize * 4;
    let mut written = 0;
    for (index, source) in sources.into_iter().enumerate() {
        let skip = if index == 0 {
            0
        } else {
            overlaps[index - 1] * stride
        };
        let pixels = &source.as_raw()[skip..];
        let bytes: &mut [u8] = output.as_mut();
        bytes[written..written + pixels.len()].copy_from_slice(pixels);
        written += pixels.len();
    }
    let format = crate::cli::frame(output).save_image_as(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
    )?;
    let result = json!({"format":format.extension(),"width":width,"height":height,"inputs":count,"overlaps":overlaps});
    let mut stdout = std::io::stdout().lock();
    (|| {
        writeln!(stdout, "{result}")?;
        stdout.flush()
    })()
    .context("Stitched image was saved; could not write JSON result")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_collision_candidates_require_exact_bytes_and_budget() {
        let first = RgbaImage::from_pixel(1, 3, image::Rgba([1, 2, 3, 4]));
        let second = RgbaImage::from_pixel(1, 4, image::Rgba([5, 6, 7, 8]));
        let mut remaining = MAX_MATCH_BYTES;
        assert!(
            overlap(
                &first,
                &second,
                &[1, 1, 1],
                &[1, 1, 1, 1],
                1,
                &mut remaining
            )
            .is_err()
        );
        let mut remaining = 3;
        assert!(compare_bytes(&[1, 2, 3, 4], &[1, 2, 3, 4], &mut remaining).is_err());
    }

    #[test]
    fn prefix_candidates_match_exhaustive_small_sequence_oracle() {
        for left_len in 2..=5_u32 {
            for right_len in 2..=5_u32 {
                for left_bits in 0..1_u32 << left_len {
                    for right_bits in 0..1_u32 << right_len {
                        let left = RgbaImage::from_fn(1, left_len, |_, row| {
                            image::Rgba([((left_bits >> row) & 1) as u8, 0, 0, 255])
                        });
                        let right = RgbaImage::from_fn(1, right_len, |_, row| {
                            image::Rgba([((right_bits >> row) & 1) as u8, 0, 0, 255])
                        });
                        for minimum in 1..left_len.min(right_len) as usize {
                            let candidates: Vec<_> = (minimum..left_len.min(right_len) as usize)
                                .filter(|rows| {
                                    left.as_raw()[left.as_raw().len() - rows * 4..]
                                        == right.as_raw()[..rows * 4]
                                })
                                .collect();
                            let expected = if left == right || candidates.len() != 1 {
                                None
                            } else {
                                Some(candidates[0])
                            };
                            let mut remaining = MAX_MATCH_BYTES;
                            let actual = overlap(
                                &left,
                                &right,
                                &row_hashes(&left),
                                &row_hashes(&right),
                                minimum,
                                &mut remaining,
                            )
                            .ok();
                            assert_eq!(
                                actual, expected,
                                "left {left_bits}/{left_len}, right {right_bits}/{right_len}, minimum {minimum}"
                            );
                        }
                    }
                }
            }
        }
    }
}
