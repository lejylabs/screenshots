//! One explicit capture, optional in-memory recipe, and one final file output.

use anyhow::{Context, Result, bail, ensure};
use app_core::{
    CaptureBackend, CapturedImage, ExportSize, ImageExportFormat, UploadDestination, UploadOutcome,
    XcapBackend,
};
use std::{ffi::OsString, io::Write, path::Path, sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target<'a> {
    Display(&'a str),
    Window(&'a str),
    Region {
        display: &'a str,
        area: app_core::PixelRegion,
    },
}

struct Options<'a> {
    target: Target<'a>,
    destination: &'a Path,
    recipe: Option<crate::workflow::Recipe>,
    delay: Duration,
    jpeg_quality: u8,
    upload: Option<UploadDestination>,
}

fn ensure_new_output(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => bail!("Output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    const USAGE: &str = "Usage: screenshots capture-task display|window ID OUTPUT [OPTIONS]; or capture-task region DISPLAY X Y WIDTH HEIGHT OUTPUT [OPTIONS]";
    let positional = match args.first().and_then(|arg| arg.to_str()) {
        Some("display" | "window") => 3,
        Some("region") => 7,
        _ => bail!(USAGE),
    };
    ensure!(
        args.len() >= positional && (args.len() - positional) % 2 == 0,
        USAGE
    );
    let label = if args[0] == "window" {
        "Window"
    } else {
        "Display"
    };
    let id = args[1]
        .to_str()
        .with_context(|| format!("{label} ID must be UTF-8"))?;
    ensure!(
        !id.is_empty() && id.len() <= 64,
        "{label} ID must contain 1–64 UTF-8 bytes"
    );
    let target = if positional == 7 {
        let numbers = args[2..6]
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .context("Region coordinates must be UTF-8")?
                    .parse::<u32>()
                    .context("Region coordinates must be whole nonnegative numbers")
            })
            .collect::<Result<Vec<_>>>()?;
        let area = app_core::PixelRegion {
            x: numbers[0],
            y: numbers[1],
            width: numbers[2],
            height: numbers[3],
        };
        app_core::validate_image_size(area.width, area.height)?;
        ensure!(
            area.x.checked_add(area.width).is_some() && area.y.checked_add(area.height).is_some(),
            "Region coordinates overflow"
        );
        ensure!(
            area.x <= i32::MAX as u32 && area.y <= i32::MAX as u32,
            "Region origin must fit nonnegative signed 32-bit coordinates"
        );
        Target::Region { display: id, area }
    } else if args[0] == "window" {
        Target::Window(id)
    } else {
        Target::Display(id)
    };
    let mut recipe_path = None;
    let mut delay = None;
    let mut quality = None;
    let mut upload_path = None;
    for pair in args[positional..].chunks_exact(2) {
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
            Some("--upload") => {
                ensure!(upload_path.is_none(), "Duplicate --upload option");
                upload_path = Some(Path::new(&pair[1]));
            }
            _ => bail!("Unknown capture-task option; use --help"),
        }
    }
    let destination = Path::new(&args[positional - 1]);
    let format = ImageExportFormat::from_path(destination)?;
    ensure!(
        quality.is_none() || format == ImageExportFormat::Jpeg,
        "--jpeg-quality requires a JPEG output path"
    );
    ensure_new_output(destination)?;
    let recipe = recipe_path.map(crate::workflow::Recipe::read).transpose()?;
    let upload = upload_path
        .map(crate::cli::load_upload_profile)
        .transpose()?;
    Ok(Options {
        target,
        destination,
        recipe,
        delay: delay.unwrap_or_default(),
        jpeg_quality: quality.unwrap_or(app_core::DEFAULT_JPEG_QUALITY),
        upload,
    })
}

