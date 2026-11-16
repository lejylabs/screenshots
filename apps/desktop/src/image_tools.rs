//! Explicit, bounded image utilities; never initializes the desktop application.

use anyhow::{Context, Result, bail, ensure};
use image::{Rgba, RgbaImage};
use std::{ffi::OsString, path::Path};

fn text(value: &OsString) -> Result<&str> {
    value.to_str().context("Option must be valid UTF-8")
}

fn number(value: &OsString) -> Result<u32> {
    text(value)?
        .parse()
        .context("Expected a nonnegative whole number")
}

pub(crate) fn color(value: &OsString) -> Result<Rgba<u8>> {
    let value = text(value)?;
    ensure!(
        matches!(value.len(), 6 | 8) && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Background must be RRGGBB or RRGGBBAA hexadecimal"
    );
    let mut channels = [0, 0, 0, 255];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        channels[index] = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(Rgba(channels))
}

fn destination(args: &[OsString]) -> Result<&Path> {
    let path = Path::new(&args[2]);
    app_core::ImageExportFormat::from_path(path)?;
    ensure!(!path.try_exists()?, "Output already exists");
    Ok(path)
}

fn save(pixels: RgbaImage, destination: &Path) -> Result<()> {
    let (width, height) = pixels.dimensions();
    let format = crate::cli::frame(pixels).save_image_as(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
    )?;
    println!(
        "{}",
        serde_json::json!({"format": format.extension(), "width": width, "height": height})
    );
    Ok(())
}

pub(crate) fn overlay(output: &mut RgbaImage, source: &RgbaImage, x: u32, y: u32) -> Result<()> {
    Ok(app_core::composite_rgba_with_cancel(
        output,
        source,
        x,
        y,
        || false,
    )?)
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() >= 3,
        "Usage: screenshots image compose|resize|remove-strip INPUT OUTPUT [options]"
    );
    match text(&args[0])? {
        "compose" => compose(args),
        "resize" => resize(args),
        "remove-strip" => remove_strip(args),
        _ => bail!("Unknown image utility; use --help"),
    }
}

fn remove_strip(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 6,
        "Usage: screenshots image remove-strip INPUT OUTPUT horizontal|vertical START LENGTH"
    );
    let vertical = match text(&args[3])? {
        "vertical" => true,
        "horizontal" => false,
        _ => bail!("Strip axis must be horizontal or vertical"),
    };
    let start = number(&args[4])?;
    let length = number(&args[5])?;
    ensure!(
        length > 0 && start.checked_add(length).is_some(),
        "Invalid strip range"
    );
    let destination = destination(args)?;
    let source = app_core::load_editor_image(Path::new(&args[1]))?;
    let output = app_core::remove_image_strip(&source, vertical, start, length)?;
    drop(source);
    save(output, destination)
}

fn compose(args: &[OsString]) -> Result<()> {
    let destination = destination(args)?;
    let mut padding = 0;
    let mut background = Rgba([0, 0, 0, 0]);
    let mut watermark = None;
    let mut at = None;
    let mut seen = std::collections::HashSet::new();
    let mut index = 3;
    while index < args.len() {
        let option = text(&args[index])?;
        ensure!(seen.insert(option), "Duplicate image composition option");
        index += 1;
        let value = args
            .get(index)
            .context("Missing composition option value")?;
        match option {
            "--padding" => padding = number(value)?,
            "--background" => background = color(value)?,
            "--watermark" => watermark = Some(Path::new(value)),
            "--at" => {
                let y = args.get(index + 1).context("--at requires X and Y")?;
                at = Some((number(value)?, number(y)?));
                index += 1;
            }
            _ => bail!("Unknown image composition option; use --help"),
        }
        index += 1;
    }
    ensure!(
        watermark.is_some() == at.is_some(),
        "--watermark and --at X Y must be used together"
    );
    let source = app_core::load_editor_image(Path::new(&args[1]))?;
    let border = padding.checked_mul(2).context("Padding is too large")?;
    let width = source
        .width()
        .checked_add(border)
        .context("Canvas width overflow")?;
    let height = source
        .height()
        .checked_add(border)
        .context("Canvas height overflow")?;
    app_core::validate_image_size(width, height)?;
    let watermark = match (watermark, at) {
        (Some(path), Some((x, y))) => {
            ensure!(
                x < width && y < height,
                "Watermark origin must be inside the output canvas"
            );
            let budget = app_core::MAX_CAPTURE_BYTES - source.as_raw().len() as u64;
            Some((app_core::load_editor_image_with_limit(path, budget)?, x, y))
        }
        _ => None,
    };
    let mut output = RgbaImage::from_pixel(width, height, background);
    overlay(&mut output, &source, padding, padding)?;
    if let Some((watermark, x, y)) = watermark {
        overlay(&mut output, &watermark, x, y)?;
    }
    save(output, destination)
}

fn resize(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 5,
        "Usage: screenshots image resize INPUT OUTPUT WIDTH HEIGHT"
    );
    let destination = destination(args)?;
    let width = number(&args[3])?;
    let height = number(&args[4])?;
    app_core::validate_image_size(width, height)?;
    let source = app_core::load_editor_image(Path::new(&args[1]))?;
    save(resize_pixels(&source, width, height)?, destination)
}

pub(crate) fn resize_pixels(source: &RgbaImage, width: u32, height: u32) -> Result<RgbaImage> {
    Ok(app_core::resize_nearest_rgba_with_cancel(
        source,
        width,
        height,
        || false,
    )?)
}
