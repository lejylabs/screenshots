//! One explicit capture, optional in-memory recipe, and one final file output.

use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, CapturedImage, ExportSize, ImageExportFormat, XcapBackend};
use std::{ffi::OsString, io::Write, path::Path, sync::Arc, time::Duration};

struct Options<'a> {
    display_id: &'a str,
    destination: &'a Path,
    recipe: Option<crate::workflow::Recipe>,
    delay: Duration,
    jpeg_quality: u8,
}

fn ensure_new_output(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => bail!("Output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(
        args.len() >= 3 && args.len() % 2 == 1 && args[0] == "display",
        "Usage: screenshots capture-task display ID OUTPUT [--recipe RECIPE] [--delay-ms MS] [--jpeg-quality 1..100]"
    );
    let display_id = args[1].to_str().context("Display ID must be UTF-8")?;
    ensure!(
        !display_id.is_empty() && display_id.len() <= 64,
        "Display ID must contain 1–64 UTF-8 bytes"
    );
    let mut recipe_path = None;
    let mut delay = None;
    let mut quality = None;
    for pair in args[3..].chunks_exact(2) {
        match pair[0].to_str() {
            Some("--recipe") => {
                ensure!(recipe_path.is_none(), "Duplicate --recipe option");
                recipe_path = Some(Path::new(&pair[1]));
            }
            Some("--delay-ms") => {
                ensure!(delay.is_none(), "Duplicate --delay-ms option");
                let text = pair[1].to_str().context("Delay must be UTF-8")?;
                let value: u64 = text.parse().context("Delay must be a whole number")?;
                ensure!(value <= 3_600_000, "Delay must be 0–3600000 milliseconds");
                delay = Some(Duration::from_millis(value));
            }
            Some("--jpeg-quality") => {
                ensure!(quality.is_none(), "Duplicate --jpeg-quality option");
                let text = pair[1].to_str().context("JPEG quality must be UTF-8")?;
                let value: u8 = text
                    .parse()
                    .context("JPEG quality must be a whole number")?;
                ensure!((1..=100).contains(&value), "JPEG quality must be 1..100");
                quality = Some(value);
            }
            _ => bail!("Unknown capture-task option; use --help"),
        }
    }
    let destination = Path::new(&args[2]);
    let format = ImageExportFormat::from_path(destination)?;
    ensure!(
        quality.is_none() || format == ImageExportFormat::Jpeg,
        "--jpeg-quality requires a JPEG output path"
    );
    ensure_new_output(destination)?;
    let recipe = recipe_path.map(crate::workflow::Recipe::read).transpose()?;
    Ok(Options {
        display_id,
        destination,
        recipe,
        delay: delay.unwrap_or_default(),
        jpeg_quality: quality.unwrap_or(app_core::DEFAULT_JPEG_QUALITY),
    })
}

fn execute(
    options: Options<'_>,
    wait: impl FnOnce(Duration) -> Result<()>,
    capture: impl FnOnce(&str) -> Result<CapturedImage>,
) -> Result<serde_json::Value> {
    if !options.delay.is_zero() {
        wait(options.delay).context("Capture delay failed")?;
    }
    // A file created during the delay must stop the task before native capture.
    ensure_new_output(options.destination)?;
    let frame = capture(options.display_id).context("Capture task failed")?;
    app_core::validate_image_size(frame.pixels.width(), frame.pixels.height())?;
    let steps = options
        .recipe
        .as_ref()
        .map_or(0, crate::workflow::Recipe::step_count);
    let frame = if let Some(recipe) = options.recipe {
        let pixels = Arc::try_unwrap(frame.pixels)
            .map_err(|_| anyhow::anyhow!("Capture task retained its source pixels"))?;
        crate::cli::frame(recipe.apply(pixels)?)
    } else {
        frame
    };
    let format = frame.save_image_as(
        options.destination,
        options.jpeg_quality,
        ExportSize::Original,
    )?;
    Ok(
        serde_json::json!({"format":format.extension(), "width":frame.width, "height":frame.height,
        "steps":steps, "delay_ms":options.delay.as_millis()}),
    )
}

