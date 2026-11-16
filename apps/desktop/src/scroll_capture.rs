//! Explicit finite native scroll capture; no resident work or GPUI startup.

use anyhow::{Context, Result, bail, ensure};
use app_core::{
    CaptureBackend, PixelRegion, ScrollAppend, ScrollAssembler, ScrollMatchOptions, XcapBackend,
};
use std::{
    ffi::OsString,
    io::Write,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
#[path = "scroll_capture_x11.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "scroll_capture_windows.rs"]
mod platform;

type Region = PixelRegion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    pid: u32,
}

struct Options<'a> {
    window: u32,
    destination: &'a Path,
    region: Region,
    steps: u32,
    wheel: u32,
    settle: Duration,
    delay: Duration,
    matcher: ScrollMatchOptions,
}

fn number(arg: &OsString) -> Result<u32> {
    let text = arg.to_str().context("Scrolling numbers must be UTF-8")?;
    ensure!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "Scrolling numbers must be nonnegative whole numbers"
    );
    text.parse().context("Scrolling number overflow")
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    const USAGE: &str = "Usage: screenshots scroll-capture WINDOW OUTPUT.png --region X Y WIDTH HEIGHT [--steps 1..128] [--wheel 1..10] [--settle-ms 20..2000] [--delay-ms 0..10000] [--min-overlap 1..1024] [--tolerance 0..16] [--trim-top N] [--trim-bottom N]";
    ensure!(
        (7..=23).contains(&args.len()) && args[2] == "--region" && (args.len() - 7) % 2 == 0,
        USAGE
    );
    ensure!(
        args[0].as_encoded_bytes().len() <= 20 && args[1].as_encoded_bytes().len() <= 4096,
        "Scrolling ID or output path exceeds its length limit"
    );
    let window = number(&args[0])?;
    ensure!(window > 0, "Scrolling window ID must be positive");
    let destination = Path::new(&args[1]);
    ensure!(
        destination
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png")),
        "Scrolling output must be PNG"
    );
    match destination.symlink_metadata() {
        Ok(_) => bail!("Scrolling output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Scrolling output is unavailable"),
    }
    let region = Region {
        x: number(&args[3])?,
        y: number(&args[4])?,
        width: number(&args[5])?,
        height: number(&args[6])?,
    };
    ensure!(
        region.x.checked_add(region.width).is_some()
            && region.y.checked_add(region.height).is_some(),
        "Scrolling region overflow"
    );
    app_core::validate_image_size(region.width, region.height)?;
    ensure!(
        region.width <= 4096
            && region.height <= 4096
            && u64::from(region.width) * u64::from(region.height) * 4 <= 16 * 1024 * 1024,
        "Scrolling viewport exceeds its 16 MiB or side limit"
    );
    let mut values = [32, 3, 200, 32, 0, 0, 0, 0];
    let mut seen = [false; 8];
    for pair in args[7..].chunks_exact(2) {
        let index = match pair[0].to_str() {
            Some("--steps") => 0,
            Some("--wheel") => 1,
            Some("--settle-ms") => 2,
            Some("--min-overlap") => 3,
            Some("--tolerance") => 4,
            Some("--trim-top") => 5,
            Some("--trim-bottom") => 6,
            Some("--delay-ms") => 7,
            _ => bail!(USAGE),
        };
        ensure!(!seen[index], "Duplicate scrolling option");
        seen[index] = true;
        values[index] = number(&pair[1])?;
    }
    let [
        steps,
        wheel,
        settle,
        minimum_overlap,
        tolerance,
        trim_top,
        trim_bottom,
        delay,
    ] = values;
    ensure!(
        (1..=128).contains(&steps) && (1..=10).contains(&wheel) && (20..=2000).contains(&settle),
        "Scrolling steps, wheel or settle time is out of range"
    );
    ensure!(
        delay <= 10_000 && steps * settle + delay <= 100_000,
        "Requested scrolling waits exceed the 100 second admission limit"
    );
    let height = region
        .height
        .checked_sub(
            trim_top
                .checked_add(trim_bottom)
                .context("Scrolling trim overflow")?,
        )
        .filter(|height| *height >= 2)
        .context("Scrolling trim must leave two rows")?;
    ensure!(
        (1..=1024).contains(&minimum_overlap) && minimum_overlap < height && tolerance <= 16,
        "Scrolling overlap or tolerance is invalid for the region"
    );
    Ok(Options {
        window,
        destination,
        region,
        steps,
        wheel,
        settle: Duration::from_millis(u64::from(settle)),
        delay: Duration::from_millis(u64::from(delay)),
        matcher: ScrollMatchOptions {
            minimum_overlap,
            tolerance: tolerance as u8,
            trim_top,
            trim_bottom,
        },
    })
}

