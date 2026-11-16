//! One frozen capture per requested display, followed by sequential PNG crops.

use anyhow::{Context, Result, ensure};
use app_core::{CaptureRequest, CapturedImage, DisplayInfo, PixelRegion};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_REGIONS: usize = 16;

struct Region {
    display: String,
    area: PixelRegion,
}

struct Options {
    directory: PathBuf,
    regions: Vec<Region>,
}

fn directory_is_empty(directory: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Capture-regions destination must be an existing empty directory"
    );
    ensure!(
        std::fs::read_dir(directory)?.next().transpose()?.is_none(),
        "Capture-regions destination must be empty"
    );
    Ok(())
}

fn options(args: &[OsString]) -> Result<Options> {
    ensure!(
        (6..=1 + MAX_REGIONS * 5).contains(&args.len()) && (args.len() - 1).is_multiple_of(5),
        "Usage: screenshots capture-regions DIRECTORY DISPLAY X Y WIDTH HEIGHT [DISPLAY X Y WIDTH HEIGHT]... (1–16 regions)"
    );
    ensure!(
        args[0].as_encoded_bytes().len() <= 4096,
        "Destination path exceeds 4096 bytes"
    );
    let directory = std::path::absolute(&args[0])?
        .components()
        .collect::<PathBuf>();
    ensure!(
        directory.as_os_str().as_encoded_bytes().len() <= 4096,
        "Destination path exceeds 4096 bytes"
    );
    let mut regions = Vec::with_capacity((args.len() - 1) / 5);
    let mut crop_bytes = 0u64;
    for group in args[1..].chunks_exact(5) {
        let display = group[0].to_str().context("Display ID must be UTF-8")?;
        ensure!(
            !display.is_empty() && display.len() <= 64 && !display.chars().any(char::is_control),
            "Display ID must contain 1–64 UTF-8 bytes without control characters"
        );
        let mut numbers = [0; 4];
        for (number, value) in numbers.iter_mut().zip(&group[1..]) {
            let value = value.to_str().context("Region coordinates must be UTF-8")?;
            ensure!(
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
                "Region coordinates must be whole nonnegative numbers"
            );
            *number = value
                .parse::<u32>()
                .context("Region coordinate exceeds u32")?;
        }
        let area = PixelRegion {
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
        crop_bytes += u64::from(area.width) * u64::from(area.height) * 4;
        ensure!(
            crop_bytes <= app_core::MAX_CAPTURE_BYTES,
            "Requested crops exceed the 256 MiB RGBA limit"
        );
        regions.push(Region {
            display: display.into(),
            area,
        });
    }
    directory_is_empty(&directory)?;
    Ok(Options { directory, regions })
}

fn same_geometry(first: &DisplayInfo, second: &DisplayInfo) -> bool {
    first.id == second.id
        && first.x == second.x
        && first.y == second.y
        && first.width == second.width
        && first.height == second.height
        && first.scale_factor == second.scale_factor
        && first.rotation == second.rotation
        && first.mac_coordinates == second.mac_coordinates
}

fn selected_displays(options: &Options, available: &[DisplayInfo]) -> Result<Vec<DisplayInfo>> {
    let mut selected: Vec<DisplayInfo> = Vec::new();
    let mut full_bytes = 0u64;
    for region in &options.regions {
        let mut matches = available
            .iter()
            .filter(|display| display.id == region.display);
        let display = matches.next().context("Requested display was not found")?;
        ensure!(
            matches.next().is_none(),
            "Requested display identity is ambiguous"
        );
        app_core::RememberedRegion::from_display(display, region.area)?;
        crate::region_analysis::validate_geometry(
            display,
            PixelRegion {
                x: 0,
                y: 0,
                width: display.width,
                height: display.height,
            },
            cfg!(target_os = "linux"),
        )?;
        if !selected.iter().any(|known| known.id == display.id) {
            full_bytes += u64::from(display.width) * u64::from(display.height) * 4;
            ensure!(
                full_bytes <= app_core::MAX_CAPTURE_BYTES,
                "Display snapshots exceed the 256 MiB RGBA limit"
            );
            selected.push(display.clone());
        }
    }
    Ok(selected)
}

fn validate_frames(displays: &[DisplayInfo], frames: &[CapturedImage]) -> Result<()> {
    ensure!(
        frames.len() == displays.len(),
        "Capture returned the wrong number of display snapshots"
    );
    let mut bytes = 0u64;
    for (display, frame) in displays.iter().zip(frames) {
        ensure!(
            frame.display_id == display.id
                && frame.window_id.is_none()
                && (frame.width, frame.height) == (display.width, display.height)
                && frame.pixels.dimensions() == (display.width, display.height)
                && frame.source_format == app_core::PixelFormat::Rgba8UnormSrgb,
            "Display snapshot identity or dimensions changed"
        );
        let expected = app_core::DisplayGeometry {
            mac_coordinates: display.mac_coordinates,
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        };
        ensure!(
            frame
                .display_geometry
                .is_none_or(|geometry| geometry == expected)
                && (!cfg!(any(target_os = "windows", target_os = "macos"))
                    || frame.display_geometry.is_some()),
            "Display snapshot geometry changed"
        );
        let expected_bytes = u64::from(display.width) * u64::from(display.height) * 4;
        ensure!(
            frame.pixels.as_raw().len() as u64 == expected_bytes,
            "Display snapshot backing does not match its dimensions"
        );
        bytes += expected_bytes;
        ensure!(
            bytes <= app_core::MAX_CAPTURE_BYTES,
            "Display snapshots exceed the 256 MiB RGBA limit"
        );
    }
    Ok(())
}

fn execute(
    options: &Options,
    mut list: impl FnMut() -> Result<Vec<DisplayInfo>>,
    capture: impl FnOnce(Vec<CaptureRequest>) -> Result<Vec<CapturedImage>>,
    mut save: impl FnMut(CapturedImage, &Path) -> Result<()>,
) -> Result<serde_json::Value> {
    let displays = selected_displays(options, &list()?)?;
    let frames = capture(
        displays
            .iter()
            .map(|display| CaptureRequest::Display(display.id.clone()))
            .collect(),
    )
    .context("Display batch capture failed; no region files were published")?;
    validate_frames(&displays, &frames)?;
    let after = list()?;
    for display in &displays {
        let mut matches = after.iter().filter(|candidate| candidate.id == display.id);
        ensure!(
            matches
                .next()
                .is_some_and(|candidate| same_geometry(display, candidate))
                && matches.next().is_none(),
            "Display geometry changed during batch capture; no region files were published"
        );
    }
    directory_is_empty(&options.directory)?;
    let mut remaining = displays
        .iter()
        .map(|display| {
            options
                .regions
                .iter()
                .filter(|region| region.display == display.id)
                .count()
        })
        .collect::<Vec<_>>();
    let mut frames = frames.into_iter().map(Some).collect::<Vec<_>>();
    let mut files = Vec::with_capacity(options.regions.len());
    for (index, region) in options.regions.iter().enumerate() {
        let result = (|| -> Result<String> {
            let display = displays
                .iter()
                .position(|display| display.id == region.display)
                .context("Region display snapshot is unavailable")?;
            let frame = app_core::crop_snapshot(
                frames[display]
                    .as_ref()
                    .context("Region display snapshot was released")?,
                region.area,
            )?;
            remaining[display] -= 1;
            if remaining[display] == 0 {
                frames[display] = None;
            }
            let name = format!("region-{:02}.png", index + 1);
            save(frame, &options.directory.join(&name))?;
            Ok(name)
        })();
        match result {
            Ok(name) => files.push(name),
            Err(error) => return Err(error.context(format!(
                "Saving region {} of {} failed; {} earlier region file(s) confirmed. The current file may also exist; published files remain. No retry or rollback was attempted.",
                index + 1,
                options.regions.len(),
                files.len()
            ))),
        }
    }
    Ok(serde_json::json!({
        "completed": files.len(), "captured_displays": displays.len(),
        "directory": options.directory.to_string_lossy(), "files": files,
        "path_encoding": "lossy-native"
    }))
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    #[cfg(target_os = "linux")]
    crate::region_analysis::require_x11(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )?;
    #[cfg(target_os = "windows")]
    let _dpi = crate::region_analysis::DpiContext::enter()?;
    let backend = app_core::XcapBackend::new(PathBuf::new());
    let result = execute(
        &options,
        || Ok(backend.list_capture_displays_with_cancel(&|| Ok(()))?),
        |requests| Ok(backend.capture_batch(requests)?),
        |frame, path| Ok(frame.save_png_as(path)?),
    )?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{result}")
        .and_then(|_| stdout.flush())
        .context("All region files were published, but the JSON result could not be written")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        sync::Arc,
    };

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "screenshots-capture-regions-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn args(&self, groups: &[&str]) -> Vec<OsString> {
            std::iter::once(self.0.as_os_str().to_owned())
                .chain(groups.iter().map(OsString::from))
                .collect()
        }
        fn empty(&self) {
            assert_eq!(std::fs::read_dir(&self.0).unwrap().count(), 0);
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn display(id: &str, width: u32, height: u32) -> DisplayInfo {
        DisplayInfo {
            mac_coordinates: if cfg!(target_os = "macos") {
                let logical = app_core::MacLogicalRect {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(width),
                    height: f64::from(height),
                };
                let proof = app_core::MacDisplayProof {
                    id: id.parse().unwrap(),
                    uuid: format!(
                        "00000000-0000-0000-0000-{:012x}",
                        id.parse::<u32>().unwrap()
                    ),
                    logical,
                    pixel_width: width,
                    pixel_height: height,
                    rotation: 0.0,
                };
                Some(
                    app_core::MacDisplayCoordinates::from_filter(proof, logical, logical, 1.0)
                        .unwrap(),
                )
            } else {
                None
            },
            id: id.into(),
            name: id.into(),
            x: 0,
            y: 0,
            width,
            height,
            scale_factor: 1.0,
            rotation: 0.0,
            is_primary: true,
        }
    }

    fn frame(display: &DisplayInfo) -> CapturedImage {
        let pixels = image::RgbaImage::from_fn(display.width, display.height, |x, y| {
            image::Rgba([x as u8, y as u8, 93, 255])
        });
        let mut frame = crate::cli::frame(pixels);
        frame.display_id = display.id.clone();
        frame.display_geometry = Some(app_core::DisplayGeometry {
            mac_coordinates: display.mac_coordinates,
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
            scale_factor: display.scale_factor,
            rotation: display.rotation,
        });
        frame
    }

    #[test]
    fn argument_and_combined_crop_admission_precede_native_work() {
        let dir = Directory::new();
        for groups in [
            vec![],
            vec!["1", "0", "0", "1"],
            vec!["1", "-1", "0", "1", "1"],
            vec!["1", "0", "0", "0", "1"],
            vec!["1", "4294967295", "0", "1", "1"],
            vec!["1", "0", "0", "8192", "4096", "1", "0", "0", "8192", "4097"],
        ] {
            assert!(options(&dir.args(&groups)).is_err());
        }
        assert!(options(&dir.args(&["1", "0", "0", "1", "1"].repeat(17))).is_err());
        let exact =
            options(&dir.args(&["1", "0", "0", "8192", "4096", "1", "0", "0", "8192", "4096"]))
                .unwrap();
        assert_eq!(exact.regions.len(), 2);
        dir.empty();
    }

    #[test]
    fn full_display_admission_counts_distinct_sources_and_checks_all_regions() {
        let dir = Directory::new();
        let parsed = options(&dir.args(&[
            "1", "0", "0", "1", "1", "1", "1", "1", "1", "1", "2", "0", "0", "1", "1",
        ]))
        .unwrap();
        let a = display("1", 8192, 4096);
        let b = display("2", 8192, 4096);
        let selected = selected_displays(&parsed, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(selected.len(), 2);
        let mut too_large = b.clone();
        too_large.height += 1;
        assert!(selected_displays(&parsed, &[a.clone(), too_large]).is_err());
        let mut scaled = b.clone();
        scaled.scale_factor = 2.0;
        assert_eq!(
            selected_displays(&parsed, &[a.clone(), scaled]).is_err(),
            !cfg!(target_os = "windows")
        );
        let mut rotated = b.clone();
        rotated.rotation = 90.0;
        assert_eq!(
            selected_displays(&parsed, &[a.clone(), rotated]).is_err(),
            !cfg!(target_os = "windows")
        );
        assert!(selected_displays(&parsed, &[a.clone(), a.clone(), b]).is_err());
        let bad = options(&dir.args(&["1", "4", "0", "1", "1"])).unwrap();
        assert!(selected_displays(&bad, &[display("1", 4, 4)]).is_err());
        let captures = Cell::new(0);
        assert!(
            execute(
                &parsed,
                || Ok(vec![a.clone()]),
                |_| {
                    captures.set(1);
                    Ok(vec![])
                },
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(captures.get(), 0);
        dir.empty();
    }

    #[test]
    fn captures_each_display_once_then_crops_the_same_frozen_pixels_in_order() {
        let dir = Directory::new();
        let parsed = options(&dir.args(&[
            "1", "1", "1", "2", "2", "2", "0", "0", "2", "1", "1", "2", "1", "1", "2",
        ]))
        .unwrap();
        let displays = vec![display("1", 4, 3), display("2", 3, 2)];
        let lists = Cell::new(0);
        let captures = Cell::new(0);
        let weak = RefCell::new(Vec::new());
        let saved = RefCell::new(Vec::new());
        let result = execute(
            &parsed,
            || {
                lists.set(lists.get() + 1);
                Ok(displays.clone())
            },
            |requests| {
                captures.set(captures.get() + 1);
                assert_eq!(requests.len(), 2);
                assert!(matches!(&requests[0], CaptureRequest::Display(id) if id=="1"));
                assert!(matches!(&requests[1], CaptureRequest::Display(id) if id=="2"));
                let frames = displays.iter().map(frame).collect::<Vec<_>>();
                *weak.borrow_mut() = frames.iter().map(|f| Arc::downgrade(&f.pixels)).collect();
                Ok(frames)
            },
            |crop, path| {
                let index = saved.borrow().len();
                let expected = match index {
                    0 => vec![
                        [1, 1, 93, 255],
                        [2, 1, 93, 255],
                        [1, 2, 93, 255],
                        [2, 2, 93, 255],
                    ],
                    1 => vec![[0, 0, 93, 255], [1, 0, 93, 255]],
                    _ => vec![[2, 1, 93, 255], [2, 2, 93, 255]],
                };
                assert_eq!(
                    crop.pixels.pixels().map(|p| p.0).collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(
                    path.file_name().unwrap(),
                    format!("region-{:02}.png", index + 1).as_str()
                );
                if index == 0 {
                    assert!(weak.borrow()[0].upgrade().is_some());
                }
                if index == 1 {
                    assert!(weak.borrow()[1].upgrade().is_none());
                }
                if index == 2 {
                    assert!(weak.borrow()[0].upgrade().is_none());
                }
                crop.save_png_as(path)?;
                saved.borrow_mut().push(path.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(captures.get(), 1);
        assert_eq!(lists.get(), 2);
        assert_eq!(result["completed"], 3);
        assert_eq!(result["captured_displays"], 2);
        assert_eq!(
            result["files"],
            serde_json::json!(["region-01.png", "region-02.png", "region-03.png"])
        );
    }

    #[test]
    fn capture_frame_topology_and_directory_failures_publish_nothing() {
        let dir = Directory::new();
        let parsed = options(&dir.args(&["1", "0", "0", "1", "1"])).unwrap();
        let a = display("1", 2, 2);
        let saves = Cell::new(0);
        assert!(
            execute(
                &parsed,
                || Ok(vec![a.clone()]),
                |_| anyhow::bail!("synthetic native failure"),
                |_, _| {
                    saves.set(1);
                    Ok(())
                }
            )
            .is_err()
        );
        let mut bad = frame(&a);
        bad.width = 1;
        assert!(
            execute(
                &parsed,
                || Ok(vec![a.clone()]),
                |_| Ok(vec![bad]),
                |_, _| {
                    saves.set(1);
                    Ok(())
                }
            )
            .is_err()
        );
        let calls = Cell::new(0);
        assert!(
            execute(
                &parsed,
                || {
                    calls.set(calls.get() + 1);
                    let mut d = a.clone();
                    if calls.get() == 2 {
                        d.x = 1;
                    }
                    Ok(vec![d])
                },
                |_| Ok(vec![frame(&a)]),
                |_, _| {
                    saves.set(1);
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(saves.get(), 0);
        dir.empty();
        assert!(
            execute(
                &parsed,
                || Ok(vec![a.clone()]),
                |_| {
                    std::fs::write(dir.0.join("foreign"), b"untouched")?;
                    Ok(vec![frame(&a)])
                },
                |_, _| {
                    saves.set(1);
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(saves.get(), 0);
        assert_eq!(std::fs::read(dir.0.join("foreign")).unwrap(), b"untouched");
    }

    #[test]
    fn save_failure_preserves_prior_files_and_exact_completed_count() {
        let dir = Directory::new();
        let parsed = options(&dir.args(&[
            "1", "0", "0", "1", "1", "1", "1", "0", "1", "1", "1", "0", "1", "1", "1",
        ]))
        .unwrap();
        let a = display("1", 2, 2);
        let saves = Cell::new(0);
        let error = execute(
            &parsed,
            || Ok(vec![a.clone()]),
            |_| Ok(vec![frame(&a)]),
            |crop, path| {
                saves.set(saves.get() + 1);
                if saves.get() == 2 {
                    std::fs::write(path, b"foreign")?;
                }
                crop.save_png_as(path)?;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("region 2 of 3 failed; 1 earlier region file(s) confirmed")
        );
        assert_eq!(saves.get(), 2);
        assert!(dir.0.join("region-01.png").is_file());
        assert_eq!(
            std::fs::read(dir.0.join("region-02.png")).unwrap(),
            b"foreign"
        );
        assert!(!dir.0.join("region-03.png").exists());
    }

    #[test]
    fn failure_after_publication_does_not_claim_the_current_file_is_absent() {
        let dir = Directory::new();
        let parsed = options(&dir.args(&["1", "0", "0", "1", "1"])).unwrap();
        let a = display("1", 2, 2);
        let error = execute(
            &parsed,
            || Ok(vec![a.clone()]),
            |_| Ok(vec![frame(&a)]),
            |crop, path| {
                crop.save_png_as(path)?;
                anyhow::bail!("synthetic failure after publication")
            },
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("0 earlier region file(s) confirmed")
        );
        assert!(error.to_string().contains("current file may also exist"));
        assert!(format!("{error:#}").contains("synthetic failure"));
        assert!(dir.0.join("region-01.png").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_rejected_without_touching_its_target() {
        let dir = Directory::new();
        let target = Directory::new();
        let link = dir.0.join("link");
        std::os::unix::fs::symlink(&target.0, &link).unwrap();
        for suffix in ["", "/", "/."] {
            let mut path = link.as_os_str().to_owned();
            path.push(suffix);
            let args = [
                path,
                "1".into(),
                "0".into(),
                "0".into(),
                "1".into(),
                "1".into(),
            ];
            assert!(options(&args).is_err(), "accepted suffix {suffix:?}");
            target.empty();
        }
    }
}
