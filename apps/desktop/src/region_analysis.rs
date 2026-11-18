//! Explicit, output-free region analysis; no resident or UI initialization.

use anyhow::{Context, Result, ensure};
use app_core::{CapturedImage, DisplayInfo, PixelRegion, XcapBackend};
#[cfg(any(target_os = "linux", test))]
use std::ffi::OsStr;
use std::{ffi::OsString, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Analysis {
    Ocr,
    Qr,
}

struct Options<'a> {
    display: &'a str,
    area: PixelRegion,
    language: &'a str,
}

fn options(args: &[OsString], analysis: Analysis) -> Result<Options<'_>> {
    ensure!(
        args.len() == 5
            || (analysis == Analysis::Ocr && args.len() == 7 && args[5] == "--language"),
        "Usage: screenshots ocr-region DISPLAY X Y WIDTH HEIGHT [--language LANG]; screenshots qr-region DISPLAY X Y WIDTH HEIGHT"
    );
    let display = args[0].to_str().context("Display ID must be UTF-8")?;
    ensure!(
        !display.is_empty() && display.len() <= 64,
        "Display ID must contain 1–64 UTF-8 bytes"
    );
    let numbers = args[1..5]
        .iter()
        .map(|arg| {
            arg.to_str()
                .context("Region coordinates must be UTF-8")?
                .parse::<u32>()
                .context("Region coordinates must be whole nonnegative numbers")
        })
        .collect::<Result<Vec<_>>>()?;
    let area = PixelRegion {
        x: numbers[0],
        y: numbers[1],
        width: numbers[2],
        height: numbers[3],
    };
    app_core::validate_image_size(area.width, area.height)?;
    let limit = match analysis {
        Analysis::Ocr => crate::ocr::MAX_OCR_IMAGE_BYTES,
        Analysis::Qr => crate::qr::MAX_SCAN_RGBA_BYTES,
    };
    ensure!(
        u64::from(area.width) * u64::from(area.height) * 4 <= limit,
        "Analysis region exceeds the 16 MiB RGBA limit"
    );
    ensure!(
        area.x <= i32::MAX as u32
            && area.y <= i32::MAX as u32
            && area.x.checked_add(area.width).is_some()
            && area.y.checked_add(area.height).is_some(),
        "Region coordinates overflow"
    );
    if analysis == Analysis::Qr {
        ensure!(
            area.width <= 4096 && area.height <= 4096,
            "QR scan dimensions must not exceed 4096 pixels per side"
        );
    }
    let language = if args.len() == 7 {
        args[6].to_str().context("OCR language must be UTF-8")?
    } else {
        "eng"
    };
    if analysis == Analysis::Ocr {
        crate::ocr::validate_language(language)?;
    }
    Ok(Options {
        display,
        area,
        language,
    })
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn require_x11(
    display: Option<&OsStr>,
    session: Option<&OsStr>,
    wayland: Option<&OsStr>,
) -> Result<()> {
    ensure!(
        display
            .and_then(OsStr::to_str)
            .is_some_and(|value| !value.trim().is_empty())
            && session != Some(OsStr::new("wayland"))
            && wayland.is_none_or(OsStr::is_empty),
        "Region analysis requires an X11 session; use ocr-portal-region or qr-portal-region for an explicit Wayland selection"
    );
    Ok(())
}

pub(crate) fn validate_geometry(display: &DisplayInfo, area: PixelRegion, x11: bool) -> Result<()> {
    if x11 {
        ensure!(
            display.scale_factor == 1.0 && display.rotation == 0.0,
            "X11 region analysis requires an unrotated display at 100% scale"
        );
    }
    app_core::validate_display_pixel_coordinates(display)?;
    ensure!(
        area.x
            .checked_add(area.width)
            .is_some_and(|end| end <= display.width)
            && area
                .y
                .checked_add(area.height)
                .is_some_and(|end| end <= display.height),
        "Region is outside the display"
    );
    if display.mac_coordinates.is_some() {
        return Ok(());
    }
    let x = i64::from(display.x) + i64::from(area.x);
    let y = i64::from(display.y) + i64::from(area.y);
    ensure!(
        i32::try_from(x).is_ok() && i32::try_from(y).is_ok(),
        "Native region coordinates overflow"
    );
    if x11 {
        ensure!(
            i16::try_from(x).is_ok()
                && i16::try_from(y).is_ok()
                && u16::try_from(area.width).is_ok()
                && u16::try_from(area.height).is_ok(),
            "Region exceeds the X11 coordinate range"
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) struct DpiContext(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

#[cfg(target_os = "windows")]
impl DpiContext {
    pub(crate) fn enter() -> Result<Self> {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        };
        // This headless thread owns no windows; restore its prior context on exit.
        let mut previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            previous =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) };
        }
        ensure!(
            !previous.is_null(),
            "Could not establish native pixel coordinates"
        );
        Ok(Self(previous))
    }
}

