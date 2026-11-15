//! Finite explicit still captures, with one frame owned at a time.

use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, CapturedImage, ExportSize, ImageExportFormat, XcapBackend};
use chrono::{DateTime, SecondsFormat, Utc};
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};

const MAX_START_DELAY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

struct ScheduledStart {
    at: DateTime<Utc>,
    deadline: Instant,
}

struct Options<'a> {
    display_id: &'a str,
    directory: &'a Path,
    count: u16,
    interval: Duration,
    start: Option<ScheduledStart>,
    format: Option<ImageExportFormat>,
    jpeg_quality: u8,
    webp_quality: Option<u8>,
}

fn options(args: &[OsString], now: DateTime<Utc>, monotonic_now: Instant) -> Result<Options<'_>> {
    ensure!(
        args.len() >= 3 && args.len() % 2 == 1 && args[0] == "display",
        "Usage: screenshots capture-sequence display ID DIRECTORY --count N --interval-ms MS [--start-at RFC3339] [--format png|jpeg|webp] [--jpeg-quality 1..100] [--webp-quality 1..100]"
    );
    let display_id = args[1].to_str().context("Display ID must be UTF-8")?;
    ensure!(
        !display_id.is_empty() && display_id.len() <= 64,
        "Display ID must contain 1–64 UTF-8 bytes"
    );
    let mut count = None;
    let mut interval = None;
    let mut start = None;
    let mut format = None;
    let mut jpeg_quality = None;
    let mut webp_quality = None;
    for pair in args[3..].chunks_exact(2) {
        let value = pair[1].to_str().context("Option value must be UTF-8")?;
        match pair[0].to_str() {
            Some("--count") => {
                ensure!(count.is_none(), "Duplicate --count option");
                let value: u16 = value.parse().context("Count must be a whole number")?;
                ensure!((1..=1000).contains(&value), "Count must be 1–1000");
                count = Some(value);
            }
            Some("--interval-ms") => {
                ensure!(interval.is_none(), "Duplicate --interval-ms option");
                let value: u64 = value.parse().context("Interval must be a whole number")?;
                ensure!(
                    (100..=3_600_000).contains(&value),
                    "Interval must be 100–3600000 milliseconds"
                );
                interval = Some(Duration::from_millis(value));
            }
            Some("--start-at") => {
                ensure!(start.is_none(), "Duplicate --start-at option");
                let at = DateTime::parse_from_rfc3339(value)
                    .context("Start time must be RFC 3339 with an explicit timezone (Z or ±HH:MM)")?
                    .with_timezone(&Utc);
                ensure!(
                    !value.ends_with("-00:00"),
                    "Start time must use a known timezone; use Z or +00:00 for UTC"
                );
                // Chrono truncates extra fractional digits, which could cross the admission bound.
                let fraction_digits = value
                    .get(19..)
                    .and_then(|suffix| suffix.strip_prefix('.'))
                    .map_or(0, |fraction| {
                        fraction.bytes().take_while(u8::is_ascii_digit).count()
                    });
                ensure!(
                    fraction_digits <= 9,
                    "Start time supports at most 9 fractional second digits"
                );
                let delay = at.signed_duration_since(now).to_std().context(
                    "Scheduled start must be in the future and no more than 7 days away",
                )?;
                ensure!(
                    !delay.is_zero() && delay <= MAX_START_DELAY,
                    "Scheduled start must be in the future and no more than 7 days away"
                );
                let deadline = monotonic_now
                    .checked_add(delay)
                    .context("Scheduled start exceeds the monotonic clock range")?;
                start = Some(ScheduledStart { at, deadline });
            }
            Some("--format") => {
                ensure!(format.is_none(), "Duplicate --format option");
                format = Some(match value {
                    "png" => ImageExportFormat::Png,
                    "jpeg" => ImageExportFormat::Jpeg,
                    "webp" => ImageExportFormat::WebP,
                    _ => bail!("Format must be png, jpeg or webp"),
                });
            }
            Some("--jpeg-quality") => {
                ensure!(jpeg_quality.is_none(), "Duplicate --jpeg-quality option");
                let value: u8 = value
                    .parse()
                    .context("JPEG quality must be a whole number")?;
                ensure!((1..=100).contains(&value), "JPEG quality must be 1..100");
                jpeg_quality = Some(value);
            }
            Some("--webp-quality") => {
                ensure!(webp_quality.is_none(), "Duplicate --webp-quality option");
                let value: u8 = value
                    .parse()
                    .context("WebP quality must be a whole number")?;
                ensure!((1..=100).contains(&value), "WebP quality must be 1..100");
                webp_quality = Some(value);
            }
            _ => bail!("Unknown capture-sequence option; use --help"),
        }
    }
    let count = count.context("Missing --count option")?;
    let interval = interval.context("Missing --interval-ms option")?;
    ensure!(
        jpeg_quality.is_none() || webp_quality.is_none(),
        "Use only one of --jpeg-quality and --webp-quality"
    );
    ensure!(
        jpeg_quality.is_none() || format == Some(ImageExportFormat::Jpeg),
        "--jpeg-quality requires --format jpeg"
    );
    ensure!(
        webp_quality.is_none() || format == Some(ImageExportFormat::WebP),
        "--webp-quality requires --format webp"
    );
    let directory = Path::new(&args[2]);
    validate_directory(directory)?;
    Ok(Options {
        display_id,
        directory,
        count,
        interval,
        start,
        format,
        jpeg_quality: jpeg_quality.unwrap_or(app_core::DEFAULT_JPEG_QUALITY),
        webp_quality,
    })
}

