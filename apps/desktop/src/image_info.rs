//! Read-only decoded-image details and exact pixel values.

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{ffi::OsString, io::Write as _, path::Path};

fn coordinate(value: &OsString) -> Result<u32> {
    let value = value.to_str().context("Coordinate must be valid UTF-8")?;
    ensure!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "Coordinate must be an unsigned whole number"
    );
    value.parse().context("Coordinate is too large")
}

pub fn run(args: &[OsString]) -> Result<()> {
    let result = match args.first().and_then(|arg| arg.to_str()) {
        Some("info") => {
            ensure!(args.len() == 2, "Usage: screenshots image info INPUT");
            let source = Path::new(&args[1]);
            let source_file_bytes = source.metadata()?.len();
            let pixels = app_core::load_editor_image(source)?;
            json!({"width":pixels.width(),"height":pixels.height(),"decoded_rgba_bytes":pixels.as_raw().len(),"source_file_bytes":source_file_bytes})
        }
        Some("pixel") => {
            ensure!(args.len() == 4, "Usage: screenshots image pixel INPUT X Y");
            let x = coordinate(&args[2])?;
            let y = coordinate(&args[3])?;
            let pixels = app_core::load_editor_image(Path::new(&args[1]))?;
            ensure!(
                x < pixels.width() && y < pixels.height(),
                "Pixel coordinates are outside the image"
            );
            let rgba = pixels.get_pixel(x, y).0;
            json!({"width":pixels.width(),"height":pixels.height(),"x":x,"y":y,"rgba":rgba,"hex_rgba":format!("#{:02X}{:02X}{:02X}{:02X}",rgba[0],rgba[1],rgba[2],rgba[3])})
        }
        _ => bail!("Unknown image information command; use --help"),
    };
    // Branch-local decoded pixels have been dropped before writing stdout.
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}").context("Could not write image information")?;
    output.flush().context("Could not flush image information")
}