#[cfg(target_os = "windows")]
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(self.0);
        }
    }
}

fn capture(
    options: &Options<'_>,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<CapturedImage> {
    check()?;
    #[cfg(target_os = "linux")]
    require_x11(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )?;
    #[cfg(target_os = "windows")]
    let _dpi = DpiContext::enter()?;
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let displays = backend.list_capture_displays_with_cancel(check)?;
    let mut matches = displays
        .iter()
        .filter(|display| display.id == options.display);
    let display = matches.next().context("Analysis display not found")?;
    ensure!(
        matches.next().is_none(),
        "Analysis display identity is ambiguous"
    );
    validate_geometry(display, options.area, cfg!(target_os = "linux"))?;
    let region = app_core::RememberedRegion::from_display(display, options.area)?;
    let mut frames = backend
        .capture_batch_with_cancel(vec![app_core::CaptureRequest::ExactRegion(region)], check)?;
    ensure!(
        frames.len() == 1,
        "Region capture returned the wrong frame count"
    );
    Ok(frames.remove(0))
}

/// Reject unrelated/changed sources before ownership transfer or processing.
fn owned_pixels(frame: CapturedImage, identity: &str, limit: u64) -> Result<image::RgbaImage> {
    ensure!(
        frame.display_id == identity
            && frame.window_id.is_none()
            && frame.display_geometry.is_none()
            && frame.desktop_layout.is_none()
            && frame.source_format == app_core::PixelFormat::Rgba8UnormSrgb,
        "Analysis capture source proof changed"
    );
    app_core::validate_image_size(frame.width, frame.height)?;
    ensure!(
        frame.pixels.dimensions() == (frame.width, frame.height),
        "Captured region dimensions changed"
    );
    let bytes = u64::from(frame.width) * u64::from(frame.height) * 4;
    ensure!(
        bytes <= limit,
        "Analysis source exceeds its RGBA byte limit"
    );
    ensure!(
        frame.pixels.as_raw().len() as u64 == bytes,
        "Captured region backing does not match its dimensions"
    );
    Arc::try_unwrap(frame.pixels)
        .map_err(|_| anyhow::anyhow!("Region capture retained its source pixels"))
}