fn validate_directory(directory: &Path) -> Result<()> {
    ensure!(
        directory.metadata()?.is_dir(),
        "Capture-sequence destination must be an existing empty directory"
    );
    ensure!(
        std::fs::read_dir(directory)?.next().transpose()?.is_none(),
        "Capture-sequence destination must be empty"
    );
    Ok(())
}

fn after_scheduled_start<T>(
    options: &Options<'_>,
    now: Instant,
    wait: impl FnOnce(Duration) -> Result<()>,
    capture: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if let Some(start) = &options.start {
        wait(start.deadline.saturating_duration_since(now))
            .context("Scheduled start wait failed")?;
        // Another process may have filled or removed the destination while we slept.
        validate_directory(options.directory)?;
    }
    capture()
}

struct SequenceFailure {
    files: Vec<String>,
    cause: anyhow::Error,
}

fn run_sequence<T>(
    count: u16,
    interval: Duration,
    mut capture: impl FnMut() -> Result<T>,
    mut save: impl FnMut(T, u16) -> Result<String>,
    mut wait: impl FnMut(Duration) -> Result<()>,
) -> Result<Vec<String>, SequenceFailure> {
    let mut files = Vec::with_capacity(usize::from(count));
    let result = (|| {
        for index in 1..=count {
            let frame = capture().with_context(|| format!("Capture {index} failed"))?;
            // save consumes the only frame; it is dropped before waiting or recapturing.
            let file =
                save(frame, index).with_context(|| format!("Saving capture {index} failed"))?;
            files.push(file);
            if index < count {
                wait(interval).with_context(|| format!("Interval after capture {index} failed"))?;
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(files),
        Err(cause) => Err(SequenceFailure { files, cause }),
    }
}

fn save_frame(
    options: &Options<'_>,
    prefix: &str,
    frame: CapturedImage,
    index: u16,
) -> Result<String> {
    let format = options.format.unwrap_or_default();
    let name = format!("{prefix}-{index:04}.{}", format.extension());
    if let Some(quality) = options.webp_quality {
        app_core::save_lossy_webp(
            &frame.pixels,
            &options.directory.join(&name),
            quality,
            || false,
        )?;
    } else {
        frame.save_image_as(
            &options.directory.join(&name),
            options.jpeg_quality,
            ExportSize::Original,
        )?;
    }
    Ok(name)
}

pub fn run(args: &[OsString]) -> Result<()> {
    let now = Utc::now();
    let options = options(args, now, Instant::now())?;
    let files = after_scheduled_start(
        &options,
        Instant::now(),
        |delay| {
            std::thread::sleep(delay);
            Ok(())
        },
        || {
            let prefix = format!("capture-{}", uuid::Uuid::new_v4());
            let backend = XcapBackend::new(std::path::PathBuf::new());
            run_sequence(
                options.count,
                options.interval,
                || Ok(backend.capture_display(options.display_id)?),
                |frame, index| save_frame(&options, &prefix, frame, index),
                |interval| {
                    std::thread::sleep(interval);
                    Ok(())
                },
            )
            .map_err(|failure| {
                failure.cause.context(format!(
                    "Capture sequence stopped after {} completed capture(s); saved files matching {prefix}-*.{} remain in {}",
                    failure.files.len(),
                    options.format.unwrap_or_default().extension(),
                    options.directory.display()
                ))
            })
        },
    )?;
    println!("{}", summary(&options, files));
    Ok(())
}

fn summary(options: &Options<'_>, files: Vec<String>) -> serde_json::Value {
    let mut result = serde_json::json!({
        "completed": files.len(),
        "interval_ms": options.interval.as_millis(),
        "directory": options.directory.to_string_lossy(),
        "path_encoding": "lossy-native",
        "files": files,
    });
    if let Some(start) = &options.start {
        result["start_at"] = start.at.to_rfc3339_opts(SecondsFormat::AutoSi, true).into();
    }
    if let Some(format) = options.format {
        result["format"] = serde_json::to_value(format).expect("image format is serializable");
        if format == ImageExportFormat::Jpeg {
            result["jpeg_quality"] = options.jpeg_quality.into();
        }
    }
    if let Some(quality) = options.webp_quality {
        result["webp_quality"] = quality.into();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "screenshots-sequence-schedule-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn args(&self, start: Option<&str>) -> Vec<OsString> {
            let mut args = vec![
                "display".into(),
                "fixture".into(),
                self.0.as_os_str().to_owned(),
                "--count".into(),
                "2".into(),
                "--interval-ms".into(),
                "100".into(),
            ];
            if let Some(start) = start {
                args.extend(["--start-at".into(), start.into()]);
            }
            args
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn validation_time() -> DateTime<Utc> {
        "2030-01-01T00:00:00Z".parse().unwrap()
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Event {
        Backend,
        Capture,
        Save(u16),
        Drop,
        Wait,
    }

    struct Frame(Rc<RefCell<Vec<Event>>>);

    impl Drop for Frame {
        fn drop(&mut self) {
            self.0.borrow_mut().push(Event::Drop);
        }
    }

    #[test]
    fn valid_argument_boundaries_and_option_order_do_not_start_capture() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-sequence-options-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        for (count, interval) in [("1", "100"), ("1000", "3600000")] {
            let args = [
                OsString::from("display"),
                OsString::from("x".repeat(64)),
                root.as_os_str().to_owned(),
                OsString::from("--interval-ms"),
                OsString::from(interval),
                OsString::from("--count"),
                OsString::from(count),
            ];
            let parsed = options(&args, Utc::now(), Instant::now()).unwrap();
            assert_eq!(parsed.count, count.parse::<u16>().unwrap());
            assert_eq!(
                parsed.interval,
                Duration::from_millis(interval.parse().unwrap())
            );
            assert_eq!(parsed.directory, root);
        }
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn scheduled_start_accepts_exact_bounds_offsets_and_any_option_order() {
        let directory = Directory::new();
        for (start, delay, normalized) in [
            (
                "2030-01-01T00:00:00.000000001Z",
                Duration::from_nanos(1),
                "2030-01-01T00:00:00.000000001Z",
            ),
            (
                "2030-01-08T00:00:00Z",
                MAX_START_DELAY,
                "2030-01-08T00:00:00Z",
            ),
            (
                "2030-01-08T05:30:00+05:30",
                MAX_START_DELAY,
                "2030-01-08T00:00:00Z",
            ),
            (
                "2029-12-31T16:00:01-08:00",
                Duration::from_secs(1),
                "2030-01-01T00:00:01Z",
            ),
        ] {
            for start_position in [3, 5, 7] {
                let mut args = directory.args(None);
                args.splice(
                    start_position..start_position,
                    ["--start-at".into(), start.into()],
                );
                let now = Instant::now();
                let parsed = options(&args, validation_time(), now).unwrap();
                assert_eq!(parsed.start.as_ref().unwrap().deadline - now, delay);
                assert_eq!(summary(&parsed, vec![])["start_at"], normalized);
            }
        }
    }

    #[test]
    fn malformed_past_and_too_distant_start_times_fail_without_output() {
        let directory = Directory::new();
        for start in [
            "",
            "tomorrow",
            "2030-01-01",
            "2030-01-01T00:00:01",
            "2030-01-01T00:00:01+25:00",
            "2030-01-01T00:00:01-00:00",
            "2030-02-30T00:00:01Z",
            "2030-01-01T00:00:01Zjunk",
            "2030-01-01T00:00:00Z",
            "2029-12-31T23:59:59.999999999Z",
            "2030-01-08T00:00:00.000000001Z",
            "2030-01-08T00:00:00.0000000001Z",
            "2030-01-07T20:00:00-05:00",
            "9999-12-31T23:59:59Z",
        ] {
            let args = directory.args(Some(start));
            assert!(
                options(&args, validation_time(), Instant::now()).is_err(),
                "{start}"
            );
        }
        let mut args = directory.args(Some("2030-01-01T00:00:01Z"));
        args.extend(["--start-at".into(), "2030-01-01T00:00:02Z".into()]);
        let error = options(&args, validation_time(), Instant::now())
            .err()
            .unwrap();
        assert!(error.to_string().contains("Duplicate --start-at"));
        assert!(std::fs::read_dir(&directory.0).unwrap().next().is_none());
    }

    #[test]
    fn scheduled_wait_precedes_backend_and_pixels_and_preserves_frame_lifetime() {
        let directory = Directory::new();
        let args = directory.args(Some("2030-01-01T00:00:01Z"));
        let now = Instant::now();
        let parsed = options(&args, validation_time(), now).unwrap();
        let events = Rc::new(RefCell::new(Vec::new()));
        let files = after_scheduled_start(
            &parsed,
            now,
            |delay| {
                assert_eq!(delay, Duration::from_secs(1));
                assert!(events.borrow().is_empty());
                events.borrow_mut().push(Event::Wait);
                Ok(())
            },
            || {
                assert_eq!(*events.borrow(), [Event::Wait]);
                events.borrow_mut().push(Event::Backend);
                run_sequence(
                    parsed.count,
                    parsed.interval,
                    || {
                        events.borrow_mut().push(Event::Capture);
                        Ok(Frame(events.clone()))
                    },
                    |frame, index| {
                        events.borrow_mut().push(Event::Save(index));
                        drop(frame);
                        Ok(format!("{index}.png"))
                    },
                    |interval| {
                        assert_eq!(interval, Duration::from_millis(100));
                        assert_eq!(events.borrow().last(), Some(&Event::Drop));
                        events.borrow_mut().push(Event::Wait);
                        Ok(())
                    },
                )
                .map_err(|failure| failure.cause)
            },
        )
        .unwrap();
        assert_eq!(files, ["1.png", "2.png"]);
        assert_eq!(
            *events.borrow(),
            [
                Event::Wait,
                Event::Backend,
                Event::Capture,
                Event::Save(1),
                Event::Drop,
                Event::Wait,
                Event::Capture,
                Event::Save(2),
                Event::Drop,
            ]
        );
    }

    #[test]
    fn immediate_start_has_no_initial_wait_or_scheduled_json_field() {
        let directory = Directory::new();
        let args = directory.args(None);
        let parsed = options(&args, validation_time(), Instant::now()).unwrap();
        assert!(parsed.start.is_none());
        let result = after_scheduled_start(
            &parsed,
            Instant::now(),
            |_| panic!("immediate capture must not wait"),
            || Ok(vec!["fixture.png".to_owned()]),
        )
        .unwrap();
        let result = summary(&parsed, result);
        assert!(result.get("start_at").is_none());
        assert!(result.get("format").is_none());
        assert!(result.get("jpeg_quality").is_none());
        assert!(result.get("webp_quality").is_none());
        assert_eq!(result["completed"], 1);
        assert_eq!(result["interval_ms"], 100);
        assert_eq!(result["files"], serde_json::json!(["fixture.png"]));
    }

    #[test]
    fn lossy_webp_sequence_preserves_alpha_and_releases_each_frame_before_wait() {
        let directory = Directory::new();
        let mut args = directory.args(None);
        args.extend([
            "--format".into(),
            "webp".into(),
            "--webp-quality".into(),
            "20".into(),
        ]);
        let parsed = options(&args, validation_time(), Instant::now()).unwrap();
        let pixels = image::RgbaImage::from_fn(32, 24, |x, y| {
            image::Rgba([
                ((x * 31 + y * 11) % 256) as u8,
                ((x * 13 + y * 47) % 256) as u8,
                ((x + y) * 7) as u8,
                [0, 64, 128, 255][x as usize % 4],
            ])
        });
        let previous = RefCell::new(None::<std::sync::Weak<image::RgbaImage>>);
        let files = run_sequence(
            2,
            Duration::from_millis(100),
            || {
                assert!(
                    previous
                        .borrow()
                        .as_ref()
                        .is_none_or(|p| p.upgrade().is_none())
                );
                let frame = crate::cli::frame(pixels.clone());
                *previous.borrow_mut() = Some(Arc::downgrade(&frame.pixels));
                Ok(frame)
            },
            |frame, index| save_frame(&parsed, "fixture", frame, index),
            |_| {
                assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
                Ok(())
            },
        )
        .unwrap_or_else(|e| panic!("{}", e.cause));
        assert_eq!(summary(&parsed, files.clone())["webp_quality"], 20);
        for name in files {
            let decoded = image::open(directory.0.join(name)).unwrap().into_rgba8();
            assert_ne!(decoded, pixels);
            assert!(
                decoded
                    .pixels()
                    .zip(pixels.pixels())
                    .all(|(a, b)| a[3] == b[3])
            );
        }
        for extra in [
            vec!["--webp-quality", "0"],
            vec!["--webp-quality", "101"],
            vec!["--webp-quality", "-1"],
            vec!["--webp-quality", "1.5"],
            vec!["--webp-quality", "20", "--webp-quality", "30"],
            vec!["--webp-quality", "20", "--jpeg-quality", "80"],
        ] {
            let empty = Directory::new();
            let mut args = empty.args(None);
            args.extend(extra.into_iter().map(OsString::from));
            assert!(options(&args, validation_time(), Instant::now()).is_err());
        }
    }

    #[test]
    fn lossy_webp_size_refusal_drops_frame_without_a_partial_file() {
        let directory = Directory::new();
        let mut args = directory.args(None);
        args.extend([
            "--format".into(),
            "webp".into(),
            "--webp-quality".into(),
            "75".into(),
        ]);
        let parsed = options(&args, validation_time(), Instant::now()).unwrap();
        let frame = crate::cli::frame(image::RgbaImage::from_pixel(
            16_384,
            1,
            image::Rgba([1, 2, 3, 255]),
        ));
        let weak = Arc::downgrade(&frame.pixels);
        assert!(
            save_frame(&parsed, "fixture", frame, 1)
                .unwrap_err()
                .to_string()
                .contains("1–16383")
        );
        assert!(weak.upgrade().is_none());
        assert!(std::fs::read_dir(&directory.0).unwrap().next().is_none());
    }

    #[test]
    fn format_and_quality_are_explicit_snapshots_with_strict_validation() {
        let directory = Directory::new();
        for (format, expected) in [
            ("png", ImageExportFormat::Png),
            ("jpeg", ImageExportFormat::Jpeg),
            ("webp", ImageExportFormat::WebP),
        ] {
            let mut args = directory.args(None);
            args.splice(3..3, ["--format".into(), format.into()]);
            let parsed = options(&args, validation_time(), Instant::now()).unwrap();
            assert_eq!(parsed.format, Some(expected));
            assert_eq!(parsed.jpeg_quality, app_core::DEFAULT_JPEG_QUALITY);
            let json = summary(&parsed, vec![]);
            assert_eq!(json["format"], format);
            assert_eq!(
                json.get("jpeg_quality"),
                (expected == ImageExportFormat::Jpeg)
                    .then_some(&serde_json::json!(app_core::DEFAULT_JPEG_QUALITY))
            );
        }
        for quality in [1, 100] {
            let mut args = directory.args(None);
            args.splice(3..3, ["--jpeg-quality".into(), quality.to_string().into()]);
            args.extend(["--format".into(), "jpeg".into()]);
            let parsed = options(&args, validation_time(), Instant::now()).unwrap();
            assert_eq!(parsed.jpeg_quality, quality);
            assert_eq!(summary(&parsed, vec![])["jpeg_quality"], quality);
        }
        for extra in [
            vec!["--format", "jpg"],
            vec!["--format", "PNG"],
            vec!["--format", "gif"],
            vec!["--format", "../png"],
            vec!["--format", ""],
            vec!["--format", "png", "--format", "jpeg"],
            vec!["--jpeg-quality", "0", "--format", "jpeg"],
            vec!["--jpeg-quality", "101", "--format", "jpeg"],
            vec!["--jpeg-quality", "256", "--format", "jpeg"],
            vec!["--jpeg-quality", "-1", "--format", "jpeg"],
            vec!["--jpeg-quality", "1.5", "--format", "jpeg"],
            vec!["--jpeg-quality", "80"],
            vec!["--format", "png", "--jpeg-quality", "80"],
            vec!["--format", "webp", "--jpeg-quality", "80"],
            vec!["--jpeg-quality", "80", "--jpeg-quality", "90"],
        ] {
            let mut args = directory.args(Some("2030-01-01T00:00:01Z"));
            args.extend(extra.iter().map(OsString::from));
            assert!(
                options(&args, validation_time(), Instant::now()).is_err(),
                "{extra:?}"
            );
        }
        assert!(std::fs::read_dir(&directory.0).unwrap().next().is_none());
    }

    #[test]
    fn configured_formats_keep_pixels_dimensions_and_one_frame_lifetime() {
        for (format, extension, expected_format) in [
            ("png", "png", image::ImageFormat::Png),
            ("jpeg", "jpg", image::ImageFormat::Jpeg),
            ("webp", "webp", image::ImageFormat::WebP),
        ] {
            let directory = Directory::new();
            let mut args = directory.args(None);
            args.extend(["--format".into(), format.into()]);
            if format == "jpeg" {
                args.extend(["--jpeg-quality".into(), "91".into()]);
            }
            let parsed = options(&args, validation_time(), Instant::now()).unwrap();
            let previous = RefCell::new(None::<std::sync::Weak<image::RgbaImage>>);
            let mut captured = 0u8;
            let files = run_sequence(
                2,
                parsed.interval,
                || {
                    assert!(
                        previous
                            .borrow()
                            .as_ref()
                            .is_none_or(|p| p.upgrade().is_none())
                    );
                    captured += 1;
                    let frame = crate::cli::frame(image::RgbaImage::from_pixel(
                        3,
                        2,
                        image::Rgba([captured, 42, 170, if format == "jpeg" { 255 } else { 93 }]),
                    ));
                    *previous.borrow_mut() = Some(Arc::downgrade(&frame.pixels));
                    Ok(frame)
                },
                |frame, index| save_frame(&parsed, "fixture", frame, index),
                |_| {
                    assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
                    Ok(())
                },
            )
            .unwrap_or_else(|failure| panic!("{}", failure.cause));
            assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
            assert_eq!(
                files,
                [
                    format!("fixture-0001.{extension}"),
                    format!("fixture-0002.{extension}")
                ]
            );
            for (index, name) in files.iter().enumerate() {
                let bytes = std::fs::read(directory.0.join(name)).unwrap();
                assert_eq!(image::guess_format(&bytes).unwrap(), expected_format);
                let image = image::load_from_memory(&bytes).unwrap().into_rgba8();
                assert_eq!(image.dimensions(), (3, 2));
                if format != "jpeg" {
                    assert_eq!(
                        image,
                        image::RgbaImage::from_pixel(
                            3,
                            2,
                            image::Rgba([index as u8 + 1, 42, 170, 93])
                        )
                    );
                }
                let collision = crate::cli::frame(image::RgbaImage::from_pixel(
                    3,
                    2,
                    image::Rgba([9, 9, 9, 255]),
                ));
                assert!(save_frame(&parsed, "fixture", collision, index as u16 + 1).is_err());
                assert_eq!(std::fs::read(directory.0.join(name)).unwrap(), bytes);
            }
            assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
        }
    }

    #[test]
    fn jpeg_refuses_alpha_before_output_and_releases_failed_frame() {
        let directory = Directory::new();
        let mut args = directory.args(None);
        args.extend(["--format".into(), "jpeg".into()]);
        let parsed = options(&args, validation_time(), Instant::now()).unwrap();
        let frame = crate::cli::frame(image::RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([1, 2, 3, 93]),
        ));
        let pixels = Arc::downgrade(&frame.pixels);
        let error = save_frame(&parsed, "fixture", frame, 1).unwrap_err();
        assert!(error.to_string().contains("transparency"), "{error}");
        assert!(pixels.upgrade().is_none());
        assert!(std::fs::read_dir(&directory.0).unwrap().next().is_none());
    }

    #[test]
    fn configured_quality_reaches_encoder_and_default_png_bytes_stay_identical() {
        let directory = Directory::new();
        let pixels = image::RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([
                ((x * 47 + y * 13) % 256) as u8,
                ((y * 71 + x * 19) % 256) as u8,
                ((x + y) * 31 % 256) as u8,
                255,
            ])
        });
        let args = directory.args(None);
        let parsed = options(&args, validation_time(), Instant::now()).unwrap();
        let frame = crate::cli::frame(pixels.clone());
        frame
            .save_png_as(&directory.0.join("original.png"))
            .unwrap();
        let name = save_frame(&parsed, "default", frame, 1).unwrap();
        assert_eq!(
            std::fs::read(directory.0.join(name)).unwrap(),
            std::fs::read(directory.0.join("original.png")).unwrap()
        );
        std::fs::remove_file(directory.0.join("original.png")).unwrap();
        std::fs::remove_file(directory.0.join("default-0001.png")).unwrap();
        let mut error = Vec::new();
        for quality in [10, 100] {
            let mut args = directory.args(None);
            args.extend([
                "--format".into(),
                "jpeg".into(),
                "--jpeg-quality".into(),
                quality.to_string().into(),
            ]);
            let parsed = options(&args, validation_time(), Instant::now()).unwrap();
            let name =
                save_frame(&parsed, "quality", crate::cli::frame(pixels.clone()), 1).unwrap();
            let decoded = image::open(directory.0.join(&name)).unwrap().into_rgba8();
            error.push(
                decoded
                    .as_raw()
                    .iter()
                    .zip(pixels.as_raw())
                    .map(|(a, b)| u64::from(a.abs_diff(*b)))
                    .sum::<u64>(),
            );
            std::fs::remove_file(directory.0.join(name)).unwrap();
        }
        assert!(error[1] < error[0], "{error:?}");
    }

    #[test]
    fn validation_time_does_not_extend_the_monotonic_deadline() {
        let directory = Directory::new();
        let args = directory.args(Some("2030-01-01T00:00:01Z"));
        let now = Instant::now();
        let parsed = options(&args, validation_time(), now).unwrap();
        for (elapsed, remaining) in [
            (Duration::from_millis(400), Duration::from_millis(600)),
            (Duration::from_secs(2), Duration::ZERO),
        ] {
            after_scheduled_start(
                &parsed,
                now + elapsed,
                |delay| {
                    assert_eq!(delay, remaining);
                    Ok(())
                },
                || Ok(()),
            )
            .unwrap();
        }
    }

    #[test]
    fn failed_wait_or_changed_destination_prevents_backend_and_pixels() {
        for change in ["cancel", "occupied", "missing", "file"] {
            let directory = Directory::new();
            let args = directory.args(Some("2030-01-01T00:00:01Z"));
            let parsed = options(&args, validation_time(), Instant::now()).unwrap();
            let error = after_scheduled_start(
                &parsed,
                Instant::now(),
                |_| {
                    match change {
                        "cancel" => bail!("fixture cancellation"),
                        "occupied" => std::fs::write(directory.0.join("new.png"), b"keep")?,
                        "missing" => std::fs::remove_dir(&directory.0)?,
                        "file" => {
                            std::fs::remove_dir(&directory.0)?;
                            std::fs::write(&directory.0, b"keep")?;
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
                || -> Result<()> { panic!("invalid schedule must not create a native backend") },
            )
            .unwrap_err();
            if change == "cancel" {
                assert!(error.to_string().contains("Scheduled start wait failed"));
            }
            if change == "occupied" {
                assert_eq!(std::fs::read(directory.0.join("new.png")).unwrap(), b"keep");
            }
            if change == "file" {
                assert_eq!(std::fs::read(&directory.0).unwrap(), b"keep");
            }
        }
    }

    #[test]
    fn finite_sequence_drops_each_frame_before_interval_and_next_capture() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let files = run_sequence(
            3,
            Duration::from_millis(123),
            || {
                events.borrow_mut().push(Event::Capture);
                Ok(Frame(events.clone()))
            },
            |frame, index| {
                events.borrow_mut().push(Event::Save(index));
                drop(frame);
                Ok(format!("{index}.png"))
            },
            |interval| {
                assert_eq!(interval, Duration::from_millis(123));
                events.borrow_mut().push(Event::Wait);
                Ok(())
            },
        )
        .unwrap_or_else(|_| panic!("synthetic sequence should succeed"));
        assert_eq!(files, ["1.png", "2.png", "3.png"]);
        assert_eq!(
            *events.borrow(),
            [
                Event::Capture,
                Event::Save(1),
                Event::Drop,
                Event::Wait,
                Event::Capture,
                Event::Save(2),
                Event::Drop,
                Event::Wait,
                Event::Capture,
                Event::Save(3),
                Event::Drop,
            ]
        );
    }

    #[test]
    fn capture_save_and_wait_failures_stop_without_backlog_or_rollback() {
        for stage in ["capture", "save", "wait"] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let captures = std::cell::Cell::new(0);
            let saves = std::cell::Cell::new(0);
            let result = run_sequence(
                3,
                Duration::from_millis(100),
                || {
                    captures.set(captures.get() + 1);
                    events.borrow_mut().push(Event::Capture);
                    ensure!(stage != "capture" || captures.get() != 2, "fixture");
                    Ok(Frame(events.clone()))
                },
                |frame, index| {
                    events.borrow_mut().push(Event::Save(index));
                    ensure!(stage != "save" || index != 2, "fixture");
                    saves.set(saves.get() + 1);
                    drop(frame);
                    Ok(format!("{index}.png"))
                },
                |_| {
                    events.borrow_mut().push(Event::Wait);
                    ensure!(stage != "wait", "fixture cancellation");
                    Ok(())
                },
            );
            let failure = result.err().expect("fixture must fail");
            assert_eq!(failure.files, ["1.png"]);
            assert_eq!(saves.get(), 1);
            assert_eq!(captures.get(), if stage == "wait" { 1 } else { 2 });
            assert!(failure.cause.to_string().contains(match stage {
                "capture" => "Capture 2 failed",
                "save" => "Saving capture 2 failed",
                _ => "Interval after capture 1 failed",
            }));
            let frame_count = events
                .borrow()
                .iter()
                .filter(|event| matches!(event, Event::Drop))
                .count();
            assert_eq!(frame_count, if stage == "save" { 2 } else { 1 });
        }
    }

    #[test]
    fn synthetic_frames_save_exact_pixels_and_release_before_wait() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-sequence-unit-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let previous = RefCell::new(None::<std::sync::Weak<image::RgbaImage>>);
        let mut index = 0;
        let result = run_sequence(
            2,
            Duration::from_millis(100),
            || {
                assert!(
                    previous
                        .borrow()
                        .as_ref()
                        .is_none_or(|p| p.upgrade().is_none())
                );
                index += 1;
                let frame = crate::cli::frame(image::RgbaImage::from_pixel(
                    2,
                    1,
                    image::Rgba([index, 42, 255, 255]),
                ));
                *previous.borrow_mut() = Some(Arc::downgrade(&frame.pixels));
                Ok(frame)
            },
            |frame, index| {
                let name = format!("frame-{index}.png");
                frame.save_png_as(&root.join(&name))?;
                Ok(name)
            },
            |_| {
                assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
                Ok(())
            },
        )
        .unwrap_or_else(|_| panic!("synthetic save should succeed"));
        assert_eq!(result.len(), 2);
        assert!(previous.borrow().as_ref().unwrap().upgrade().is_none());
        for index in 1..=2 {
            assert_eq!(
                image::open(root.join(format!("frame-{index}.png")))
                    .unwrap()
                    .into_rgba8(),
                image::RgbaImage::from_pixel(2, 1, image::Rgba([index, 42, 255, 255]))
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_collision_keeps_completed_images_and_existing_destination_bytes() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-sequence-collision-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("frame-2.png"), b"preexisting bytes").unwrap();
        let captures = std::cell::Cell::new(0);
        let failure = run_sequence(
            3,
            Duration::from_millis(100),
            || {
                captures.set(captures.get() + 1);
                Ok(crate::cli::frame(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([1, 2, 3, 255]),
                )))
            },
            |frame, index| {
                let name = format!("frame-{index}.png");
                frame.save_png_as(&root.join(&name))?;
                Ok(name)
            },
            |_| Ok(()),
        )
        .err()
        .expect("collision must stop sequence");
        assert_eq!(captures.get(), 2);
        assert_eq!(failure.files, ["frame-1.png"]);
        assert_eq!(
            image::open(root.join("frame-1.png"))
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0),
            &image::Rgba([1, 2, 3, 255])
        );
        assert_eq!(
            std::fs::read(root.join("frame-2.png")).unwrap(),
            b"preexisting bytes"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
