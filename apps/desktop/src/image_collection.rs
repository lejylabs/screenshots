//! Explicit file collections; bounded decoded pixels and no application state.

use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use serde_json::json;
use std::{ffi::OsString, io::Write as _, path::Path};

fn text(value: &OsString) -> Result<&str> {
    value.to_str().context("Option must be valid UTF-8")
}

fn fresh_output(path: &Path, png_only: bool) -> Result<()> {
    let format = app_core::ImageExportFormat::from_path(path)?;
    ensure!(
        !png_only || format == app_core::ImageExportFormat::Png,
        "Difference output must use .png"
    );
    ensure!(
        std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Output already exists or is unavailable"
    );
    Ok(())
}

fn print_result(value: serde_json::Value, completed: &str) -> Result<()> {
    let mut output = std::io::stdout().lock();
    (|| {
        writeln!(output, "{value}")?;
        output.flush()
    })()
    .with_context(|| format!("{completed}; could not write the JSON result"))
}

fn save(pixels: RgbaImage, destination: &Path) -> Result<app_core::ImageExportFormat> {
    Ok(crate::cli::frame(pixels).save_image_as(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
    )?)
}

pub fn run(args: &[OsString]) -> Result<()> {
    match args.first().map(OsString::as_os_str) {
        Some(command) if command == "combine" => combine(args),
        Some(command) if command == "compare" => compare(args),
        Some(command) if command == "split" => split(args),
        _ => bail!("Unknown image collection command; use --help"),
    }
}

fn combine(args: &[OsString]) -> Result<()> {
    ensure!(
        (5..=19).contains(&args.len()),
        "Usage: screenshots image combine horizontal|vertical OUTPUT INPUT INPUT [INPUT...] (2–16 inputs)"
    );
    let horizontal = match text(&args[1])? {
        "horizontal" => true,
        "vertical" => false,
        _ => bail!("Combine direction must be horizontal or vertical"),
    };
    let destination = Path::new(&args[2]);
    fresh_output(destination, false)?;
    let mut sources = Vec::with_capacity(args.len() - 3);
    let (mut width, mut height) = (0_u32, 0_u32);
    let mut remaining = app_core::MAX_CAPTURE_BYTES;
    for input in &args[3..] {
        let source = app_core::load_editor_image_with_limit(Path::new(input), remaining)?;
        remaining -= u64::try_from(source.as_raw().len())?;
        if horizontal {
            width = width
                .checked_add(source.width())
                .context("Combined width overflow")?;
            height = height.max(source.height());
        } else {
            width = width.max(source.width());
            height = height
                .checked_add(source.height())
                .context("Combined height overflow")?;
        }
        app_core::validate_image_size(width, height)?;
        sources.push(source);
    }
    let count = sources.len();
    let mut output = RgbaImage::new(width, height);
    let (mut x, mut y) = (0, 0);
    for source in sources {
        copy_at(&mut output, &source, x, y);
        if horizontal {
            x += source.width();
        } else {
            y += source.height();
        }
        // The owned source is released here; all sources are gone before encoding.
    }
    let format = save(output, destination)?;
    print_result(
        json!({"format":format.extension(),"width":width,"height":height,"inputs":count}),
        "Combined image was saved",
    )
}

fn copy_at(destination: &mut RgbaImage, source: &RgbaImage, x: u32, y: u32) {
    let destination_stride = destination.width() as usize * 4;
    let source_stride = source.width() as usize * 4;
    let destination: &mut [u8] = destination.as_mut();
    for (row, pixels) in source.as_raw().chunks_exact(source_stride).enumerate() {
        let start = (y as usize + row) * destination_stride + x as usize * 4;
        destination[start..start + source_stride].copy_from_slice(pixels);
    }
}

fn compare(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 4,
        "Usage: screenshots image compare FIRST SECOND OUTPUT.png"
    );
    let destination = Path::new(&args[3]);
    fresh_output(destination, true)?;
    let mut first = app_core::load_editor_image(Path::new(&args[1]))?;
    let remaining = app_core::MAX_CAPTURE_BYTES - u64::try_from(first.as_raw().len())?;
    let second = app_core::load_editor_image_with_limit(Path::new(&args[2]), remaining)?;
    ensure!(
        first.dimensions() == second.dimensions(),
        "Compared images must have identical dimensions"
    );
    let (width, height) = first.dimensions();
    let mut changed = 0_u64;
    for (left, right) in first.pixels_mut().zip(second.pixels()) {
        if *left != *right {
            changed += 1;
        }
        let alpha = left[3].abs_diff(right[3]);
        for channel in 0..3 {
            left[channel] = left[channel].abs_diff(right[channel]).max(alpha);
        }
        left[3] = 255;
    }
    drop(second);
    save(first, destination)?;
    print_result(
        json!({"format":"png","width":width,"height":height,"changed_pixels":changed,"total_pixels":u64::from(width)*u64::from(height)}),
        "Difference image was saved",
    )
}

fn split(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 7,
        "Usage: screenshots image split INPUT DIRECTORY --columns N --rows N"
    );
    let (mut columns, mut rows) = (None, None);
    for pair in args[3..].chunks_exact(2) {
        let field = match text(&pair[0])? {
            "--columns" => &mut columns,
            "--rows" => &mut rows,
            _ => bail!("Unknown image split option; use --help"),
        };
        ensure!(field.is_none(), "Duplicate image split option");
        let value: u32 = text(&pair[1])?
            .parse()
            .context("Grid values must be whole numbers")?;
        ensure!((1..=16).contains(&value), "Grid values must be 1–16");
        *field = Some(value);
    }
    let columns = columns.context("Missing --columns")?;
    let rows = rows.context("Missing --rows")?;
    ensure!(columns * rows <= 64, "Split supports at most 64 tiles");
    let directory = Path::new(&args[2]);
    ensure!(
        directory.metadata()?.is_dir(),
        "Split destination must be an existing empty directory"
    );
    ensure!(
        std::fs::read_dir(directory)?.next().transpose()?.is_none(),
        "Split destination must be empty"
    );
    let source = app_core::load_editor_image(Path::new(&args[1]))?;
    let (width, height) = source.dimensions();
    ensure!(
        width.is_multiple_of(columns) && height.is_multiple_of(rows),
        "Image dimensions must divide evenly by the grid"
    );
    let (tile_width, tile_height) = (width / columns, height / rows);
    let files: Vec<_> = (1..=columns * rows)
        .map(|index| format!("tile-{index:03}.png"))
        .collect();
    for name in &files {
        fresh_output(&directory.join(name), true)?;
    }
    for (index, name) in files.iter().enumerate() {
        let x = index as u32 % columns * tile_width;
        let y = index as u32 / columns * tile_height;
        let tile = image::imageops::crop_imm(&source, x, y, tile_width, tile_height).to_image();
        save(tile, &directory.join(name)).with_context(|| format!("Split stopped after {index} completed tile(s); completed tile-NNN.png files remain in {}", directory.display()))?;
    }
    let count = files.len();
    print_result(
        json!({"completed":count,"columns":columns,"rows":rows,"tile_width":tile_width,"tile_height":tile_height,"files":files}),
        &format!(
            "All {count} split tiles were saved in {}",
            directory.display()
        ),
    )
}