pub fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    let result = execute(
        options,
        |delay| {
            std::thread::sleep(delay);
            Ok(())
        },
        |id| {
            let backend = XcapBackend::new(std::path::PathBuf::new());
            Ok(backend.capture_display(id)?)
        },
    )?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{result}")
        .and_then(|_| stdout.flush())
        .context("Capture task output was saved, but its result could not be written")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("screenshots-capture-task-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn args(&self, output: &str, extra: &[&str]) -> Vec<OsString> {
            let mut args = vec![
                "display".into(),
                "fixture".into(),
                self.0.join(output).into_os_string(),
            ];
            args.extend(extra.iter().map(OsString::from));
            args
        }
        fn recipe(&self, text: &[u8]) -> OsString {
            let path = self.0.join("recipe.json");
            std::fs::write(&path, text).unwrap();
            path.into_os_string()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn delay_and_recipe_are_snapshotted_before_capture_and_only_final_pixels_are_saved() {
        let fixture = Fixture::new();
        let recipe_path = fixture.recipe(br#"{"version":1,"steps":[{"op":"crop","x":1,"y":0,"width":2,"height":2},{"op":"redact","x":0,"y":0,"width":1,"height":2},{"op":"rotate","degrees":90}]}"#);
        let mut args = fixture.args("final.webp", &["--delay-ms", "1234"]);
        args.extend(["--recipe".into(), recipe_path.clone()]);
        let parsed = options(&args).unwrap();
        let waited = Cell::new(false);
        let previous = RefCell::new(None::<std::sync::Weak<image::RgbaImage>>);
        let source = image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 50, y as u8 * 80, 123, 255])
        });
        let result = execute(
            parsed,
            |delay| {
                assert_eq!(delay, Duration::from_millis(1234));
                waited.set(true);
                std::fs::write(Path::new(&recipe_path), b"changed after validation").unwrap();
                Ok(())
            },
            |id| {
                assert!(waited.get());
                assert_eq!(id, "fixture");
                let frame = crate::cli::frame(source.clone());
                let weak = Arc::downgrade(&frame.pixels);
                // The returned frame is the only strong reference before recipe application.
                assert_eq!(weak.strong_count(), 1);
                *previous.borrow_mut() = Some(weak);
                Ok(frame)
            },
        )
        .unwrap();
        assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
        assert_eq!(result["steps"], 3);
        assert_eq!(result["delay_ms"], 1234);
        let pixels = image::open(fixture.0.join("final.webp"))
            .unwrap()
            .into_rgba8();
        assert_eq!(pixels.dimensions(), (2, 2));
        assert_eq!(*pixels.get_pixel(0, 0), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*pixels.get_pixel(1, 0), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*pixels.get_pixel(0, 1), *source.get_pixel(2, 1));
        assert_eq!(*pixels.get_pixel(1, 1), *source.get_pixel(2, 0));
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }

    #[test]
    fn zero_delay_is_immediate_and_dropped_frame_outputs_original_alpha() {
        let fixture = Fixture::new();
        let args = fixture.args("final.png", &[]);
        let frame = crate::cli::frame(image::RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([42, 9, 7, 93]),
        ));
        let weak = Arc::downgrade(&frame.pixels);
        let result = execute(
            options(&args).unwrap(),
            |_| panic!("zero delay must not sleep"),
            |_| Ok(frame),
        )
        .unwrap();
        assert!(weak.upgrade().is_none());
        assert_eq!(result["steps"], 0);
        assert_eq!(result["delay_ms"], 0);
        assert_eq!(
            *image::open(fixture.0.join("final.png"))
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0),
            image::Rgba([42, 9, 7, 93])
        );
    }

    #[test]
    fn delayed_collision_and_wait_failure_stop_before_capture() {
        for collision in [true, false] {
            let fixture = Fixture::new();
            let args = fixture.args("final.png", &["--delay-ms", "1"]);
            let result = execute(
                options(&args).unwrap(),
                |_| {
                    if collision {
                        std::fs::write(fixture.0.join("final.png"), b"preserve")?;
                        Ok(())
                    } else {
                        bail!("fixture cancelled")
                    }
                },
                |_| panic!("failed delay or output validation must not capture"),
            );
            assert!(result.is_err());
            if collision {
                assert_eq!(
                    std::fs::read(fixture.0.join("final.png")).unwrap(),
                    b"preserve"
                );
            } else {
                assert!(!fixture.0.join("final.png").exists());
            }
        }
    }

    #[test]
    fn recipe_failure_and_jpeg_alpha_refusal_publish_nothing() {
        for recipe in [true, false] {
            let fixture = Fixture::new();
            let mut args = fixture.args(if recipe { "final.png" } else { "final.jpg" }, &[]);
            if recipe {
                args.extend(["--recipe".into(), fixture.recipe(br#"{"version":1,"steps":[{"op":"crop","x":2,"y":0,"width":1,"height":1}]}"#)]);
            }
            let frame = crate::cli::frame(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([1, 2, 3, 90]),
            ));
            let weak = Arc::downgrade(&frame.pixels);
            assert!(
                execute(
                    options(&args).unwrap(),
                    |_| panic!("no delay"),
                    |_| Ok(frame)
                )
                .is_err()
            );
            assert!(weak.upgrade().is_none());
            assert!(
                !fixture
                    .0
                    .join(if recipe { "final.png" } else { "final.jpg" })
                    .exists()
            );
        }
    }

    #[test]
    fn delay_bounds_and_option_order_are_checked_before_capture() {
        let fixture = Fixture::new();
        for delay in ["0", "1", "3600000"] {
            let args = fixture.args("final.jpg", &["--jpeg-quality", "1", "--delay-ms", delay]);
            let parsed = options(&args).unwrap();
            assert_eq!(parsed.delay.as_millis(), delay.parse::<u128>().unwrap());
        }
        for extra in [
            vec!["--delay-ms", "3600001"],
            vec!["--delay-ms", "-1"],
            vec!["--delay-ms", "1.5"],
            vec!["--delay-ms", "1", "--delay-ms", "2"],
            vec!["--jpeg-quality", "0"],
            vec!["--jpeg-quality", "101"],
            vec!["--jpeg-quality", "30", "--jpeg-quality", "40"],
            vec!["--unknown", "x"],
        ] {
            assert!(
                options(&fixture.args("final.jpg", &extra)).is_err(),
                "{extra:?}"
            );
        }
        assert!(options(&fixture.args("final.png", &["--jpeg-quality", "80"])).is_err());
    }
}