fn execute_checked(
    options: &Options<'_>,
    capture: impl FnOnce(&Options<'_>) -> Result<CapturedImage>,
    process: impl FnOnce(image::RgbaImage) -> Result<serde_json::Value>,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<serde_json::Value> {
    check()?;
    let frame = capture(options).context("Analysis region capture failed")?;
    check()?;
    ensure!(
        (frame.width, frame.height) == (options.area.width, options.area.height),
        "Captured region dimensions changed"
    );
    let pixels = owned_pixels(frame, options.display, crate::qr::MAX_SCAN_RGBA_BYTES)?;
    check()?;
    let result = process(pixels)?;
    check()?;
    Ok(result)
}

#[cfg(test)]
fn execute(
    options: &Options<'_>,
    capture: impl FnOnce(&Options<'_>) -> Result<CapturedImage>,
    process: impl FnOnce(image::RgbaImage) -> Result<serde_json::Value>,
) -> Result<serde_json::Value> {
    execute_checked(options, capture, process, &|| Ok(()))
}

fn portal_options(
    args: &[OsString],
    analysis: Analysis,
) -> Result<(Options<'_>, app_core::PortalCaptureSource)> {
    ensure!(
        args.len() == 5
            || (analysis == Analysis::Ocr && args.len() == 7 && args[5] == "--language"),
        "Usage: screenshots ocr-portal-region display|window X Y WIDTH HEIGHT [--language LANG]; screenshots qr-portal-region display|window X Y WIDTH HEIGHT"
    );
    let source = match args[0].to_str() {
        Some("display") => app_core::PortalCaptureSource::Display,
        Some("window") => app_core::PortalCaptureSource::Window,
        _ => anyhow::bail!("Portal analysis source must be display or window"),
    };
    Ok((options(args, analysis)?, source))
}

fn execute_portal(
    options: &Options<'_>,
    source: app_core::PortalCaptureSource,
    capture: impl FnOnce() -> Result<CapturedImage>,
    process: impl FnOnce(image::RgbaImage) -> Result<serde_json::Value>,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<serde_json::Value> {
    check()?;
    let frame = capture().context("Analysis portal capture failed")?;
    check()?;
    let pixels = owned_pixels(frame, source.identifier(), app_core::MAX_CAPTURE_BYTES)?;
    let area = options.area;
    ensure!(
        area.x
            .checked_add(area.width)
            .is_some_and(|end| end <= pixels.width())
            && area
                .y
                .checked_add(area.height)
                .is_some_and(|end| end <= pixels.height()),
        "Analysis region is outside the selected portal raster"
    );
    // Portal selection has no physical monitor binding: crop only its verified natural raster.
    let pixels = if (area.x, area.y, area.width, area.height)
        == (0, 0, pixels.width(), pixels.height())
    {
        pixels
    } else {
        let cropped =
            image::imageops::crop_imm(&pixels, area.x, area.y, area.width, area.height).to_image();
        drop(pixels);
        cropped
    };
    check()?;
    let result = process(pixels)?;
    check()?;
    Ok(result)
}

fn run_source(args: &[OsString], analysis: Analysis, portal: bool) -> Result<()> {
    let (options, source) = if portal {
        let (options, source) = portal_options(args, analysis)?;
        (options, Some(source))
    } else {
        (options(args, analysis)?, None)
    };
    #[cfg(not(target_os = "linux"))]
    ensure!(!portal, "Portal analysis requires Linux");
    #[cfg(unix)]
    let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(unix)]
    let _cancellation = crate::record::unix::Cancellation::with_flag(canceled.clone())?;
    let check = || {
        #[cfg(unix)]
        if canceled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(app_core::CaptureError::NativeCanceled);
        }
        Ok(())
    };
    let process = |pixels| match analysis {
        Analysis::Ocr => crate::ocr::recognize_pixels_with_cancel(pixels, options.language, &check),
        Analysis::Qr => crate::qr::scan_pixels(pixels),
    };
    let result = if let Some(source) = source {
        execute_portal(
            &options,
            source,
            || {
                Ok(app_core::capture_portal_with_cancel(
                    source,
                    app_core::PortalCaptureCursor::Exclude,
                    &check,
                )?)
            },
            process,
            &check,
        )?
    } else {
        execute_checked(
            &options,
            |options| capture(options, &check),
            process,
            &check,
        )?
    };
    check()?;
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    #[cfg(unix)]
    _cancellation.output_phase(true);
    writeln!(output, "{result}")
        .and_then(|_| output.flush())
        .context("Analysis completed but its result could not be written")
}

pub(crate) fn run(args: &[OsString], analysis: Analysis) -> Result<()> {
    run_source(args, analysis, false)
}

pub(crate) fn run_portal(args: &[OsString], analysis: Analysis) -> Result<()> {
    run_source(args, analysis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    fn frame(pixels: RgbaImage) -> CapturedImage {
        let mut frame = crate::cli::frame(pixels);
        frame.display_id = "fixture".into();
        frame
    }
    fn display() -> DisplayInfo {
        DisplayInfo {
            mac_coordinates: None,
            id: "fixture".into(),
            name: "test".into(),
            x: -100,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
            rotation: 0.0,
            is_primary: true,
        }
    }

    #[test]
    fn rejects_invalid_arguments_and_admits_exact_limits() {
        for (kind, values) in [
            (Analysis::Qr, vec!["fixture", "0", "0", "4097", "1"]),
            (Analysis::Ocr, vec!["fixture", "0", "0", "2049", "2048"]),
            (Analysis::Qr, vec!["fixture", "0", "0", "0", "1"]),
            (Analysis::Qr, vec!["fixture", "4294967295", "0", "2", "1"]),
            (
                Analysis::Ocr,
                vec!["fixture", "0", "0", "1", "1", "--language", "../eng"],
            ),
            (
                Analysis::Qr,
                vec!["fixture", "0", "0", "1", "1", "--language", "eng"],
            ),
        ] {
            assert!(options(&args(&values), kind).is_err(), "{values:?}");
        }
        assert!(options(&args(&["fixture", "0", "0", "2048", "2048"]), Analysis::Qr).is_ok());
        assert!(options(&args(&["fixture", "0", "0", "65536", "64"]), Analysis::Ocr).is_ok());
    }

    #[test]
    fn session_and_native_coordinate_limits_fail_closed() {
        assert!(require_x11(Some(OsStr::new(":0")), Some(OsStr::new("x11")), None).is_ok());
        for (display, session, wayland) in [
            (None, None, None),
            (Some(OsStr::new(":0")), Some(OsStr::new("wayland")), None),
            (
                Some(OsStr::new(":0")),
                None,
                Some(OsStr::new("custom-socket")),
            ),
        ] {
            assert!(require_x11(display, session, wayland).is_err());
        }
        let area = PixelRegion {
            x: 1,
            y: 2,
            width: 10,
            height: 10,
        };
        assert_eq!(
            validate_geometry(&display(), area, true).is_ok(),
            !cfg!(target_os = "macos")
        );
        let mut info = display();
        info.scale_factor = 2.0;
        assert!(validate_geometry(&info, area, true).is_err());
        info.scale_factor = 1.0;
        info.rotation = 90.0;
        assert!(validate_geometry(&info, area, true).is_err());
        info.rotation = 0.0;
        info.x = 32767;
        assert!(validate_geometry(&info, area, true).is_err());
        assert_eq!(
            validate_geometry(&info, area, false).is_ok(),
            !cfg!(target_os = "macos")
        );
        info.x = i32::MAX;
        assert!(validate_geometry(&info, area, false).is_err());
    }

    #[test]
    fn owned_pixels_move_once_and_inconsistent_or_shared_frames_never_reach_analysis() {
        let args = args(&["fixture", "0", "0", "2", "2"]);
        let options = options(&args, Analysis::Qr).unwrap();
        let pixels = RgbaImage::from_pixel(2, 2, Rgba([11, 22, 33, 255]));
        let pointer = pixels.as_ptr();
        let result = execute(
            &options,
            |_| Ok(frame(pixels)),
            |pixels| {
                assert_eq!(pixels.as_ptr(), pointer);
                Ok(serde_json::json!({"fixture": true}))
            },
        )
        .unwrap();
        assert_eq!(result["fixture"], true);
        let mut invalid_frame = frame(RgbaImage::new(2, 2));
        invalid_frame.width = 3;
        assert!(
            execute(
                &options,
                |_| Ok(invalid_frame),
                |_| panic!("invalid frame processed")
            )
            .is_err()
        );
        let shared_frame = frame(RgbaImage::new(2, 2));
        let shared = shared_frame.pixels.clone();
        assert!(
            execute(
                &options,
                |_| Ok(shared_frame),
                |_| panic!("shared frame processed")
            )
            .is_err()
        );
        assert_eq!(Arc::strong_count(&shared), 1);
        let oversized_backing = image::ImageBuffer::from_raw(2, 2, vec![255; 20]).unwrap();
        assert!(
            execute(
                &options,
                |_| Ok(frame(oversized_backing)),
                |_| panic!("extra backing processed")
            )
            .is_err()
        );
    }

    #[test]
    fn captured_qr_pixels_use_the_real_decoder_without_files_or_url_actions() {
        let text = "https://example.invalid/region?x=$(touch-nope)";
        let code = qrcode::QrCode::new(text).unwrap();
        let side = (code.width() as u32 + 8) * 4;
        let pixels = RgbaImage::from_fn(side, side, |x, y| {
            let (x, y) = ((x / 4) as usize, (y / 4) as usize);
            let dark = x >= 4
                && y >= 4
                && x < code.width() + 4
                && y < code.width() + 4
                && code[(x - 4, y - 4)] == qrcode::Color::Dark;
            Rgba(if dark {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            })
        });
        let args = args(&["fixture", "0", "0", &side.to_string(), &side.to_string()]);
        let options = options(&args, Analysis::Qr).unwrap();
        let result = execute(&options, |_| Ok(frame(pixels)), crate::qr::scan_pixels).unwrap();
        assert_eq!(result["codes"][0]["text"], text);
        assert!(
            execute(
                &options,
                |_| Ok(frame(RgbaImage::from_pixel(side, side, Rgba([255; 4])))),
                crate::qr::scan_pixels
            )
            .is_err()
        );
    }
    fn portal_frame(pixels: RgbaImage) -> CapturedImage {
        let mut frame = frame(pixels);
        frame.display_id = "portal:display".into();
        frame
    }

    #[test]
    fn portal_parser_preserves_named_region_meaning_and_bounds() {
        for source in ["display", "window"] {
            let input = args(&[source, "1", "2", "3", "4", "--language", "eng+deu"]);
            let (options, selected) = portal_options(&input, Analysis::Ocr).unwrap();
            assert_eq!(options.area.x, 1);
            assert_eq!(options.language, "eng+deu");
            assert_eq!(selected.identifier(), format!("portal:{source}"));
        }
        for values in [
            vec!["fixture", "0", "0", "1", "1"],
            vec!["display", "0", "0", "4097", "1"],
            vec!["display", "4294967295", "0", "2", "1"],
            vec!["window", "0", "0", "1", "1", "--language", "eng"],
        ] {
            assert!(portal_options(&args(&values), Analysis::Qr).is_err());
        }
        let input = args(&["display", "0", "0", "1", "1", "--language", "../eng"]);
        assert!(portal_options(&input, Analysis::Ocr).is_err());
        assert!(portal_options(&[], Analysis::Ocr).is_err());
        // These remain display identifiers for the original API, never a portal switch.
        assert_eq!(
            options(&args(&["window", "0", "0", "1", "1"]), Analysis::Qr)
                .unwrap()
                .display,
            "window"
        );
    }

    #[test]
    fn portal_crop_uses_exact_natural_pixels_and_full_raster_moves_without_copy() {
        let input = args(&["display", "1", "1", "2", "2"]);
        let (options, source) = portal_options(&input, Analysis::Qr).unwrap();
        let result = execute_portal(
            &options,
            source,
            || {
                Ok(portal_frame(RgbaImage::from_fn(4, 3, |x, y| {
                    Rgba([x as u8, y as u8, 19, 255])
                })))
            },
            |pixels| {
                assert_eq!(pixels.dimensions(), (2, 2));
                assert_eq!(pixels.get_pixel(0, 0).0, [1, 1, 19, 255]);
                assert_eq!(pixels.get_pixel(1, 1).0, [2, 2, 19, 255]);
                Ok(serde_json::json!({"crop": true}))
            },
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(result["crop"], true);
        let input = args(&["display", "0", "0", "4", "3"]);
        let (options, source) = portal_options(&input, Analysis::Qr).unwrap();
        let pixels = RgbaImage::new(4, 3);
        let pointer = pixels.as_ptr();
        execute_portal(
            &options,
            source,
            || Ok(portal_frame(pixels)),
            |pixels| {
                assert_eq!(pixels.as_ptr(), pointer);
                Ok(serde_json::json!({}))
            },
            &|| Ok(()),
        )
        .unwrap();
    }

    #[test]
    fn portal_source_bounds_identity_and_backing_are_required() {
        let input = args(&["display", "1", "1", "2", "2"]);
        let (options, source) = portal_options(&input, Analysis::Qr).unwrap();
        let mut wrong_identity = portal_frame(RgbaImage::new(4, 3));
        wrong_identity.display_id = "portal:window".into();
        let mut wrong_window = portal_frame(RgbaImage::new(4, 3));
        wrong_window.window_id = Some("selected-window".into());
        let mut wrong_geometry = portal_frame(RgbaImage::new(4, 3));
        wrong_geometry.display_geometry = Some(app_core::DisplayGeometry {
            mac_coordinates: None,
            x: 0,
            y: 0,
            width: 4,
            height: 3,
            scale_factor: 1.0,
            rotation: 0.0,
        });
        let shared = portal_frame(RgbaImage::new(4, 3));
        let retained = shared.pixels.clone();
        for frame in [
            wrong_identity,
            wrong_window,
            wrong_geometry,
            shared,
            portal_frame(RgbaImage::new(2, 2)),
            portal_frame(RgbaImage::from_raw(4, 3, vec![0; 52]).unwrap()),
        ] {
            assert!(
                execute_portal(
                    &options,
                    source,
                    || Ok(frame),
                    |_| panic!("Unproved raster reached processor"),
                    &|| Ok(())
                )
                .is_err()
            );
        }
        assert_eq!(Arc::strong_count(&retained), 1);
        assert!(owned_pixels(portal_frame(RgbaImage::new(4, 3)), "portal:display", 47).is_err());
    }

    #[test]
    fn cancellation_prevents_capture_processing_and_late_success() {
        use std::cell::Cell;
        let input = args(&["fixture", "0", "0", "2", "2"]);
        let options = options(&input, Analysis::Qr).unwrap();
        for cancel_at in 0..4 {
            let checks = Cell::new(0);
            let captured = Cell::new(false);
            let processed = Cell::new(false);
            let result = execute_checked(
                &options,
                |_| {
                    captured.set(true);
                    Ok(frame(RgbaImage::new(2, 2)))
                },
                |_| {
                    processed.set(true);
                    Ok(serde_json::json!({"must_not_publish":true}))
                },
                &|| {
                    let call = checks.get();
                    checks.set(call + 1);
                    if call == cancel_at {
                        Err(app_core::CaptureError::NativeCanceled)
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(captured.get(), cancel_at != 0);
            assert_eq!(processed.get(), cancel_at == 3);
        }
        let input = args(&["display", "0", "0", "2", "2"]);
        let (options, source) = portal_options(&input, Analysis::Qr).unwrap();
        for cancel_at in 0..4 {
            let checks = Cell::new(0);
            let result = execute_portal(
                &options,
                source,
                || {
                    assert!(cancel_at != 0);
                    Ok(portal_frame(RgbaImage::new(4, 3)))
                },
                |_| {
                    assert_eq!(cancel_at, 3);
                    Ok(serde_json::json!({}))
                },
                &|| {
                    let call = checks.get();
                    checks.set(call + 1);
                    if call == cancel_at {
                        Err(app_core::CaptureError::NativeCanceled)
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn named_region_source_identity_is_not_replaced_by_equal_sized_pixels() {
        let input = args(&["fixture", "0", "0", "2", "2"]);
        let options = options(&input, Analysis::Qr).unwrap();
        assert!(
            execute(
                &options,
                |_| Ok(portal_frame(RgbaImage::new(2, 2))),
                |_| panic!("Portal substituted for physical display")
            )
            .is_err()
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn calibrated_physical_geometry_keeps_scale_and_rotation_separate_from_x11_policy() {
        let mut display = display();
        display.scale_factor = 1.5;
        display.rotation = 90.0;
        let area = PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert!(validate_geometry(&display, area, false).is_ok());
        assert!(app_core::RememberedRegion::from_display(&display, area).is_ok());
        assert!(validate_geometry(&display, area, true).is_err());
        display.rotation = 45.0;
        assert!(validate_geometry(&display, area, false).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_region_requires_complete_native_raster_proof() {
        let coordinates = app_core::MacDisplayCoordinates {
            display_id: 1,
            display_uuid: [1; 16],
            logical: app_core::MacLogicalRect {
                x: -100.0,
                y: 0.0,
                width: 100.0,
                height: 80.0,
            },
            mode_pixel_width: 160,
            mode_pixel_height: 200,
            rotation: 90.0,
            pixel_width: 200,
            pixel_height: 160,
            pixels_per_point: 2.0,
        };
        let mut display = display();
        display.id = "1".into();
        display.width = 200;
        display.height = 160;
        display.scale_factor = 2.0;
        display.rotation = 90.0;
        display.mac_coordinates = Some(coordinates);
        let area = PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert!(validate_geometry(&display, area, false).is_ok());
        assert!(app_core::RememberedRegion::from_display(&display, area).is_ok());
        display.mac_coordinates.as_mut().unwrap().pixel_width = 160;
        assert!(validate_geometry(&display, area, false).is_err());
        display.mac_coordinates = None;
        assert!(validate_geometry(&display, area, false).is_err());
    }
}
