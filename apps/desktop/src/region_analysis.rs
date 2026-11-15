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
        "Region analysis requires an X11 session; the Wayland capture path is not supported"
    );
    Ok(())
}

pub(crate) fn validate_geometry(display: &DisplayInfo, area: PixelRegion, x11: bool) -> Result<()> {
    ensure!(
        display.scale_factor == 1.0 && display.rotation == 0.0,
        "Region analysis currently requires an unrotated display at 100% scale"
    );
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
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        // This headless thread owns no windows; restore its prior context on exit.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
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

fn capture(options: &Options<'_>) -> Result<CapturedImage> {
    use app_core::CaptureBackend;
    #[cfg(target_os = "linux")]
    require_x11(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )?;
    #[cfg(target_os = "windows")]
    let _dpi = DpiContext::enter()?;
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let display = backend
        .list_displays()?
        .into_iter()
        .find(|display| display.id == options.display)
        .context("Analysis display not found")?;
    validate_geometry(&display, options.area, cfg!(target_os = "linux"))?;
    let region = app_core::RememberedRegion::from_display(&display, options.area)?;
    backend
        .capture_batch(vec![app_core::CaptureRequest::ExactRegion(region)])?
        .pop()
        .context("Region capture returned no image")
}

fn execute(
    options: &Options<'_>,
    capture: impl FnOnce(&Options<'_>) -> Result<CapturedImage>,
    process: impl FnOnce(image::RgbaImage) -> Result<serde_json::Value>,
) -> Result<serde_json::Value> {
    let frame = capture(options).context("Analysis region capture failed")?;
    ensure!(
        frame.width == options.area.width
            && frame.height == options.area.height
            && frame.pixels.dimensions() == (frame.width, frame.height),
        "Captured region dimensions changed"
    );
    ensure!(
        frame.pixels.as_raw().len() as u64 <= crate::qr::MAX_SCAN_RGBA_BYTES,
        "Captured region exceeds the 16 MiB RGBA limit"
    );
    ensure!(
        frame.pixels.as_raw().len() as u64 == u64::from(frame.width) * u64::from(frame.height) * 4,
        "Captured region backing does not match its dimensions"
    );
    let pixels = Arc::try_unwrap(frame.pixels)
        .map_err(|_| anyhow::anyhow!("Region capture retained its source pixels"))?;
    process(pixels)
}

pub(crate) fn run(args: &[OsString], analysis: Analysis) -> Result<()> {
    let options = options(args, analysis)?;
    let result = execute(&options, capture, |pixels| match analysis {
        Analysis::Ocr => crate::ocr::recognize_pixels(pixels, options.language),
        Analysis::Qr => crate::qr::scan_pixels(pixels),
    })?;
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}")
        .and_then(|_| output.flush())
        .context("Analysis completed but its result could not be written")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    fn display() -> DisplayInfo {
        DisplayInfo {
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
        assert!(validate_geometry(&display(), area, true).is_ok());
        let mut info = display();
        info.scale_factor = 2.0;
        assert!(validate_geometry(&info, area, true).is_err());
        info.scale_factor = 1.0;
        info.rotation = 90.0;
        assert!(validate_geometry(&info, area, true).is_err());
        info.rotation = 0.0;
        info.x = 32767;
        assert!(validate_geometry(&info, area, true).is_err());
        assert!(validate_geometry(&info, area, false).is_ok());
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
            |_| Ok(crate::cli::frame(pixels)),
            |pixels| {
                assert_eq!(pixels.as_ptr(), pointer);
                Ok(serde_json::json!({"fixture": true}))
            },
        )
        .unwrap();
        assert_eq!(result["fixture"], true);
        let mut frame = crate::cli::frame(RgbaImage::new(2, 2));
        frame.width = 3;
        assert!(
            execute(
                &options,
                |_| Ok(frame),
                |_| panic!("invalid frame processed")
            )
            .is_err()
        );
        let frame = crate::cli::frame(RgbaImage::new(2, 2));
        let shared = frame.pixels.clone();
        assert!(
            execute(
                &options,
                |_| Ok(frame),
                |_| panic!("shared frame processed")
            )
            .is_err()
        );
        assert_eq!(Arc::strong_count(&shared), 1);
        let oversized_backing = image::ImageBuffer::from_raw(2, 2, vec![255; 20]).unwrap();
        assert!(
            execute(
                &options,
                |_| Ok(crate::cli::frame(oversized_backing)),
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
        let result = execute(
            &options,
            |_| Ok(crate::cli::frame(pixels)),
            crate::qr::scan_pixels,
        )
        .unwrap();
        assert_eq!(result["codes"][0]["text"], text);
        assert!(
            execute(
                &options,
                |_| Ok(crate::cli::frame(RgbaImage::from_pixel(
                    side,
                    side,
                    Rgba([255; 4])
                ))),
                crate::qr::scan_pixels
            )
            .is_err()
        );
    }
}
