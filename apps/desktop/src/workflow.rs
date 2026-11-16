//! Explicit offline recipes. Only the final image is published to disk.

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, fs::OpenOptions, io::Read, path::Path, sync::Arc};

const MAX_RECIPE_BYTES: u64 = 64 * 1024;
const MAX_STEPS: usize = 16;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    version: u32,
    steps: Vec<Step>,
}

impl Recipe {
    pub(crate) fn config_digest(&self) -> Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        Ok(Sha256::digest(serde_json::to_vec(self)?).into())
    }
    pub(crate) fn read(path: &Path) -> Result<Self> {
        read_recipe(path)
    }

    pub(crate) fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub(crate) fn apply(self, pixels: RgbaImage) -> Result<RgbaImage> {
        self.apply_with_cancel(pixels, &|| Ok(()))
    }

    pub(crate) fn apply_with_cancel(
        self,
        mut pixels: RgbaImage,
        check: &impl Fn() -> Result<()>,
    ) -> Result<RgbaImage> {
        for (index, step) in self.steps.into_iter().enumerate() {
            check()?;
            let result = step.apply(pixels, &|| check().is_err());
            check()?;
            pixels = result.with_context(|| format!("Workflow step {} failed", index + 1))?;
        }
        Ok(pixels)
    }
}

#[derive(Deserialize, Serialize)]
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
    RemoveStrip {
        axis: Axis,
        start: u32,
        length: u32,
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
    Blur {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Pixelate {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

#[derive(Deserialize, Serialize)]
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
            }
            | Self::Blur {
                x,
                y,
                width,
                height,
            }
            | Self::Pixelate {
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
            Self::RemoveStrip { start, length, .. } => {
                ensure!(
                    *length > 0 && start.checked_add(*length).is_some(),
                    "Invalid workflow strip range"
                );
            }
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

    fn apply(self, pixels: RgbaImage, cancelled: &impl Fn() -> bool) -> Result<RgbaImage> {
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
            }
            | Self::Blur {
                x,
                y,
                width,
                height,
            }
            | Self::Pixelate {
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
                let grid = if matches!(self, Self::Pixelate { .. }) {
                    ensure!(
                        x.checked_add(width).is_some_and(|end| end <= source.width)
                            && y.checked_add(height)
                                .is_some_and(|end| end <= source.height),
                        "Edit area is outside the image"
                    );
                    Some(app_core::PixelationGrid::prepare(
                        &source.pixels,
                        cancelled,
                    )?)
                } else {
                    None
                };
                let output = app_core::render_ordered_snapshot_with_cancel(
                    &source,
                    if matches!(self, Self::Redact { .. }) {
                        std::slice::from_ref(&region)
                    } else {
                        &[]
                    },
                    if matches!(self, Self::Crop { .. }) {
                        Some(region)
                    } else {
                        None
                    },
                    &[],
                    if matches!(self, Self::Pixelate { .. }) {
                        std::slice::from_ref(&region)
                    } else {
                        &[]
                    },
                    grid.as_ref(),
                    if matches!(self, Self::Blur { .. }) {
                        std::slice::from_ref(&region)
                    } else {
                        &[]
                    },
                    cancelled,
                )?;
                Arc::try_unwrap(output.pixels)
                    .map_err(|_| anyhow::anyhow!("Workflow edit retained its output pixels"))
            }
            Self::Rotate { degrees } => Ok(app_core::rotate_rgba_with_cancel(
                pixels, degrees, cancelled,
            )?),
            Self::Flip { axis } => Ok(app_core::flip_rgba_with_cancel(
                pixels,
                matches!(axis, Axis::Horizontal),
                cancelled,
            )?),
            Self::RemoveStrip {
                axis,
                start,
                length,
            } => Ok(app_core::remove_image_strip_with_cancel(
                &pixels,
                matches!(axis, Axis::Vertical),
                start,
                length,
                cancelled,
            )?),
            Self::Resize { width, height } => Ok(app_core::resize_nearest_rgba_with_cancel(
                &pixels, width, height, cancelled,
            )?),
            Self::Canvas {
                padding,
                background,
            } => {
                let background = crate::image_tools::color(&OsString::from(background))?;
                Ok(app_core::canvas_rgba_with_cancel(
                    &pixels,
                    padding,
                    background.0,
                    cancelled,
                )?)
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
    let file = open_recipe(path)?;
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

fn open_recipe(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    // A FIFO replacing the regular file after preflight must not wait for a writer.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 3,
        "Usage: screenshots workflow RECIPE INPUT OUTPUT"
    );
    let destination = Path::new(&args[2]);
    app_core::ImageExportFormat::from_path(destination)?;
    ensure!(!destination.try_exists()?, "Output already exists");
    #[cfg(unix)]
    let cancellation = crate::record::unix::Cancellation::new()?;
    #[cfg(unix)]
    let flag = cancellation.flag();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let check = || -> Result<()> {
        #[cfg(unix)]
        ensure!(
            !flag.load(std::sync::atomic::Ordering::Relaxed),
            "Workflow canceled"
        );
        ensure!(
            std::time::Instant::now() < deadline,
            "Workflow exceeded its five-minute processing budget"
        );
        Ok(())
    };
    check()?;
    let recipe = Recipe::read(Path::new(&args[0]))?;
    let step_count = recipe.step_count();
    let pixels =
        recipe.apply_with_cancel(app_core::load_editor_image(Path::new(&args[1]))?, &check)?;
    let (width, height) = pixels.dimensions();
    let format = crate::cli::frame(pixels).save_image_as_with_cancel(
        destination,
        app_core::DEFAULT_JPEG_QUALITY,
        app_core::ExportSize::Original,
        || check().is_err(),
    )?;
    #[cfg(unix)]
    cancellation.output_phase(true);
    use std::io::Write as _;
    let mut output = std::io::stdout().lock();
    writeln!(output,
        "{}",
        serde_json::json!({
            "version": 1, "steps": step_count,
            "format": format.extension(), "width": width, "height": height,
        })
    ).and_then(|_|output.flush()).context("Workflow image was saved but its result could not be written; the completed output remains")?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn recipe_replaced_with_fifo_is_opened_without_waiting_for_a_writer() {
        const CHILD_PATH: &str = "SCREENSHOTS_WORKFLOW_FIFO_TEST_PATH";
        if let Some(path) = std::env::var_os(CHILD_PATH) {
            let path = Path::new(&path);
            let opened = open_recipe(path).unwrap();
            assert!(!opened.metadata().unwrap().is_file());
            assert!(read_recipe(path).is_err());
            return;
        }
        let root =
            std::env::temp_dir().join(format!("screenshots-recipe-race-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("recipe.json");
        std::fs::write(&path, b"regular at preflight").unwrap();
        assert!(path.metadata().unwrap().is_file());
        std::fs::remove_file(&path).unwrap();
        let native = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // The path is a NUL-terminated fixture path, and mkfifo reads it only during the call.
        assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "workflow::tests::recipe_replaced_with_fifo_is_opened_without_waiting_for_a_writer",
                "--nocapture",
            ])
            .env(CHILD_PATH, &path)
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if started.elapsed() >= std::time::Duration::from_secs(3) {
                child.kill().unwrap();
                child.wait().unwrap();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(root).unwrap();
        assert!(
            status.is_some_and(|status| status.success()),
            "FIFO open failed or timed out"
        );
    }
}
