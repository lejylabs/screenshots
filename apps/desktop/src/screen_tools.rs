//! Explicit one-pixel sampling and bounded physical-coordinate measurement.
use anyhow::{Context, Result, ensure};
use app_core::{
    CaptureError, CaptureRequest, CapturedImage, DisplayInfo, NativeCaptureCheck, XcapBackend,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

const COLOR_USAGE: &str = "Usage: screenshots screen-color DISPLAY X Y [--global] (physical pixels, display-local unless --global)";
const RULER_USAGE: &str =
    "Usage: screenshots ruler X1 Y1 X2 Y2 [--dpi X_DPI Y_DPI] (signed physical pixel coordinates)";

struct Sample<'a> {
    display: &'a str,
    x: i32,
    y: i32,
    global: bool,
}

fn text(value: &OsString) -> Result<&str> {
    value
        .to_str()
        .context("Screen tool arguments must be UTF-8")
}

fn coordinate(value: &OsString) -> Result<i32> {
    let value = text(value)?;
    let digits = value.strip_prefix('-').unwrap_or(value);
    ensure!(
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()),
        "Coordinates must be signed whole numbers within i32"
    );
    value.parse().context("Coordinate exceeds i32")
}

fn sample_options(args: &[OsString]) -> Result<Sample<'_>> {
    ensure!(
        args.len() == 3 || (args.len() == 4 && args[3] == "--global"),
        COLOR_USAGE
    );
    let display = text(&args[0])?;
    ensure!(
        (1..=64).contains(&display.len())
            && display
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b':')),
        "Display ID must contain 1–64 ASCII identity bytes"
    );
    let x = coordinate(&args[1])?;
    let y = coordinate(&args[2])?;
    let global = args.len() == 4;
    ensure!(global || (x >= 0 && y >= 0), COLOR_USAGE);
    Ok(Sample {
        display,
        x,
        y,
        global,
    })
}

fn sample_request(options: &Sample<'_>, display: Option<&DisplayInfo>) -> Result<CaptureRequest> {
    if options.global {
        let display = display.context("Global color sampling requires current display geometry")?;
        ensure!(
            display.id == options.display,
            "Color display identity changed"
        );
        app_core::validate_display_pixel_coordinates(display)?;
        let x = i64::from(options.x) - i64::from(display.x);
        let y = i64::from(options.y) - i64::from(display.y);
        ensure!(
            x >= 0 && y >= 0 && x < i64::from(display.width) && y < i64::from(display.height),
            "Global color coordinate is outside the selected display"
        );
        let area = app_core::PixelRegion {
            x: x as u32,
            y: y as u32,
            width: 1,
            height: 1,
        };
        Ok(CaptureRequest::ExactRegion(
            app_core::RememberedRegion::from_display(display, area)?,
        ))
    } else {
        Ok(CaptureRequest::Region {
            display_id: options.display.to_owned(),
            x: options.x,
            y: options.y,
            width: 1,
            height: 1,
        })
    }
}

fn sample(
    options: &Sample<'_>,
    display: Option<&DisplayInfo>,
    check: &NativeCaptureCheck<'_>,
    capture: impl FnOnce(CaptureRequest, &NativeCaptureCheck<'_>) -> Result<Vec<CapturedImage>>,
) -> Result<Value> {
    check()?;
    let request = sample_request(options, display)?;
    let mut frames = capture(request, check)?;
    check()?;
    ensure!(
        frames.len() == 1,
        "Color capture returned an invalid frame count"
    );
    let frame = frames.pop().context("Color capture returned no frame")?;
    ensure!(
        frame.display_id == options.display
            && frame.window_id.is_none()
            && frame.display_geometry.is_none()
            && frame.width == 1
            && frame.height == 1
            && frame.pixels.dimensions() == (1, 1)
            && frame.pixels.as_raw().len() == 4,
        "Color capture identity, dimensions or backing changed"
    );
    let rgba = frame.pixels.get_pixel(0, 0).0;
    drop(frame);
    check()?;
    Ok(json!({
        "display": options.display, "x": options.x, "y": options.y,
        "coordinate_space": if options.global { "desktop_physical_pixels" } else { "display_local_physical_pixels" }, "rgba": rgba,
        "hex_rgb": format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2]),
        "hex_rgba": format!("#{:02X}{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2], rgba[3]),
        "encoding": "capture_backend_rgba8", "cursor": "excluded"
    }))
}

