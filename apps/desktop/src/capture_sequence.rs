//! Finite explicit still captures, with one frame owned at a time.

use anyhow::{Context, Result, bail, ensure};
use app_core::{CaptureBackend, XcapBackend};
use std::{ffi::OsString, path::Path, time::Duration};

struct Options<'a> {
    display_id: &'a str,
    directory: &'a Path,
    count: u16,
    interval: Duration,
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(
        args.len() == 7 && args[0] == "display",
        "Usage: screenshots capture-sequence display ID DIRECTORY --count N --interval-ms MS"
    );
    let display_id = args[1].to_str().context("Display ID must be UTF-8")?;
    ensure!(
        !display_id.is_empty() && display_id.len() <= 64,
        "Display ID must contain 1–64 UTF-8 bytes"
    );
    let mut count = None;
    let mut interval = None;
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
            _ => bail!("Unknown capture-sequence option; use --help"),
        }
    }
    let count = count.context("Missing --count option")?;
    let interval = interval.context("Missing --interval-ms option")?;
    let directory = Path::new(&args[2]);
    ensure!(
        directory.metadata()?.is_dir(),
        "Capture-sequence destination must be an existing empty directory"
    );
    ensure!(
        std::fs::read_dir(directory)?.next().transpose()?.is_none(),
        "Capture-sequence destination must be empty"
    );
    Ok(Options {
        display_id,
        directory,
        count,
        interval,
    })
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

pub fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    let prefix = format!("capture-{}", uuid::Uuid::new_v4());
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let files = run_sequence(
        options.count,
        options.interval,
        || Ok(backend.capture_display(options.display_id)?),
        |frame, index| {
            let name = format!("{prefix}-{index:04}.png");
            frame.save_png_as(&options.directory.join(&name))?;
            Ok(name)
        },
        |interval| {
            std::thread::sleep(interval);
            Ok(())
        },
    )
    .map_err(|failure| {
        failure.cause.context(format!(
            "Capture sequence stopped after {} completed capture(s); saved files matching {prefix}-*.png remain in {}",
            failure.files.len(),
            options.directory.display()
        ))
    })?;
    println!(
        "{}",
        serde_json::json!({
            "completed": files.len(),
            "interval_ms": options.interval.as_millis(),
            "directory": options.directory.to_string_lossy(),
            "path_encoding": "lossy-native",
            "files": files,
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Event {
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
            let parsed = options(&args).unwrap();
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
