//! Parent-owned publication/encoding shared by fixed-display macOS and portal-selected Linux video.
use super::{Options, OutputFormat};
use anyhow::{Context, Result, ensure};
use app_core::CaptureError;
#[cfg(target_os = "macos")]
use app_core::{MacVideoRequest, PixelRegion};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    process::Command,
    time::{Duration, Instant},
};

pub(super) fn record(options: Options<'_>, args: Vec<OsString>) -> Result<()> {
    let cancellation = super::unix::Cancellation::new()?;
    let deadline = Instant::now()
        + Duration::from_secs(
            u64::from(options.seconds)
                + if options.mode.starts_with("portal-") {
                    150
                } else {
                    10
                },
        );
    let destination = std::path::absolute(options.destination)?;
    let parent = destination
        .parent()
        .context("Recording output has no parent")?;
    let staging = super::unix::Staging::new(parent, options.format)?;
    let stage = staging.file.clone();
    #[cfg(target_os = "macos")]
    let request = app_core::NativeCaptureCall::MacVideo {
        request: MacVideoRequest {
            display: options
                .display
                .context("Native screen recording requires --display")?,
            region: PixelRegion {
                x: options.x,
                y: options.y,
                width: options.width,
                height: options.height,
            },
            seconds: options.seconds,
            fps: options.fps,
            cursor: options.cursor.unwrap_or(false),
            microphone: options
                .audio
                .is_some_and(|audio| audio.mode == "avfoundation"),
        },
    };
    #[cfg(target_os = "linux")]
    let request = app_core::NativeCaptureCall::PortalVideo {
        request: app_core::PortalVideoRequest {
            stream: app_core::PortalStreamSource {
                source: match options.mode {
                    "portal-display" => app_core::PortalCaptureSource::Display,
                    "portal-window" => app_core::PortalCaptureSource::Window,
                    _ => anyhow::bail!("Native source unavailable"),
                },
                cursor: if options.cursor.unwrap_or(false) {
                    app_core::PortalCaptureCursor::Include
                } else {
                    app_core::PortalCaptureCursor::Exclude
                },
                width: options.width,
                height: options.height,
            },
            seconds: options.seconds,
            fps: options.fps,
        },
    };
    request.validate()?;
    let peers: crate::capture_worker::video::Peers = Box::new(
        move |group, input, flag, deadline, ready| {
            let result = (|| -> Result<()> {
                let options = super::options(&args)?;
                let cancellation = super::unix::Cancellation::with_flag(flag)?;
                let mut capabilities = Command::new("ffmpeg");
                capabilities.args([
                    "-hide_banner",
                    "-h",
                    &format!("encoder={}", options.format.codec()),
                ]);
                let caps = super::unix::borrowed_group_engine(
                    capabilities,
                    true,
                    &cancellation,
                    deadline,
                    None,
                    super::unix::BorrowedGroup {
                        group,
                        stdin: None,
                        ready: None,
                    },
                )?;
                ensure!(
                    std::str::from_utf8(&caps).ok().is_some_and(
                        |text| text.contains(&format!("Encoder {} ", options.format.codec()))
                    ),
                    "Installed FFmpeg does not advertise the requested encoder"
                );
                let mut probe_version = Command::new("ffprobe");
                probe_version.arg("-version");
                let caps = super::unix::borrowed_group_engine(
                    probe_version,
                    true,
                    &cancellation,
                    deadline,
                    None,
                    super::unix::BorrowedGroup {
                        group,
                        stdin: None,
                        ready: None,
                    },
                )?;
                ensure!(
                    std::str::from_utf8(&caps)
                        .ok()
                        .is_some_and(|text| text.starts_with("ffprobe version ")),
                    "Installed ffprobe is unavailable/unrecognized"
                );
                super::unix::borrowed_group_engine(
                    super::unix::raw_encoder(&options, &stage),
                    false,
                    &cancellation,
                    deadline,
                    None,
                    super::unix::BorrowedGroup {
                        group,
                        stdin: Some(input),
                        ready: Some(ready),
                    },
                )?;
                let metadata = stage.symlink_metadata()?;
                ensure!(
                    metadata.is_file()
                        && metadata.len() >= 6
                        && metadata.len() <= 256 * 1024 * 1024,
                    "Encoded recording exceeds its bound or is not a regular file"
                );
                let mut file = File::open(&stage)?;
                let mut header = [0u8; 6];
                file.read_exact(&mut header)?;
                ensure!(
                    match options.format {
                        OutputFormat::Matroska => header[..4] == [0x1a, 0x45, 0xdf, 0xa3],
                        OutputFormat::Gif => &header == b"GIF87a" || &header == b"GIF89a",
                    },
                    "Recording header does not match requested format"
                );
                let mut probe = Command::new("ffprobe");
                probe.args(["-hide_banner","-loglevel","error","-max_alloc","67108864","-threads","2","-protocol_whitelist","file","-f",options.format.container(),"-max_streams",if options.audio.is_some(){"2"}else{"1"},"-count_frames","-show_entries",
                if options.audio.is_some(){"stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration,sample_rate,channels,bits_per_sample:format=format_name"}else{"stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration:format=format_name"},"-of","json","-i"]).arg(&stage);
                let result = super::unix::borrowed_group_engine(
                    probe,
                    true,
                    &cancellation,
                    deadline,
                    None,
                    super::unix::BorrowedGroup {
                        group,
                        stdin: None,
                        ready: None,
                    },
                )?;
                super::verify_probe(&result, &options)?;
                if options.audio.is_some() {
                    let probe = super::audio_sample_probe(&stage, false);
                    let result = super::unix::borrowed_group_engine(
                        probe,
                        true,
                        &cancellation,
                        deadline,
                        None,
                        super::unix::BorrowedGroup {
                            group,
                            stdin: None,
                            ready: None,
                        },
                    )?;
                    super::verify_audio_samples(&result, options.seconds)?;
                }
                Ok(())
            })();
            result.map_err(|_| {
                CaptureError::Backend(
                    "Native recording encoder/probe failed; no recording was published".into(),
                )
            })
        },
    );
    #[cfg(target_os = "macos")]
    let plan = {
        let app_core::NativeCaptureCall::MacVideo { request } = request.clone() else {
            unreachable!()
        };
        crate::capture_worker::video::Plan::new(request, peers)?
    };
    #[cfg(target_os = "linux")]
    let plan = {
        let app_core::NativeCaptureCall::PortalVideo { request } = request.clone() else {
            unreachable!()
        };
        crate::capture_worker::video::Plan::portal(request, peers)?
    };
    let result = (|| -> Result<_> {
        let check = || {
            if Instant::now() >= deadline {
                return Err(CaptureError::NativeTimeout);
            }
            cancellation
                .check(deadline)
                .map_err(|_| CaptureError::NativeCanceled)
        };
        #[cfg(target_os = "macos")]
        let summary = crate::capture_worker::video::execute(plan, &check)?;
        #[cfg(target_os = "linux")]
        let summary = crate::capture_worker::video::execute_portal(plan, &check)?;
        #[cfg(target_os = "macos")]
        {
            let app_core::NativeCaptureCall::MacVideo { request } = request.clone() else {
                unreachable!()
            };
            summary.validate(request)?;
        }
        #[cfg(target_os = "linux")]
        {
            let app_core::NativeCaptureCall::PortalVideo { request } = request.clone() else {
                unreachable!()
            };
            summary.validate(request)?;
        }
        let metadata = staging.file.symlink_metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 256 * 1024 * 1024,
            "Recording staging changed after verification"
        );
        File::open(&staging.file)?.sync_all()?;
        cancellation.check(deadline)?;
        std::fs::hard_link(&staging.file, &destination)
            .context("Could not publish recording without replacing existing file")?;
        File::open(parent)?
            .sync_all()
            .context("Recording was published, but directory durability could not be confirmed")?;
        let mut report = options.summary(metadata.len());
        #[cfg(target_os = "macos")]
        {
            let app_core::NativeCaptureCall::MacVideo { request } = request.clone() else {
                unreachable!()
            };
            report["display_id"] = request.display.into();
            report["native_source_frames"] = summary.frames.into();
            report["native_source_span_ms"] = (summary.last_pts_ns - summary.first_pts_ns)
                .div_ceil(1_000_000)
                .into();
        }
        #[cfg(target_os = "linux")]
        {
            let app_core::NativeCaptureCall::PortalVideo { request } = request.clone() else {
                unreachable!()
            };
            report["source"] = request.stream.source.identifier().into();
            report["decoded_source_frames"] = summary.decoded_frames.into();
            report["repeated_frames"] = summary.repeated_frames.into();
            report["sample_span_ms"] = summary.sample_span_ns.div_ceil(1_000_000).into();
        }
        Ok(report)
    })();
    let cleanup = staging.cleanup();
    let report = match (result, cleanup) {
        (Ok(report), Ok(())) => report,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(error.context("Recording was published, but staging cleanup failed"));
        }
        (Err(error), Err(_)) => {
            return Err(error.context("Recording failed and private staging cleanup also failed"));
        }
    };
    cancellation.output_phase(true);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{report}")
        .and_then(|_| stdout.flush())
        .context("Recording was published, but its result could not be written")?;
    Ok(())
}