fn dpi(value: &OsString) -> Result<f64> {
    let value: f64 = text(value)?
        .parse()
        .context("DPI must be a finite positive number")?;
    ensure!(
        value.is_finite() && (0.000_001..=1_000_000.0).contains(&value),
        "DPI must be finite and between 0.000001 and1000000"
    );
    Ok(value)
}

fn measure(args: &[OsString]) -> Result<Value> {
    ensure!(
        args.len() == 4 || (args.len() == 7 && args[4] == "--dpi"),
        RULER_USAGE
    );
    let x1 = coordinate(&args[0])?;
    let y1 = coordinate(&args[1])?;
    let x2 = coordinate(&args[2])?;
    let y2 = coordinate(&args[3])?;
    let dx = i64::from(x2) - i64::from(x1);
    let dy = i64::from(y2) - i64::from(y1);
    let width = dx.unsigned_abs();
    let height = dy.unsigned_abs();
    let distance = (dx as f64).hypot(dy as f64);
    let angle = if dx == 0 && dy == 0 {
        None
    } else {
        Some((dy as f64).atan2(dx as f64).to_degrees())
    };
    let mut result = json!({
        "coordinate_space": "physical_pixel_centers", "from": [x1, y1], "to": [x2, y2],
        "dx_pixels": dx, "dy_pixels": dy, "width_pixels": width, "height_pixels": height,
        "rectangle_area_square_pixels": width * height, "distance_pixels": distance,
        "clockwise_angle_degrees": angle
    });
    if args.len() == 7 {
        let x_dpi = dpi(&args[5])?;
        let y_dpi = dpi(&args[6])?;
        let dx_inches = dx as f64 / x_dpi;
        let dy_inches = dy as f64 / y_dpi;
        let distance_inches = dx_inches.hypot(dy_inches);
        result["physical"] = json!({
            "dpi_source": "explicit_user_input", "dpi": [x_dpi, y_dpi],
            "dx_inches": dx_inches, "dy_inches": dy_inches,
            "distance_inches": distance_inches, "distance_mm": distance_inches * 25.4,
            "rectangle_area_square_mm": (dx_inches * dy_inches).abs() * 25.4 * 25.4
        });
    }
    Ok(result)
}

