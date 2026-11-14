//! Explicit QR utilities. Nothing initializes or runs while the desktop is idle.

use anyhow::{Context, Result, bail, ensure};
use image::{Rgba, RgbaImage};
use qrcode::{Color, EcLevel, QrCode};
use std::{ffi::OsString, path::Path};

const MAX_GENERATED_TEXT_BYTES: usize = 2048;
const MAX_SCAN_RGBA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SCAN_SIDE: u32 = 4096;
// quircs silently stops at eight grids; reject saturation rather than omit codes.
const MAX_RESULTS: usize = 7;
const MAX_RESULT_BYTES: usize = 8192;
const QUIET_MODULES: u32 = 4;
const MODULE_PIXELS: u32 = 4;

pub fn run(args: &[OsString]) -> Result<()> {
    let mode = args.first().and_then(|arg| arg.to_str());
    match mode {
        Some("generate") if args.len() == 3 => generate(&args[1], Path::new(&args[2])),
        Some("scan") if args.len() == 2 => scan(Path::new(&args[1])),
        _ => bail!("Usage: screenshots qr generate TEXT OUTPUT_PNG; screenshots qr scan INPUT"),
    }
}

fn generate(text: &OsString, destination: &Path) -> Result<()> {
    let text = text.to_str().context("QR text must be valid UTF-8")?;
    ensure!(
        !text.is_empty() && text.len() <= MAX_GENERATED_TEXT_BYTES,
        "QR text must contain 1–2048 UTF-8 bytes"
    );
    ensure!(
        app_core::ImageExportFormat::from_path(destination)? == app_core::ImageExportFormat::Png,
        "QR output must be PNG"
    );
    ensure!(!destination.try_exists()?, "Output already exists");
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .context("Text cannot fit a normal QR code")?;
    // Normal QR versions 1–40 have widths 21–177; no Micro QR output is implied.
    let modules = u32::try_from(code.width())?;
    ensure!((21..=177).contains(&modules), "Unsupported QR version");
    let side = (modules + QUIET_MODULES * 2) * MODULE_PIXELS;
    app_core::validate_image_size(side, side)?;
    let pixels = RgbaImage::from_fn(side, side, |x, y| {
        let x = x / MODULE_PIXELS;
        let y = y / MODULE_PIXELS;
        let dark = x >= QUIET_MODULES
            && y >= QUIET_MODULES
            && x < modules + QUIET_MODULES
            && y < modules + QUIET_MODULES
            && code[((x - QUIET_MODULES) as usize, (y - QUIET_MODULES) as usize)] == Color::Dark;
        let level = if dark { 0 } else { 255 };
        Rgba([level, level, level, 255])
    });
    crate::cli::frame(pixels).save_png_as(destination)?;
    println!(
        "{}",
        serde_json::json!({"format": "png", "width": side, "height": side, "error_correction": "M"})
    );
    Ok(())
}

fn scan(source: &Path) -> Result<()> {
    let pixels = app_core::load_editor_image_with_limit(source, MAX_SCAN_RGBA_BYTES)?;
    let (width, height) = pixels.dimensions();
    ensure!(
        width <= MAX_SCAN_SIDE && height <= MAX_SCAN_SIDE,
        "QR scan dimensions must not exceed 4096 pixels per side"
    );
    let grayscale = pixels
        .pixels()
        .map(|pixel| {
            let [r, g, b, a] = pixel.0.map(u32::from);
            let luminance = (299 * r + 587 * g + 114 * b + 500) / 1000;
            // Match visible pixels on a white matte; hidden RGB is never a QR signal.
            ((luminance * a + 255 * (255 - a) + 127) / 255) as u8
        })
        .collect::<Vec<_>>();
    drop(pixels);
    ensure!(
        grayscale.len() <= 4 * 1024 * 1024,
        "QR grayscale budget exceeded"
    );
    let mut decoder = quircs::Quirc::default();
    let codes = decoder
        .identify(width as usize, height as usize, &grayscale)
        .take(MAX_RESULTS + 1)
        .collect::<Vec<_>>();
    // These upstream limits stop candidate grouping from growing with noisy input.
    ensure!(
        decoder.num_capstones() < 32 && decoder.num_regions() < 65_534,
        "QR scan reached its detector candidate limit; crop to fewer codes"
    );
    ensure!(
        codes.len() <= MAX_RESULTS,
        "QR scan contains too many codes"
    );
    ensure!(!codes.is_empty(), "No QR code found");
    let mut results = Vec::with_capacity(codes.len());
    for code in codes {
        let decoded = code
            .context("Could not extract a QR grid")?
            .decode()
            .context("Could not decode a detected QR code")?;
        ensure!(
            decoded.payload.len() <= MAX_RESULT_BYTES,
            "QR payload exceeds 8192 bytes"
        );
        let text = std::str::from_utf8(&decoded.payload).context("QR payload is not UTF-8 text")?;
        results.push(serde_json::json!({"text": text, "version": decoded.version}));
    }
    println!("{}", serde_json::json!({"codes": results}));
    Ok(())
}
