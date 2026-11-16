//! Whole finite Mac scroll job; retained AX references never leave its owned helper.
use crate::{
    CaptureError, CaptureOptions, CaptureWindowShadow, CapturedImage, MacScrollRequest,
    MacScrollStop, MacScrollSummary, NativeCaptureCheck, PixelFormat, ScrollAppend,
    ScrollAssembler,
};
use anyhow::{Result, ensure};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) fn run(
    request: MacScrollRequest,
    originating_pid: u32,
) -> std::result::Result<(CapturedImage, MacScrollSummary), CaptureError> {
    request
        .validate()
        .map_err(|_| CaptureError::InvalidRegion)?;
    let result = (|| -> Result<_> {
        let start = Instant::now();
        let until = start + Duration::from_secs(120);
        let events = crate::capture_macos_loop::EventLoop::new()?;
        let _topology = crate::capture_macos_loop::Topology::new(events.wake())?;
        let check = || -> Result<()> {
            crate::capture_macos_loop::Topology::check()?;
            ensure!(
                Instant::now() < until,
                "Native macOS scrolling deadline exceeded"
            );
            Ok(())
        };
        let target =
            crate::capture_macos_ax::Target::new(request.window, request.region, originating_pid)?;
        let options = CaptureOptions {
            window_shadow: CaptureWindowShadow::Exclude,
            ..CaptureOptions::default()
        };
        let source = crate::capture_macos_sck::Window::new(target.proof, until)?;
        let expected = target.proof.bounds.pixel_size(target.scale)?;
        if let Some(source) = &source {
            ensure!(
                (source.scale - target.scale).abs() <= 1.0 / 1024.0
                    && (source.width, source.height) == expected,
                "Native screenshot scale/extent does not match the retained scroll target"
            );
        }
        let capture = || -> Result<image::RgbaImage> {
            check()?;
            target.check()?;
            let pixels = if let Some(source) = &source {
                let image = source.capture(
                    &options,
                    until.min(Instant::now() + Duration::from_secs(20)),
                    u64::from(request.region.width) * u64::from(request.region.height) * 4,
                )?;
                unsafe { crate::capture_macos_image::decode(image.0) }?
            } else {
                crate::capture_macos_image::legacy_window(target.proof, &options)?
            };
            ensure!(
                pixels.dimensions() == expected
                    && pixels.as_raw().len() as u64
                        == u64::from(expected.0) * u64::from(expected.1) * 4,
                "Native scrolling pixels changed extent"
            );
            check()?;
            target.check()?;
            Ok(image::imageops::crop_imm(
                &pixels,
                request.region.x,
                request.region.y,
                request.region.width,
                request.region.height,
            )
            .to_image())
        };
        let mut assembler = ScrollAssembler::new(capture()?, request.matcher())?;
        let mut captured = 1u32;
        let mut actions = 0u32;
        let mut unchanged = 0u32;
        let mut reason = MacScrollStop::StepLimit;
        let mut complete = false;
        for _ in 0..request.steps {
            check()?;
            target.increment(request.wheel, &check)?;
            actions += 1;
            let settle =
                (Instant::now() + Duration::from_millis(u64::from(request.settle_ms))).min(until);
            while Instant::now() < settle {
                check()?;
                std::thread::sleep(
                    settle
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(20)),
                );
            }
            let pixels = capture()?;
            captured += 1;
            match assembler.push(pixels, || Instant::now() >= until)? {
                ScrollAppend::Advanced(_) => unchanged = 0,
                ScrollAppend::Unchanged => {
                    unchanged += 1;
                    if unchanged >= 2 {
                        complete = !assembler.advances().is_empty();
                        reason = if complete {
                            MacScrollStop::Unchanged
                        } else {
                            MacScrollStop::NoScroll
                        };
                        break;
                    }
                }
            }
        }
        check()?;
        target.check()?;
        let summary = MacScrollSummary {
            captured_frames: captured,
            scroll_actions: actions,
            vertical_advances: assembler.advances().to_vec(),
            stop_reason: reason,
            complete,
        };
        let pixels = assembler.finish()?;
        summary.validate(&request, pixels.width(), pixels.height())?;
        let image = CapturedImage {
            display_id: target.display.id.to_string(),
            window_id: Some(request.window.to_string()),
            width: pixels.width(),
            height: pixels.height(),
            png_path: PathBuf::new(),
            pixels: Arc::new(pixels),
            elapsed_ms: start.elapsed().as_millis(),
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        Ok((image, summary))
    })();
    result.map_err(|error|error.downcast::<CaptureError>().unwrap_or_else(|_|CaptureError::Backend("Native macOS scrolling failed; verify exact target focus and supported scrollbar/pixel geometry".into())))
}

/// The public call never initializes native frameworks outside the installed owned executor.
pub fn capture_macos_scroll_with_cancel(
    request: MacScrollRequest,
    check: &NativeCaptureCheck<'_>,
) -> std::result::Result<(CapturedImage, MacScrollSummary), CaptureError> {
    request
        .validate()
        .map_err(|_| CaptureError::InvalidRegion)?;
    check()?;
    let call = crate::NativeCaptureCall::MacScroll { request };
    let executor = crate::native_capture_job::executor().ok_or(CaptureError::Unsupported(
        "Mac scrolling requires the desktop owned native executor",
    ))?;
    let reply = executor(call.clone(), check)?;
    call.validate_reply(&reply)?;
    check()?;
    let crate::NativeCaptureReply::MacScroll { image, summary } = reply else {
        return Err(CaptureError::Backend(
            "Mac scrolling returned an unexpected result".into(),
        ));
    };
    Ok((image, summary))
}