pub(crate) fn run(args: &[OsString], color: bool) -> Result<()> {
    let result = if color {
        let options = sample_options(args)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let cancel = app_core::NativeCaptureCancellation::default();
        #[cfg(unix)]
        let cancellation = crate::record::unix::Cancellation::new()?;
        let check = || {
            #[cfg(unix)]
            if cancellation.check(deadline).is_err() {
                cancel.cancel();
            }
            if Instant::now() >= deadline {
                cancel.cancel();
                return Err(CaptureError::NativeTimeout);
            }
            cancel.check()
        };
        let backend = XcapBackend::new(PathBuf::new());
        let display = if options.global {
            let mut matches = backend
                .list_displays_with_cancel(&check)?
                .into_iter()
                .filter(|display| display.id == options.display);
            let display = matches.next().context("Color display is unavailable")?;
            ensure!(
                matches.next().is_none(),
                "Color display identity is ambiguous"
            );
            Some(display)
        } else {
            None
        };
        let result = sample(&options, display.as_ref(), &check, |request, check| {
            Ok(backend.capture_batch_with_cancel(vec![request], check)?)
        })?;
        check()?;
        #[cfg(unix)]
        cancellation.output_phase(true);
        result
    } else {
        measure(args)?
    };
    let mut output = std::io::stdout().lock();
    writeln!(output, "{result}").context("Could not write screen tool result")?;
    output.flush().context("Could not flush screen tool result")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::sync::Arc;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn frame() -> CapturedImage {
        let mut frame = crate::cli::frame(RgbaImage::from_pixel(1, 1, Rgba([18, 52, 86, 255])));
        frame.display_id = "fixture".into();
        frame
    }

    #[test]
    fn invalid_sampling_metadata_refuses_before_native_work() {
        for input in [
            vec![],
            vec!["fixture", "0"],
            vec!["fixture", "-1", "0"],
            vec!["fixture", "2147483648", "0"],
            vec!["fixture", "+1", "0"],
            vec!["fixture", "NaN", "0"],
            vec!["", "0", "0"],
            vec!["../host", "0", "0"],
            vec!["fixture\n", "0", "0"],
        ] {
            assert!(sample_options(&args(&input)).is_err(), "{input:?}");
        }
        assert!(sample_options(&args(&["fixture", "2147483647", "0"])).is_ok());
    }

    #[test]
    fn sampling_requests_only_one_pixel_and_releases_pixels_before_result() {
        let input = args(&["fixture", "123", "45"]);
        let options = sample_options(&input).unwrap();
        let frame = frame();
        let pixels = Arc::downgrade(&frame.pixels);
        let result = sample(&options, None, &|| Ok(()), |request, check| {
            check()?;
            match request {
                CaptureRequest::Region {
                    display_id,
                    x,
                    y,
                    width,
                    height,
                } => {
                    assert_eq!(
                        (display_id.as_str(), x, y, width, height),
                        ("fixture", 123, 45, 1, 1)
                    );
                }
                _ => panic!("color command captured a different source"),
            }
            Ok(vec![frame])
        })
        .unwrap();
        assert!(pixels.upgrade().is_none());
        assert_eq!(result["rgba"], json!([18, 52, 86, 255]));
        assert_eq!(result["hex_rgb"], "#123456");
        assert_eq!(result["hex_rgba"], "#123456FF");
    }

    #[test]
    fn inconsistent_frames_and_late_cancellation_never_produce_color() {
        let input = args(&["fixture", "0", "0"]);
        let options = sample_options(&input).unwrap();
        for mutation in 0..7 {
            let mut invalid = frame();
            match mutation {
                0 => invalid.display_id = "other".into(),
                1 => invalid.window_id = Some("1".into()),
                2 => invalid.width = 2,
                3 => invalid.pixels = Arc::new(RgbaImage::new(2, 1)),
                4 => invalid.pixels = Arc::new(RgbaImage::from_raw(1, 1, vec![0; 8]).unwrap()),
                _ => {}
            }
            let frames = match mutation {
                5 => vec![invalid, frame()],
                6 => vec![],
                _ => vec![invalid],
            };
            assert!(sample(&options, None, &|| Ok(()), |_, _| Ok(frames)).is_err());
        }
        let cancel = app_core::NativeCaptureCancellation::default();
        assert!(
            sample(&options, None, &|| cancel.check(), |_, _| {
                cancel.cancel();
                Ok(vec![frame()])
            })
            .is_err()
        );
        assert!(
            sample(
                &options,
                None,
                &|| Err(CaptureError::NativeCanceled),
                |_, _| { panic!("canceled command reached native capture") }
            )
            .is_err()
        );
    }

    #[test]
    fn global_negative_origins_use_checked_local_pixels_and_exact_geometry() {
        let mut display = DisplayInfo {
            id: "fixture".into(),
            name: "test".into(),
            x: -1920,
            y: -1080,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
            rotation: 0.0,
            is_primary: false,
        };
        let input = args(&["fixture", "-1910", "-1060", "--global"]);
        let options = sample_options(&input).unwrap();
        let request = sample_request(&options, Some(&display)).unwrap();
        let CaptureRequest::ExactRegion(region) = request else {
            panic!("Global coordinates need exact geometry")
        };
        assert_eq!(
            region.area(),
            app_core::PixelRegion {
                x: 10,
                y: 20,
                width: 1,
                height: 1
            }
        );
        assert!(sample_request(&options, None).is_err());
        display.id = "replacement".into();
        assert!(sample_request(&options, Some(&display)).is_err());
        display.id = "fixture".into();
        for (x, y) in [
            (-1921, -1080),
            (-1920, -1081),
            (0, -1080),
            (-1920, 0),
            (i32::MAX, i32::MIN),
        ] {
            let options = Sample {
                display: "fixture",
                x,
                y,
                global: true,
            };
            assert!(sample_request(&options, Some(&display)).is_err());
        }
        display.x = i32::MIN;
        display.y = i32::MIN;
        let options = Sample {
            display: "fixture",
            x: i32::MIN,
            y: i32::MIN,
            global: true,
        };
        assert!(sample_request(&options, Some(&display)).is_ok());
        display.x = i32::MAX;
        display.y = i32::MAX;
        assert!(sample_request(&options, Some(&display)).is_err());
        let options = Sample {
            display: "fixture",
            x: i32::MAX,
            y: i32::MAX,
            global: true,
        };
        assert!(sample_request(&options, Some(&display)).is_ok());
        display.scale_factor = f32::NAN;
        assert!(sample_request(&options, Some(&display)).is_err());
    }

    #[test]
    fn ruler_uses_pixel_centers_signed_direction_and_explicit_anisotropic_dpi() {
        let result = measure(&args(&["10", "20", "13", "24"])).unwrap();
        assert_eq!(result["dx_pixels"], 3);
        assert_eq!(result["dy_pixels"], 4);
        assert_eq!(result["distance_pixels"], 5.0);
        assert_eq!(result["rectangle_area_square_pixels"], 12);
        assert!(result.get("physical").is_none());
        let result = measure(&args(&["-100", "-200", "-4", "-8", "--dpi", "96", "192"])).unwrap();
        assert_eq!(result["physical"]["dx_inches"], 1.0);
        assert_eq!(result["physical"]["dy_inches"], 1.0);
        let result = measure(&args(&["10", "20", "10", "20"])).unwrap();
        assert_eq!(result["distance_pixels"], 0.0);
        assert!(result["clockwise_angle_degrees"].is_null());
        let result = measure(&args(&["10", "20", "7", "16"])).unwrap();
        assert_eq!(result["dx_pixels"], -3);
        assert_eq!(result["dy_pixels"], -4);
    }

    #[test]
    fn full_i32_span_and_dpi_bounds_do_not_overflow() {
        let result = measure(&args(&[
            "-2147483648",
            "-2147483648",
            "2147483647",
            "2147483647",
        ]))
        .unwrap();
        assert_eq!(result["width_pixels"], 4_294_967_295u64);
        assert_eq!(
            result["rectangle_area_square_pixels"],
            18_446_744_065_119_617_025u64
        );
        assert!(result["distance_pixels"].as_f64().unwrap().is_finite());
        for value in ["NaN", "inf", "0", "-1", "0.0000001", "1000001"] {
            assert!(measure(&args(&["0", "0", "3", "4", "--dpi", value, "96"])).is_err());
        }
        for values in [
            vec!["0", "0", "3"],
            vec!["0", "0", "3", "4", "--dpi", "96"],
            vec!["0", "0", "3", "4", "--scale", "96", "96"],
            vec!["2147483648", "0", "3", "4"],
        ] {
            assert!(measure(&args(&values)).is_err());
        }
        let result = measure(&args(&[
            "-2147483648",
            "-2147483648",
            "2147483647",
            "2147483647",
            "--dpi",
            "0.000001",
            "1000000",
        ]))
        .unwrap();
        assert!(
            result["physical"]["distance_mm"]
                .as_f64()
                .unwrap()
                .is_finite()
        );
    }
}