fn execute(
    options: Options<'_>,
    wait: impl FnOnce(Duration) -> Result<()>,
    capture: impl FnOnce(Target<'_>) -> Result<CapturedImage>,
) -> Result<serde_json::Value> {
    execute_with_upload(options, wait, capture, |destination, png| {
        app_core::upload_png_bytes_with_progress(
            destination,
            png,
            app_core::UploadCancellation::default(),
            None,
        )
    })
}

fn execute_with_upload(
    options: Options<'_>,
    wait: impl FnOnce(Duration) -> Result<()>,
    capture: impl FnOnce(Target<'_>) -> Result<CapturedImage>,
    upload: impl FnOnce(&UploadDestination, Vec<u8>) -> Result<UploadOutcome>,
) -> Result<serde_json::Value> {
    if !options.delay.is_zero() {
        wait(options.delay).context("Capture delay failed")?;
    }
    // A file created during the delay must stop the task before native capture.
    ensure_new_output(options.destination)?;
    let frame = capture(options.target).context("Capture task failed")?;
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
    let format = ImageExportFormat::from_path(options.destination)?;
    let shared_png = if options.upload.is_some() && format == ImageExportFormat::Png {
        let png = frame.png_bytes()?;
        app_core::save_png_bytes(options.destination, &png)?;
        Some(png)
    } else {
        frame.save_image_as(
            options.destination,
            options.jpeg_quality,
            ExportSize::Original,
        )?;
        None
    };
    let mut result = serde_json::json!({"format":format.extension(), "width":frame.width, "height":frame.height,
        "steps":steps, "delay_ms":options.delay.as_millis()});
    if let Some(destination) = options.upload {
        let png = match shared_png {
            Some(png) => png,
            None => frame.png_bytes().context(
                "Capture task output was saved; upload preparation failed before any request",
            )?,
        };
        drop(frame);
        let outcome = upload(&destination, png).context("Capture task output was saved; upload failed and a remote file may exist; no retry was attempted")?;
        result["upload"] = serde_json::json!({"url":outcome.url, "thumbnail":outcome.thumbnail, "deletion":outcome.deletion});
    }
    Ok(result)
}

pub fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    let result = execute(
        options,
        |delay| {
            std::thread::sleep(delay);
            Ok(())
        },
        |target| {
            let backend = XcapBackend::new(std::path::PathBuf::new());
            Ok(match target {
                Target::Display(id) => backend.capture_display(id)?,
                Target::Window(id) => backend.capture_window(id)?,
                Target::Region { display, area } => backend.capture_region(
                    display,
                    area.x as i32,
                    area.y as i32,
                    area.width,
                    area.height,
                )?,
            })
        },
    )?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{result}")
        .and_then(|_| stdout.flush())
        .context(if result.get("upload").is_some() {
            "Capture task output was saved and uploaded, but its result could not be written; no retry was attempted"
        } else {
            "Capture task output was saved, but its result could not be written"
        })?;
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
                assert_eq!(id, Target::Display("fixture"));
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

    #[test]
    fn display_window_and_region_targets_route_after_validation() {
        for (kind, expected) in [
            ("display", Target::Display("fixture")),
            ("window", Target::Window("fixture")),
        ] {
            let fixture = Fixture::new();
            let mut args = fixture.args("final.png", &[]);
            args[0] = kind.into();
            execute(
                options(&args).unwrap(),
                |_| panic!("no delay"),
                |target| {
                    assert_eq!(target, expected);
                    Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([1, 2, 3, 255]),
                    )))
                },
            )
            .unwrap();
        }
        let fixture = Fixture::new();
        let args = vec![
            "region".into(),
            "fixture".into(),
            "2".into(),
            "3".into(),
            "4".into(),
            "5".into(),
            fixture.0.join("region.png").into_os_string(),
        ];
        execute(
            options(&args).unwrap(),
            |_| panic!("no delay"),
            |target| {
                assert_eq!(
                    target,
                    Target::Region {
                        display: "fixture",
                        area: app_core::PixelRegion {
                            x: 2,
                            y: 3,
                            width: 4,
                            height: 5
                        }
                    }
                );
                Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                    4,
                    5,
                    image::Rgba([1, 2, 3, 255]),
                )))
            },
        )
        .unwrap();
    }

    fn upload_profile(fixture: &Fixture) -> OsString {
        let path = fixture.0.join("upload.json");
        std::fs::write(
            &path,
            br#"{"url":"https://example.invalid/upload","json_pointer":"/url"}"#,
        )
        .unwrap();
        path.into_os_string()
    }

    #[test]
    fn explicit_upload_receives_only_finalized_png_after_local_save_and_pixel_release() {
        let fixture = Fixture::new();
        let profile = upload_profile(&fixture);
        let recipe = fixture
            .recipe(br#"{"version":1,"steps":[{"op":"redact","x":0,"y":0,"width":1,"height":1}]}"#);
        let mut args = fixture.args("final.jpg", &["--delay-ms", "10"]);
        args.extend([
            "--upload".into(),
            profile.clone(),
            "--recipe".into(),
            recipe,
        ]);
        let parsed = options(&args).unwrap();
        let waiting = Cell::new(false);
        let weak = RefCell::new(None);
        let result = execute_with_upload(
            parsed,
            |_| {
                waiting.set(true);
                std::fs::write(Path::new(&profile), b"changed after validation").unwrap();
                Ok(())
            },
            |_| {
                assert!(waiting.get());
                let frame = crate::cli::frame(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([240, 10, 20, 255]),
                ));
                *weak.borrow_mut() = Some(Arc::downgrade(&frame.pixels));
                Ok(frame)
            },
            |destination, png| {
                assert!(fixture.0.join("final.jpg").is_file());
                assert!(weak.borrow().as_ref().unwrap().upgrade().is_none());
                assert!(
                    serde_json::to_value(destination).unwrap()["url"]
                        .as_str()
                        .unwrap()
                        .contains("example.invalid")
                );
                assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
                let image = image::load_from_memory(&png).unwrap().into_rgba8();
                assert_eq!(*image.get_pixel(0, 0), image::Rgba([0, 0, 0, 255]));
                assert_eq!(*image.get_pixel(1, 0), image::Rgba([240, 10, 20, 255]));
                Ok(UploadOutcome {
                    url: "https://example.invalid/final".into(),
                    thumbnail: Some("https://example.invalid/thumb".into()),
                    deletion: Some("https://example.invalid/delete".into()),
                })
            },
        )
        .unwrap();
        assert_eq!(result["upload"]["url"], "https://example.invalid/final");
        assert_eq!(
            result["upload"]["thumbnail"],
            "https://example.invalid/thumb"
        );
        assert_eq!(
            result["upload"]["deletion"],
            "https://example.invalid/delete"
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 3);
    }

    #[test]
    fn upload_failure_keeps_saved_output_and_save_failure_never_uploads() {
        for fail_save in [false, true] {
            let fixture = Fixture::new();
            let profile = upload_profile(&fixture);
            let mut args = fixture.args("final.png", &[]);
            args.extend(["--upload".into(), profile]);
            let uploads = Cell::new(0);
            let result = execute_with_upload(
                options(&args).unwrap(),
                |_| panic!("no delay"),
                |_| {
                    if fail_save {
                        std::fs::write(fixture.0.join("final.png"), b"preserve").unwrap();
                    }
                    Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([1, 2, 3, 255]),
                    )))
                },
                |_, _| {
                    uploads.set(uploads.get() + 1);
                    bail!("fixture response failure")
                },
            );
            let error = result.unwrap_err();
            assert!(fixture.0.join("final.png").is_file());
            if fail_save {
                assert_eq!(uploads.get(), 0);
                assert_eq!(
                    std::fs::read(fixture.0.join("final.png")).unwrap(),
                    b"preserve"
                );
            } else {
                assert_eq!(uploads.get(), 1);
                assert!(
                    error
                        .to_string()
                        .contains("output was saved; upload failed and a remote file may exist")
                );
            }
        }
    }

    #[test]
    fn png_upload_reuses_the_exact_published_png_bytes() {
        let fixture = Fixture::new();
        let profile = upload_profile(&fixture);
        let mut args = fixture.args("final.png", &[]);
        args.extend(["--upload".into(), profile]);
        let result = execute_with_upload(
            options(&args).unwrap(),
            |_| panic!("no delay"),
            |_| {
                Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([25, 50, 75, 100]),
                )))
            },
            |_, png| {
                assert_eq!(png, std::fs::read(fixture.0.join("final.png")).unwrap());
                Ok(UploadOutcome {
                    url: "https://example.invalid/final".into(),
                    thumbnail: None,
                    deletion: None,
                })
            },
        )
        .unwrap();
        assert_eq!(result["upload"]["url"], "https://example.invalid/final");
    }

    #[test]
    fn real_loopback_upload_receives_finalized_saved_pixels() {
        use std::io::{BufRead, Read};
        let fixture = Fixture::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let profile = fixture.0.join("loopback.json");
        std::fs::write(
            &profile,
            serde_json::to_vec(&serde_json::json!({
                "url":format!("http://{}/upload", listener.local_addr().unwrap()),
                "allow_http":true, "json_pointer":"/url"
            }))
            .unwrap(),
        )
        .unwrap();
        let output = fixture.0.join("final.png");
        let saved_path = output.clone();
        let server = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(5) =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("loopback accept: {error}"),
                }
            };
            assert!(saved_path.is_file(), "upload must follow local publication");
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut socket);
            let mut length = None;
            let mut header_bytes = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                header_bytes += line.len();
                assert!(header_bytes <= 16 * 1024);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let length = length.unwrap();
            assert!(length < 64 * 1024);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            drop(reader);
            let png = std::fs::read(saved_path).unwrap();
            assert!(body.windows(png.len()).any(|bytes| bytes == png));
            let response = br#"{"url":"https://example.invalid/final"}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .unwrap();
            socket.write_all(response).unwrap();
        });
        let recipe = fixture
            .recipe(br#"{"version":1,"steps":[{"op":"redact","x":0,"y":0,"width":1,"height":1}]}"#);
        let mut args = fixture.args("final.png", &[]);
        args.extend([
            "--upload".into(),
            profile.into_os_string(),
            "--recipe".into(),
            recipe,
        ]);
        let result = execute(
            options(&args).unwrap(),
            |_| panic!("no delay"),
            |_| {
                Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([20, 40, 60, 255]),
                )))
            },
        );
        server.join().unwrap();
        assert_eq!(
            result.unwrap()["upload"]["url"],
            "https://example.invalid/final"
        );
        assert_eq!(
            *image::open(output).unwrap().into_rgba8().get_pixel(0, 0),
            image::Rgba([0, 0, 0, 255])
        );
    }
}
