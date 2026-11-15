//! Explicit offline recipes. Only the final image is published to disk.

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde::Deserialize;
use std::{ffi::OsString, fs::File, io::Read, path::Path, sync::Arc};

const MAX_RECIPE_BYTES: u64 = 64 * 1024;
const MAX_STEPS: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: u32,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Rotate {
        degrees: u16,
    },
    Flip {
        axis: Axis,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Canvas {
        padding: u32,
        background: String,
    },
    Redact {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Axis {
    Horizontal,
    Vertical,
}

impl Step {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Crop {
                x,
                y,
                width,
                height,
            }
            | Self::Redact {
                x,
                y,
                width,
                height,
            } => {
                app_core::validate_image_size(*width, *height)?;
                ensure!(
                    x.checked_add(*width).is_some() && y.checked_add(*height).is_some(),
                    "Workflow region coordinates overflow"
                );
            }
            Self::Rotate { degrees } => ensure!(
                matches!(degrees, 90 | 180 | 270),
                "Workflow rotation must be 90, 180 or 270"
            ),
            Self::Flip { .. } => {}
            Self::Resize { width, height } => app_core::validate_image_size(*width, *height)?,
            Self::Canvas {
                padding,
                background,
            } => {
                padding.checked_mul(2).context("Padding is too large")?;
                crate::image_tools::color(&OsString::from(background))?;
            }
        }
        Ok(())
    }

    fn apply(self, mut pixels: RgbaImage) -> Result<RgbaImage> {
        match self {
            Self::Crop {
                x,
                y,
                width,
                height,
            }
            | Self::Redact {
                x,
                y,
                width,
                height,
            } => {
                let region = app_core::PixelRegion {
                    x,
                    y,
                    width,
                    height,
                };
                let source = crate::cli::frame(pixels);
                let output = if matches!(self, Self::Crop { .. }) {
                    app_core::crop_snapshot(&source, region)?
                } else {
                    app_core::redact_snapshot(&source, &[region])?
                };
                Arc::try_unwrap(output.pixels)
                    .map_err(|_| anyhow::anyhow!("Workflow edit retained its output pixels"))
            }
            Self::Rotate { degrees } => match degrees {
                90 => Ok(image::imageops::rotate90(&pixels)),
                180 => {
                    image::imageops::rotate180_in_place(&mut pixels);
                    Ok(pixels)
                }
                270 => Ok(image::imageops::rotate270(&pixels)),
                _ => unreachable!("recipe rotation was validated"),
            },
            Self::Flip { axis } => {
                match axis {
                    Axis::Horizontal => image::imageops::flip_horizontal_in_place(&mut pixels),
                    Axis::Vertical => image::imageops::flip_vertical_in_place(&mut pixels),
                }
                Ok(pixels)
            }
            Self::Resize { width, height } => {
                crate::image_tools::resize_pixels(&pixels, width, height)
            }
            Self::Canvas {
                padding,
                background,
            } => {
                let border = padding.checked_mul(2).context("Padding is too large")?;
                let width = pixels
                    .width()
                    .checked_add(border)
                    .context("Canvas width overflow")?;
                let height = pixels
                    .height()
                    .checked_add(border)
                    .context("Canvas height overflow")?;
                app_core::validate_image_size(width, height)?;
                let background = crate::image_tools::color(&OsString::from(background))?;
                let mut output = RgbaImage::from_pixel(width, height, background);
                crate::image_tools::overlay(&mut output, &pixels, padding, padding);
                Ok(output)
            }
        }
    }
}

fn read_recipe(path: &Path) -> Result<Recipe> {
    // Reject obvious special files before opening; recheck the opened handle after races.
    ensure!(
        path.metadata()?.is_file(),
        "Workflow recipe must be a regular file"
    );
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "Workflow recipe must be a regular file");
    ensure!(
        metadata.len() <= MAX_RECIPE_BYTES,
        "Workflow recipe exceeds 64 KiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_RECIPE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_RECIPE_BYTES,
        "Workflow recipe exceeds 64 KiB"
    );
    let recipe: Recipe = serde_json::from_slice(&bytes).context("Invalid workflow recipe")?;
    ensure!(recipe.version == 1, "Workflow version must be 1");
    ensure!(
        (1..=MAX_STEPS).contains(&recipe.steps.len()),
        "Workflow must contain 1–16 steps"
    );
    for (index, step) in recipe.steps.iter().enumerate() {
        step.validate()
            .with_context(|| format!("Invalid workflow step {}", index + 1))?;
    }
    Ok(recipe)
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 3,
        "Usage: screenshots workflow RECIPE INPUT OUTPUT"
    );
    let destination = Path::new(&args[2]);
    app_core::ImageExportFormat::from_path(destination)?;
    ensure!(!destination.try_exists()?, "Output already exists");
    let recipe = read_recipe(Path::new(&args[0]))?;
    let step_count = recipe.steps.len();
    let mut pixels = app_core::load_editor_image(Path::new(&args[1]))?;
    for (index, step) in recipe.steps.into_iter().enumerate() {
        pixels = step
            .apply(pixels)
            .with_context(|| format!("Workflow step {} failed", index + 1))?;
    }
    let (width, height) = pixels.dimensions();
    let format = crate::cli::frame(pixels).save_image_as(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "version": 1, "steps": step_count,
            "format": format.extension(), "width": width, "height": height,
        })
    );
    Ok(())
}