struct Cancellation {
    flag: Arc<AtomicBool>,
    #[cfg(unix)]
    registrations: Vec<signal_hook::SigId>,
}
#[cfg(target_os = "windows")]
static WINDOWS_CANCELED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
unsafe extern "system" fn console_signal(code: u32) -> i32 {
    if matches!(code, 0 | 1 | 2 | 5 | 6) {
        WINDOWS_CANCELED.store(true, Ordering::Relaxed);
        1
    } else {
        0
    }
}
#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}
impl Cancellation {
    fn new() -> Result<Self> {
        let flag = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        {
            let mut registrations = Vec::new();
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                match signal_hook::flag::register(signal, flag.clone()) {
                    Ok(id) => registrations.push(id),
                    Err(error) => {
                        for id in registrations {
                            signal_hook::low_level::unregister(id);
                        }
                        return Err(error).context("Could not install scrolling cancellation");
                    }
                }
            }
            Ok(Self {
                flag,
                registrations,
            })
        }
        #[cfg(target_os = "windows")]
        {
            WINDOWS_CANCELED.store(false, Ordering::Relaxed);
            ensure!(
                unsafe { SetConsoleCtrlHandler(Some(console_signal), 1) } != 0,
                "Could not install scrolling cancellation"
            );
            Ok(Self { flag })
        }
        #[cfg(not(any(unix, target_os = "windows")))]
        Ok(Self { flag })
    }
    fn canceled(&self) -> bool {
        #[cfg(target_os = "windows")]
        if WINDOWS_CANCELED.load(Ordering::Relaxed) {
            return true;
        }
        self.flag.load(Ordering::Relaxed)
    }
    fn check(&self, start: Instant) -> Result<()> {
        ensure!(
            !self.canceled(),
            "Scrolling capture canceled; the target scroll position was not restored"
        );
        ensure!(
            start.elapsed() < Duration::from_secs(120),
            "Scrolling capture exceeded its 120 second operation deadline"
        );
        Ok(())
    }
    fn settle(&self, start: Instant, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.check(start)?;
            std::thread::sleep(
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        self.check(start)
    }
}
impl Drop for Cancellation {
    fn drop(&mut self) {
        #[cfg(unix)]
        for id in self.registrations.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        #[cfg(target_os = "windows")]
        unsafe {
            SetConsoleCtrlHandler(Some(console_signal), 0);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn execute(options: &Options<'_>) -> Result<serde_json::Value> {
    let start = Instant::now();
    let cancel = Cancellation::new()?;
    cancel.settle(start, options.delay)?;
    let native = platform::Native::new(options.window)?;
    let expected = native.geometry()?;
    app_core::validate_image_size(expected.width, expected.height)?;
    ensure!(
        u64::from(expected.width) * u64::from(expected.height) * 4 <= 32 * 1024 * 1024,
        "Scrolling target window exceeds the 32 MiB native frame limit"
    );
    ensure!(
        options.region.x + options.region.width <= expected.width
            && options.region.y + options.region.height <= expected.height,
        "Scrolling region is outside the window"
    );
    let backend = XcapBackend::new(std::path::PathBuf::new());
    let display = backend
        .list_displays()?
        .into_iter()
        .find(|display| {
            i64::from(expected.x) >= i64::from(display.x)
                && i64::from(expected.y) >= i64::from(display.y)
                && i64::from(expected.x) + i64::from(expected.width)
                    <= i64::from(display.x) + i64::from(display.width)
                && i64::from(expected.y) + i64::from(expected.height)
                    <= i64::from(display.y) + i64::from(display.height)
        })
        .context("Scrolling target must fit entirely on one display")?;
    ensure!(
        display.scale_factor == 1.0 && display.rotation == 0.0,
        "Scrolling currently requires one unrotated display at 100% scale"
    );
    let topology = || -> Result<()> {
        let current = backend
            .list_displays()?
            .into_iter()
            .find(|value| value.id == display.id)
            .context("Scrolling display disappeared")?;
        ensure!(
            current.x == display.x
                && current.y == display.y
                && current.width == display.width
                && current.height == display.height
                && current.rotation == display.rotation
                && current.scale_factor == display.scale_factor,
            "Scrolling display topology changed"
        );
        Ok(())
    };
    let capture = || -> Result<image::RgbaImage> {
        cancel.check(start)?;
        ensure!(
            native.geometry()? == expected,
            "Scrolling target geometry or process changed"
        );
        topology()?;
        let frame = backend
            .capture_batch(vec![app_core::CaptureRequest::ForegroundWindow {
                window_id: options.window.to_string(),
                process_id: expected.pid,
            }])?
            .pop()
            .context("Scrolling capture returned no image")?;
        cancel.check(start)?;
        ensure!(
            native.geometry()? == expected
                && frame.window_id.as_deref() == Some(options.window.to_string().as_str())
                && frame.width == expected.width
                && frame.height == expected.height
                && frame.pixels.dimensions() == (expected.width, expected.height)
                && frame.pixels.as_raw().len() as u64
                    == u64::from(expected.width) * u64::from(expected.height) * 4,
            "Scrolling native capture geometry or process changed"
        );
        topology()?;
        let crop = app_core::crop_snapshot(&frame, options.region)?;
        Arc::try_unwrap(crop.pixels)
            .map_err(|_| anyhow::anyhow!("Scrolling crop retained its pixels"))
    };
    let mut assembled = ScrollAssembler::new(capture()?, options.matcher)?;
    let mut captured = 1u32;
    let mut actions = 0u32;
    let mut unchanged = 0;
    let mut stop_reason = "step_limit";
    let mut complete = false;
    for _ in 0..options.steps {
        cancel.check(start)?;
        native.wheel(expected, options.region, options.wheel)?;
        actions += 1;
        cancel.settle(start, options.settle)?;
        let frame = capture()?;
        captured += 1;
        match assembled.push(frame, || {
            cancel.canceled() || start.elapsed() >= Duration::from_secs(120)
        })? {
            ScrollAppend::Unchanged => {
                unchanged += 1;
                if unchanged >= 2 {
                    complete = !assembled.advances().is_empty();
                    stop_reason = if complete { "unchanged" } else { "no_scroll" };
                    break;
                }
            }
            ScrollAppend::Advanced(_) => unchanged = 0,
        }
    }
    let advances = assembled.advances().to_vec();
    let image = assembled.finish()?;
    let (width, height) = image.dimensions();
    cancel.check(start)?;
    ScrollAssembler::save_png(&image, options.destination, || {
        cancel.canceled() || start.elapsed() >= Duration::from_secs(120)
    })?;
    Ok(
        serde_json::json!({ "format":"png", "width":width, "height":height,
        "captured_frames":captured, "scroll_actions":actions, "appended_frames":advances.len(),
        "vertical_advances":advances, "tolerance":options.matcher.tolerance,
        "trim_top":options.matcher.trim_top, "trim_bottom":options.matcher.trim_bottom,
        "stop_reason":stop_reason, "complete":complete }),
    )
}

pub(crate) fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    #[cfg(target_os = "macos")]
    {
        let start = Instant::now();
        let cancel = Cancellation::new()?;
        cancel.settle(start, options.delay)?;
        let request = app_core::MacScrollRequest {
            window: options.window,
            region: options.region,
            steps: options.steps,
            wheel: options.wheel,
            settle_ms: options.settle.as_millis() as u32,
            minimum_overlap: options.matcher.minimum_overlap,
            tolerance: options.matcher.tolerance,
            trim_top: options.matcher.trim_top,
            trim_bottom: options.matcher.trim_bottom,
        };
        let (frame, summary) = app_core::capture_macos_scroll_with_cancel(request, &|| {
            if cancel.canceled() {
                return Err(app_core::CaptureError::NativeCanceled);
            }
            if start.elapsed() >= Duration::from_secs(120) {
                return Err(app_core::CaptureError::NativeTimeout);
            }
            Ok(())
        })?;
        cancel.check(start)?;
        frame.save_image_as_with_cancel(
            options.destination,
            app_core::DEFAULT_JPEG_QUALITY,
            app_core::ExportSize::Original,
            || cancel.canceled() || start.elapsed() >= Duration::from_secs(120),
        )?;
        let result = serde_json::json!({"format":"png","width":frame.width,"height":frame.height,"captured_frames":summary.captured_frames,
            "scroll_actions":summary.scroll_actions,"appended_frames":summary.vertical_advances.len(),"vertical_advances":summary.vertical_advances,
            "tolerance":options.matcher.tolerance,"trim_top":options.matcher.trim_top,"trim_bottom":options.matcher.trim_bottom,"stop_reason":summary.stop_reason,"complete":summary.complete});
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{result}")
            .and_then(|_| stdout.flush())
            .context("Scrolling image was saved but its result could not be written")?;
        return Ok(());
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let result = execute(&options)?;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{result}")
            .and_then(|_| stdout.flush())
            .context("Scrolling image was saved but its result could not be written")
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = options;
        bail!("Native scrolling capture is not available on this platform")
    }
}
