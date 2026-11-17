use anyhow::{Context, Result, ensure};
use app_core::{
    CaptureMonitorIdentity, CapturedImage, DisplayGeometry, DisplayInfo, NativeCaptureCall,
    NativeCaptureReply, PixelFormat, WindowInfo,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    sync::Arc,
};

const MAGIC: &[u8; 8] = b"SCNAT001";
const HEADER: usize = 32;
const META_MAX: usize = 1024 * 1024;
const CHUNK: usize = 64 * 1024;
pub(super) const HELLO: u32 = 1;
pub(super) const ACK: u32 = 2;
pub(super) const VIDEO_READY: u32 = 8;
pub(super) const VIDEO_START: u32 = 9;
pub(super) const VIDEO_FINISH: u32 = 10;
const REQUEST: u32 = 3;
const META: u32 = 4;
const FRAME: u32 = 5;
const ERROR: u32 = 6;
const END: u32 = 7;
const DROP_BODY: u32 = 11;
fn packet(output: &mut impl Write, kind: u32, metadata: &[u8], pixels: u64) -> Result<()> {
    let mut header = [0; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&kind.to_be_bytes());
    header[12..16].copy_from_slice(&(metadata.len() as u32).to_be_bytes());
    header[16..24].copy_from_slice(&pixels.to_be_bytes());
    output.write_all(&header)?;
    output.write_all(metadata)?;
    Ok(())
}
fn header(input: &mut impl Read) -> Result<(u32, usize, u64)> {
    let mut value = [0; HEADER];
    input.read_exact(&mut value)?;
    ensure!(
        &value[..8] == MAGIC && value[24..] == [0; 8],
        "Invalid native worker header"
    );
    Ok((
        u32::from_be_bytes(value[8..12].try_into().unwrap()),
        u32::from_be_bytes(value[12..16].try_into().unwrap()) as usize,
        u64::from_be_bytes(value[16..24].try_into().unwrap()),
    ))
}
fn metadata(input: &mut impl Read, length: usize, limit: usize) -> Result<Vec<u8>> {
    ensure!(length <= limit, "Native worker metadata exceeds its bound");
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn object<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ensure!(
        bytes
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            == Some(b'{'),
        "Native worker metadata must be an object"
    );
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("Invalid native worker metadata"))
}
pub(super) fn handshake(output: &mut impl Write, kind: u32, nonce: &[u8; 16]) -> Result<()> {
    packet(output, kind, nonce, 0)?;
    output.flush()?;
    Ok(())
}
pub(super) fn expect_handshake(input: &mut impl Read, kind: u32, nonce: &[u8; 16]) -> Result<()> {
    let (actual, length, pixels) = header(input)?;
    if actual == ERROR {
        ensure!(length == 1 && pixels == 0, "Invalid native error reply");
        let mut code = [0];
        input.read_exact(&mut code)?;
        return Err(error_from_code(code[0])?.into());
    }
    ensure!(
        actual == kind && length == 16 && pixels == 0,
        "Native worker handshake failed"
    );
    let mut value = [0; 16];
    input.read_exact(&mut value)?;
    ensure!(&value == nonce, "Native worker challenge mismatch");
    Ok(())
}
pub(super) fn request(output: &mut impl Write, call: &NativeCaptureCall) -> Result<()> {
    call.validate()?;
    let bytes = serde_json::to_vec(call)?;
    ensure!(bytes.len() <= 65536, "Native request exceeds 64 KiB");
    packet(output, REQUEST, &bytes, 0)?;
    output.flush()?;
    Ok(())
}
pub(super) fn read_request(input: &mut impl Read) -> Result<NativeCaptureCall> {
    let (kind, length, pixels) = header(input)?;
    ensure!(
        kind == REQUEST && pixels == 0 && length > 0,
        "Invalid native request"
    );
    let bytes = metadata(input, length, 65536)?;
    let shape: serde_json::Value = object(&bytes)?;
    let allowed: &[&str] = match shape.get("operation").and_then(|value| value.as_str()) {
        Some("inventory" | "displays" | "pixel_displays" | "windows") => &["operation"],
        Some("identities") => &["operation", "displays"],
        Some("batch") => &["operation", "requests", "options"],
        Some("focused_window" | "focused_display" | "desktop") => &["operation", "options"],
        Some("template") => &["operation", "template", "options"],
        Some("mac_scroll") => &["operation", "request"],
        Some("mac_video" | "portal_video" | "portal_scroll") => &["operation", "request"],
        Some("portal") => &["operation", "source", "cursor"],
        Some("image_url") => &["operation", "source"],
        Some("native_drop") => &["operation", "source", "image"],
        Some("clipboard_image") => &["operation", "backend"],
        Some("clipboard_inspect") => &["operation", "kind", "backend"],
        _ => anyhow::bail!("Unknown native operation"),
    };
    ensure!(
        shape
            .as_object()
            .unwrap()
            .keys()
            .all(|key| allowed.contains(&key.as_str())),
        "Unknown native request field"
    );
    ensure!(
        shape
            .get("options")
            .is_none_or(serde_json::Value::is_object),
        "Native options must be an object"
    );
    if shape.get("operation").and_then(|value| value.as_str()) == Some("image_url") {
        ensure!(
            shape
                .get("source")
                .is_some_and(serde_json::Value::is_object),
            "Image URL source must be an object"
        );
    }
    if let Some(requests) = shape.get("requests") {
        ensure!(
            requests
                .as_array()
                .is_some_and(|requests| requests.iter().all(|request| {
                    request.is_object()
                        && request
                            .get("value")
                            .is_some_and(|value| value.is_string() || value.is_object())
                })),
            "Native targets must be objects"
        );
        for request in requests.as_array().unwrap() {
            if matches!(
                request.get("kind").and_then(|value| value.as_str()),
                Some("repeat_region" | "exact_region")
            ) {
                let value = &request["value"];
                ensure!(
                    value.get("area").is_some_and(serde_json::Value::is_object)
                        && value
                            .get("geometry")
                            .is_some_and(serde_json::Value::is_object),
                    "Native region metadata must be objects"
                );
            }
        }
    }
    if let Some(displays) = shape.get("displays") {
        ensure!(
            displays
                .as_array()
                .is_some_and(|values| values.iter().all(serde_json::Value::is_object)),
            "Native display metadata must be objects"
        );
    }
    if let Some(template) = shape.get("template") {
        ensure!(
            template.is_object()
                && template
                    .get("monitor")
                    .is_some_and(serde_json::Value::is_object)
                && template
                    .get("geometry")
                    .is_some_and(serde_json::Value::is_object),
            "Native template must be an object"
        );
    }
    if matches!(
        shape.get("operation").and_then(|value| value.as_str()),
        Some("portal_video" | "portal_scroll")
    ) {
        ensure!(
            shape
                .get("request")
                .is_some_and(|request| request.is_object()
                    && request
                        .get("stream")
                        .is_some_and(serde_json::Value::is_object)),
            "Portal stream request must be an object"
        );
    }
    let call: NativeCaptureCall = object(&bytes)?;
    if matches!(
        call,
        NativeCaptureCall::MacScroll { .. } | NativeCaptureCall::MacVideo { .. }
    ) {
        let request = shape
            .get("request")
            .and_then(serde_json::Value::as_object)
            .context("Native scroll request must be an object")?;
        let region = request
            .get("region")
            .and_then(serde_json::Value::as_object)
            .context("Native scroll region must be an object")?;
        ensure!(
            region
                .keys()
                .all(|key| ["x", "y", "width", "height"].contains(&key.as_str())),
            "Unknown native scroll region field"
        );
    }
    call.validate()?;
    Ok(call)
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Meta {
    PortalVideo {
        summary: app_core::PortalVideoSummary,
    },
    PortalScroll {
        count: usize,
        summary: app_core::PortalScrollSummary,
    },
    NativeDrop {
        mime: app_core::NativeDropMime,
        bytes: usize,
        root_offset: Option<[i16; 2]>,
    },
    MacVideo {
        summary: app_core::MacVideoSummary,
    },
    MacScroll {
        count: usize,
        summary: app_core::MacScrollSummary,
    },
    Clipboard {
        snapshot: app_core::ClipboardSnapshot,
    },
    Inventory {
        displays: Option<Vec<DisplayInfo>>,
        windows: Option<Vec<WindowInfo>>,
        display_error: Option<u8>,
        window_error: Option<u8>,
    },
    Displays {
        values: Vec<DisplayInfo>,
    },
    Windows {
        values: Vec<WindowInfo>,
    },
    Identities {
        values: Vec<CaptureMonitorIdentity>,
    },
    Frames {
        count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        desktop_layout: Option<app_core::MacDesktopLayout>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    display_id: String,
    window_id: Option<String>,
    width: u32,
    height: u32,
    elapsed_ms: u128,
    source_format: PixelFormat,
    display_geometry: Option<DisplayGeometry>,
}
pub(super) fn reply(output: &mut impl Write, reply: NativeCaptureReply) -> Result<()> {
    if let NativeCaptureReply::NativeDrop(reply) = reply {
        ensure!(
            reply.bytes.len() <= reply.mime.maximum(),
            "Native drop exceeds its bound"
        );
        let value = Meta::NativeDrop {
            mime: reply.mime,
            bytes: reply.bytes.len(),
            root_offset: reply.root_offset,
        };
        let metadata = serde_json::to_vec(&value)?;
        packet(output, META, &metadata, 0)?;
        packet(output, DROP_BODY, &[], reply.bytes.len() as u64)?;
        for bytes in reply.bytes.chunks(CHUNK) {
            output.write_all(bytes)?;
        }
        packet(output, END, &[], 0)?;
        output.flush()?;
        return Ok(());
    }
    let (value, frames) = match reply {
        NativeCaptureReply::PortalVideo { summary } => (Meta::PortalVideo { summary }, Vec::new()),
        NativeCaptureReply::PortalScroll { image, summary } => {
            (Meta::PortalScroll { count: 1, summary }, vec![image])
        }
        NativeCaptureReply::NativeDrop(_) => unreachable!("Native drop has its bounded wire body"),
        NativeCaptureReply::MacVideo { summary } => (Meta::MacVideo { summary }, Vec::new()),
        NativeCaptureReply::MacScroll { image, summary } => {
            (Meta::MacScroll { count: 1, summary }, vec![image])
        }
        NativeCaptureReply::Clipboard(snapshot) => {
            snapshot.validate()?;
            (Meta::Clipboard { snapshot }, Vec::new())
        }
        NativeCaptureReply::Inventory(inventory) => {
            let (displays, display_error) = match inventory.displays {
                Ok(values) => (Some(values), None),
                Err(error) => (None, Some(error_code(&error))),
            };
            let (windows, window_error) = match inventory.windows {
                Ok(values) => (Some(values), None),
                Err(error) => (None, Some(error_code(&error))),
            };
            (
                Meta::Inventory {
                    displays,
                    windows,
                    display_error,
                    window_error,
                },
                Vec::new(),
            )
        }
        NativeCaptureReply::Displays(values) => (Meta::Displays { values }, Vec::new()),
        NativeCaptureReply::Windows(values) => (Meta::Windows { values }, Vec::new()),
        NativeCaptureReply::Identities(values) => (Meta::Identities { values }, Vec::new()),
        NativeCaptureReply::Frames(frames) => (
            Meta::Frames {
                count: frames.len(),
                desktop_layout: frames
                    .first()
                    .and_then(|frame| frame.desktop_layout.as_deref())
                    .cloned(),
            },
            frames,
        ),
    };
    let bytes = serde_json::to_vec(&value)?;
    ensure!(bytes.len() <= META_MAX, "Native metadata exceeds 1 MiB");
    packet(output, META, &bytes, 0)?;
    for frame in frames {
        let value = Frame {
            display_id: frame.display_id,
            window_id: frame.window_id,
            width: frame.width,
            height: frame.height,
            elapsed_ms: frame.elapsed_ms,
            source_format: frame.source_format,
            display_geometry: frame.display_geometry,
        };
        let bytes = serde_json::to_vec(&value)?;
        ensure!(
            bytes.len() <= 4096,
            "Native frame metadata exceeds 4096 bytes"
        );
        packet(output, FRAME, &bytes, frame.pixels.as_raw().len() as u64)?;
        for bytes in frame.pixels.as_raw().chunks(CHUNK) {
            output.write_all(bytes)?;
        }
    }
    packet(output, END, &[], 0)?;
    output.flush()?;
    Ok(())
}
fn error_code(error: &app_core::CaptureError) -> u8 {
    use app_core::CaptureError as E;
    match error {
        E::StaleRegion => 1,
        E::DisplayNotFound(_) => 2,
        E::WindowNotFound(_) => 3,
        E::MemoryLimit => 4,
        E::EmptyFrame => 5,
        E::InvalidRegion => 6,
        E::Unsupported(_) => 7,
        E::NativeCanceled => 8,
        E::NativeTimeout => 9,
        E::PermissionDenied => 10,
        E::ImageUrlFailed => 11,
        E::Clipboard => 12,
        E::MacScreenPermissionDenied => 13,
        E::MacAccessibilityPermissionDenied => 14,
        E::MacMicrophonePermissionDenied => 15,
        E::ImageColorPolicy => 16,
        E::TemplateRebindRequired => 17,
        _ => 0,
    }
}
fn error_from_code(code: u8) -> Result<app_core::CaptureError> {
    use app_core::CaptureError as E;
    Ok(match code {
        0 => E::Backend(
            "Native operation failed; verify source availability and OS permissions".into(),
        ),
        1 => E::StaleRegion,
        2 => E::DisplayNotFound("requested display".into()),
        3 => E::WindowNotFound("requested window".into()),
        4 => E::MemoryLimit,
        5 => E::EmptyFrame,
        6 => E::InvalidRegion,
        7 => E::Unsupported("requested native source or policy"),
        8 => E::NativeCanceled,
        9 => E::NativeTimeout,
        10 => E::PermissionDenied,
        11 => E::ImageUrlFailed,
        12 => E::Clipboard,
        13 => E::MacScreenPermissionDenied,
        14 => E::MacAccessibilityPermissionDenied,
        15 => E::MacMicrophonePermissionDenied,
        16 => E::ImageColorPolicy,
        17 => E::TemplateRebindRequired,
        _ => anyhow::bail!("Unknown native error category"),
    })
}
pub(super) fn failure(output: &mut impl Write, error: &app_core::CaptureError) -> Result<()> {
    let code = error_code(error);
    packet(output, ERROR, &[code], 0)?;
    output.flush()?;
    Ok(())
}
pub(super) fn read_reply(
    input: &mut impl Read,
    call: &NativeCaptureCall,
) -> Result<NativeCaptureReply> {
    read_reply_with_eof(input, call, true)
}
pub(super) fn read_reply_with_eof(
    input: &mut impl Read,
    call: &NativeCaptureCall,
    require_eof: bool,
) -> Result<NativeCaptureReply> {
    let (kind, length, pixels) = header(input)?;
    if kind == ERROR {
        ensure!(length == 1 && pixels == 0, "Invalid native error reply");
        let mut code = [0];
        input.read_exact(&mut code)?;
        let error = error_from_code(code[0])?;
        return Err(error.into());
    }
    ensure!(
        kind == META && pixels == 0 && length > 0,
        "Invalid native reply"
    );
    let bytes = metadata(input, length, META_MAX)?;
    let shape: serde_json::Value = object(&bytes)?;
    if let Some(values) = shape.get("values") {
        ensure!(
            values
                .as_array()
                .is_some_and(|values| values.iter().all(serde_json::Value::is_object)),
            "Native inventory records must be objects"
        );
    }
    for key in ["displays", "windows"] {
        if let Some(values) = shape.get(key).filter(|value| !value.is_null()) {
            ensure!(
                values
                    .as_array()
                    .is_some_and(|values| values.iter().all(serde_json::Value::is_object)),
                "Native inventory records must be objects"
            );
        }
    }
    let value: Meta = object(&bytes)?;
    if matches!(
        value,
        Meta::MacVideo { .. } | Meta::PortalVideo { .. } | Meta::PortalScroll { .. }
    ) {
        ensure!(
            shape
                .get("summary")
                .is_some_and(serde_json::Value::is_object),
            "Native video summary must be an object"
        );
    }
    if matches!(value, Meta::MacScroll { .. }) {
        let summary = shape
            .get("summary")
            .and_then(serde_json::Value::as_object)
            .context("Native scroll summary must be an object")?;
        ensure!(
            summary
                .get("vertical_advances")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|values| values.len() <= 128),
            "Invalid native scroll advances"
        );
    }
    let portal_scroll = match &value {
        Meta::PortalScroll { count, summary } => {
            ensure!(
                *count == 1 && summary.vertical_advances.len() <= 128,
                "Invalid portal scroll frame/summary bound"
            );
            let NativeCaptureCall::PortalScroll { request } = call else {
                anyhow::bail!("Unexpected portal scroll reply");
            };
            ensure!(
                summary.captured_frames == request.frames,
                "Invalid portal scroll frame count"
            );
            Some(summary.clone())
        }
        _ => None,
    };
    let scroll = match &value {
        Meta::MacScroll { count, summary } => {
            ensure!(
                *count == 1
                    && summary.vertical_advances.len() <= 128
                    && matches!(call, NativeCaptureCall::MacScroll { .. }),
                "Unexpected or oversized native scroll summary"
            );
            Some(summary.clone())
        }
        _ => None,
    };
    ensure!(
        !matches!(call, NativeCaptureCall::MacScroll { .. }) || scroll.is_some(),
        "Native scrolling reply omitted its summary"
    );
    let desktop_layout = match &value {
        Meta::Frames {
            desktop_layout: Some(layout),
            count,
        } => {
            ensure!(
                cfg!(target_os = "macos")
                    && *count == 1
                    && matches!(call, NativeCaptureCall::Desktop { .. }),
                "Unexpected native desktop grid"
            );
            layout.validate()?;
            Some(Arc::new(layout.clone()))
        }
        _ => None,
    };
    ensure!(
        !cfg!(target_os = "macos")
            || !matches!(call, NativeCaptureCall::Desktop { .. })
            || desktop_layout.is_some(),
        "Native macOS desktop omitted its output grid metadata"
    );
    let reply = match value {
        Meta::PortalVideo { summary } => NativeCaptureReply::PortalVideo { summary },
        Meta::NativeDrop {
            mime,
            bytes,
            root_offset,
        } => {
            let NativeCaptureCall::NativeDrop { source, image } = call else {
                anyhow::bail!("Unexpected native drop reply");
            };
            app_core::NativeDropReply::validate_shape(mime, bytes, *image)?;
            ensure!(
                root_offset.is_some() == matches!(source, app_core::NativeDropSource::X11 { .. }),
                "Native drop coordinate authority differs from its request"
            );
            let (kind, metadata, body) = header(input)?;
            ensure!(
                kind == DROP_BODY && metadata == 0 && body == bytes as u64,
                "Invalid native drop body framing"
            );
            let mut body = Vec::new();
            body.try_reserve_exact(bytes)?;
            body.resize(bytes, 0);
            for chunk in body.chunks_mut(CHUNK) {
                input.read_exact(chunk)?;
            }
            NativeCaptureReply::NativeDrop(app_core::NativeDropReply {
                mime,
                bytes: body,
                root_offset,
            })
        }
        Meta::MacVideo { summary } => NativeCaptureReply::MacVideo { summary },
        Meta::Clipboard { snapshot } => {
            snapshot.validate()?;
            NativeCaptureReply::Clipboard(snapshot)
        }
        Meta::Inventory {
            displays,
            windows,
            display_error,
            window_error,
        } => {
            ensure!(
                displays.is_some() != display_error.is_some()
                    && windows.is_some() != window_error.is_some(),
                "Native inventory must contain one result per source kind"
            );
            NativeCaptureReply::Inventory(app_core::NativeSourceInventory {
                displays: match displays {
                    Some(values) => Ok(values),
                    None => Err(error_from_code(display_error.unwrap())?),
                },
                windows: match windows {
                    Some(values) => Ok(values),
                    None => Err(error_from_code(window_error.unwrap())?),
                },
            })
        }
        Meta::Displays { values } => NativeCaptureReply::Displays(values),
        Meta::Windows { values } => NativeCaptureReply::Windows(values),
        Meta::Identities { values } => NativeCaptureReply::Identities(values),
        Meta::Frames {
            count,
            desktop_layout: _,
        }
        | Meta::MacScroll { count, summary: _ }
        | Meta::PortalScroll { count, summary: _ } => {
            ensure!(
                (1..=32).contains(&count) && call.frame_count() == Some(count),
                "Native frame count exceeds its bound or differs from its request"
            );
            let mut frames = Vec::with_capacity(count);
            let mut total = 0u64;
            for index in 0..count {
                let (kind, length, pixels) = header(input)?;
                ensure!(kind == FRAME && length > 0, "Invalid native frame header");
                let bytes = metadata(input, length, 4096)?;
                let shape: serde_json::Value = object(&bytes)?;
                ensure!(
                    shape
                        .get("display_geometry")
                        .is_none_or(|value| value.is_null() || value.is_object()),
                    "Native geometry must be an object"
                );
                let frame: Frame = object(&bytes)?;
                call.validate_frame_metadata(
                    index,
                    &frame.display_id,
                    frame.window_id.as_deref(),
                    frame.width,
                    frame.height,
                    frame.display_geometry,
                )?;
                if let (Some(summary), NativeCaptureCall::MacScroll { request }) = (&scroll, call) {
                    summary.validate(request, frame.width, frame.height)?;
                }
                if let (Some(summary), NativeCaptureCall::PortalScroll { request }) =
                    (&portal_scroll, call)
                {
                    summary.validate(*request, frame.width, frame.height)?;
                }
                if let Some(layout) = &desktop_layout {
                    ensure!(
                        (frame.width, frame.height) == (layout.width, layout.height),
                        "Desktop frame/grid dimensions disagree"
                    );
                }
                let expected = admit_frame(&mut total, frame.width, frame.height, pixels)?;
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(expected as usize)?;
                bytes.resize(expected as usize, 0);
                for bytes in bytes.chunks_mut(CHUNK) {
                    input.read_exact(bytes)?;
                }
                let pixels = image::RgbaImage::from_raw(frame.width, frame.height, bytes)
                    .context("Invalid native pixel backing")?;
                frames.push(CapturedImage {
                    desktop_layout: desktop_layout.clone(),
                    display_id: frame.display_id,
                    window_id: frame.window_id,
                    width: frame.width,
                    height: frame.height,
                    png_path: std::path::PathBuf::new(),
                    pixels: Arc::new(pixels),
                    elapsed_ms: frame.elapsed_ms,
                    source_format: frame.source_format,
                    display_geometry: frame.display_geometry,
                });
            }
            if let Some(summary) = portal_scroll {
                NativeCaptureReply::PortalScroll {
                    image: frames.pop().context("Portal scrolling frame missing")?,
                    summary,
                }
            } else if let Some(summary) = scroll {
                NativeCaptureReply::MacScroll {
                    image: frames.pop().context("Native scrolling frame missing")?,
                    summary,
                }
            } else {
                NativeCaptureReply::Frames(frames)
            }
        }
    };
    let (kind, length, pixels) = header(input)?;
    ensure!(
        kind == END && length == 0 && pixels == 0,
        "Native reply did not terminate exactly"
    );
    if require_eof {
        expect_eof(input)?;
    }
    call.validate_reply(&reply)?;
    Ok(reply)
}
pub(super) fn expect_eof(input: &mut impl Read) -> Result<()> {
    let mut extra = [0];
    ensure!(
        input.read(&mut extra)? == 0,
        "Native reply has trailing data"
    );
    Ok(())
}
fn admit_frame(total: &mut u64, width: u32, height: u32, payload: u64) -> Result<u64> {
    app_core::validate_image_size(width, height)?;
    let expected = u64::from(width) * u64::from(height) * 4;
    ensure!(
        payload == expected,
        "Native frame payload does not match dimensions"
    );
    let next = total
        .checked_add(expected)
        .filter(|total| *total <= app_core::MAX_CAPTURE_BYTES)
        .context("Native frames exceed 256 MiB")?;
    *total = next;
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mac_scroll() -> (NativeCaptureCall, NativeCaptureReply) {
        let request = app_core::MacScrollRequest {
            window: 42,
            region: app_core::PixelRegion {
                x: 0,
                y: 0,
                width: 2,
                height: 4,
            },
            steps: 4,
            wheel: 1,
            settle_ms: 20,
            minimum_overlap: 1,
            tolerance: 0,
            trim_top: 0,
            trim_bottom: 0,
        };
        let image = CapturedImage {
            desktop_layout: None,
            display_id: "7".into(),
            window_id: Some("42".into()),
            width: 2,
            height: 6,
            png_path: "never-transmit-scroll-output.png".into(),
            pixels: Arc::new(image::RgbaImage::from_pixel(
                2,
                6,
                image::Rgba([7, 11, 13, 255]),
            )),
            elapsed_ms: 1,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        let summary = app_core::MacScrollSummary {
            captured_frames: 4,
            scroll_actions: 3,
            vertical_advances: vec![2],
            stop_reason: app_core::MacScrollStop::Unchanged,
            complete: true,
        };
        (
            NativeCaptureCall::MacScroll { request },
            NativeCaptureReply::MacScroll { image, summary },
        )
    }
    #[test]
    fn native_scroll_summary_and_pixels_are_admitted_together_without_output_authority() {
        let (call, response) = mac_scroll();
        let mut bytes = Vec::new();
        reply(&mut bytes, response).unwrap();
        assert!(
            !bytes
                .windows(20)
                .any(|bytes| bytes == b"never-transmit-scroll")
        );
        let response = read_reply(&mut bytes.as_slice(), &call).unwrap();
        let NativeCaptureReply::MacScroll { image, summary } = response else {
            panic!("Expected scroll reply")
        };
        assert_eq!(image.pixels.get_pixel(0, 0).0, [7, 11, 13, 255]);
        assert!(summary.complete);
        assert_eq!(summary.vertical_advances, vec![2]);
        let (_, response) = mac_scroll();
        let NativeCaptureReply::MacScroll { image, .. } = response else {
            unreachable!()
        };
        let mut bytes = Vec::new();
        reply(&mut bytes, NativeCaptureReply::Frames(vec![image])).unwrap();
        assert!(read_reply(&mut bytes.as_slice(), &call).is_err());
    }
    #[test]
    fn invalid_scroll_summary_identity_and_permission_categories_fail_truthfully() {
        for mode in 0..3 {
            let (call, response) = mac_scroll();
            let NativeCaptureReply::MacScroll {
                mut image,
                mut summary,
            } = response
            else {
                unreachable!()
            };
            match mode {
                0 => summary.complete = false,
                1 => summary.vertical_advances = vec![2; 129],
                _ => image.window_id = Some("43".into()),
            };
            let mut bytes = Vec::new();
            reply(&mut bytes, NativeCaptureReply::MacScroll { image, summary }).unwrap();
            assert!(read_reply(&mut bytes.as_slice(), &call).is_err());
        }
        for error in [
            app_core::CaptureError::MacScreenPermissionDenied,
            app_core::CaptureError::MacAccessibilityPermissionDenied,
            app_core::CaptureError::ImageColorPolicy,
        ] {
            let mut bytes = Vec::new();
            failure(&mut bytes, &error).unwrap();
            let (call, _) = mac_scroll();
            let result = read_reply(&mut bytes.as_slice(), &call).err().unwrap();
            assert_eq!(result.to_string(), error.to_string());
        }
    }
    fn call() -> NativeCaptureCall {
        NativeCaptureCall::Batch {
            requests: vec![app_core::CaptureRequest::Region {
                display_id: "fixture".into(),
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            }],
            options: app_core::CaptureOptions::default(),
        }
    }
    fn frame() -> CapturedImage {
        CapturedImage {
            desktop_layout: None,
            display_id: "fixture".into(),
            window_id: None,
            width: 2,
            height: 1,
            png_path: "never-transfer-this-path".into(),
            pixels: Arc::new(
                image::RgbaImage::from_raw(2, 1, vec![13, 17, 19, 0, 23, 29, 31, 255]).unwrap(),
            ),
            elapsed_ms: 7,
            source_format: PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }
    }
    #[test]
    fn raw_protocol_roundtrips_exact_hidden_rgb_alpha_and_never_transfers_output_paths() {
        let mut bytes = Vec::new();
        reply(&mut bytes, NativeCaptureReply::Frames(vec![frame()])).unwrap();
        assert!(
            !bytes
                .windows(b"never-transfer-this-path".len())
                .any(|value| value == b"never-transfer-this-path")
        );
        let NativeCaptureReply::Frames(frames) =
            read_reply(&mut bytes.as_slice(), &call()).unwrap()
        else {
            panic!("not frames");
        };
        assert_eq!(frames[0].pixels.as_raw(), frame().pixels.as_raw());
        assert!(frames[0].png_path.as_os_str().is_empty());
        let mut encoded = Vec::new();
        request(&mut encoded, &call()).unwrap();
        read_request(&mut encoded.as_slice()).unwrap();
    }
    #[test]
    fn native_frame_byte_admission_checks_exact_aggregate_bound_without_allocating() {
        let mut total = app_core::MAX_CAPTURE_BYTES - 8;
        assert_eq!(admit_frame(&mut total, 2, 1, 8).unwrap(), 8);
        assert_eq!(total, app_core::MAX_CAPTURE_BYTES);
        assert!(admit_frame(&mut total, 1, 1, 4).is_err());
        assert_eq!(total, app_core::MAX_CAPTURE_BYTES);
        for (w, h, bytes) in [(0, 1, 0), (u32::MAX, u32::MAX, u64::MAX), (1, 1, 5)] {
            let mut total = 0;
            assert!(admit_frame(&mut total, w, h, bytes).is_err());
            assert_eq!(total, 0);
        }
    }
    #[test]
    fn private_handshake_rejects_wrong_magic_nonce_and_reserved_header_fields() {
        let nonce = [17; 16];
        let mut bytes = Vec::new();
        handshake(&mut bytes, HELLO, &nonce).unwrap();
        expect_handshake(&mut bytes.as_slice(), HELLO, &nonce).unwrap();
        assert!(expect_handshake(&mut bytes.as_slice(), ACK, &nonce).is_err());
        assert!(expect_handshake(&mut bytes.as_slice(), HELLO, &[18; 16]).is_err());
        bytes[31] = 1;
        assert!(expect_handshake(&mut bytes.as_slice(), HELLO, &nonce).is_err());
        bytes[0] = 0;
        assert!(expect_handshake(&mut bytes.as_slice(), HELLO, &nonce).is_err());
    }
    #[test]
    fn typed_failures_preserve_stale_region_semantics_without_native_diagnostics() {
        let mut bytes = Vec::new();
        failure(&mut bytes, &app_core::CaptureError::StaleRegion).unwrap();
        let error = read_reply(&mut bytes.as_slice(), &call())
            .err()
            .unwrap()
            .downcast::<app_core::CaptureError>()
            .unwrap();
        assert!(matches!(error, app_core::CaptureError::StaleRegion));
        let mut bytes = Vec::new();
        failure(
            &mut bytes,
            &app_core::CaptureError::Backend("generated-private-diagnostic".into()),
        )
        .unwrap();
        assert!(
            !bytes
                .windows(b"generated-private-diagnostic".len())
                .any(|value| value == b"generated-private-diagnostic")
        );
        assert!(read_reply(&mut bytes.as_slice(), &call()).is_err());
    }
    #[test]
    fn malformed_truncated_extra_or_wrong_target_frames_never_return_partial_results() {
        let mut valid = Vec::new();
        reply(&mut valid, NativeCaptureReply::Frames(vec![frame()])).unwrap();
        for length in [0, 1, HEADER - 1, valid.len() - 1] {
            assert!(read_reply(&mut &valid[..length], &call()).is_err());
        }
        let mut extra = valid.clone();
        extra.push(1);
        assert!(read_reply(&mut extra.as_slice(), &call()).is_err());
        let wrong = NativeCaptureCall::Batch {
            requests: vec![app_core::CaptureRequest::Display("other".into())],
            options: Default::default(),
        };
        assert!(read_reply(&mut valid.as_slice(), &wrong).is_err());
        let mut malformed = Vec::new();
        packet(&mut malformed, META, br#"{"kind":"frames","count":33}"#, 0).unwrap();
        assert!(read_reply(&mut malformed.as_slice(), &call()).is_err());
        let mut malformed = Vec::new();
        packet(
            &mut malformed,
            META,
            br#"{"kind":"frames","count":1,"count":1}"#,
            0,
        )
        .unwrap();
        assert!(read_reply(&mut malformed.as_slice(), &call()).is_err());
    }
    #[test]
    fn native_request_shape_unknown_fields_and_delay_are_rejected() {
        for bytes in [br#"{"operation":"focused_window","options":[]}"#.as_slice(),
            br#"{"operation":"focused_window","options":{"delay_ms":1}}"#,
            br#"{"operation":"displays","unexpected":true}"#,
            br#"{"operation":"identities","displays":[[]]}"#,
            br#"{"operation":"batch","requests":[{"kind":"region","value":["fixture",0,0,2,1]}],"options":{}}"#] {
            let mut encoded = Vec::new(); packet(&mut encoded, REQUEST, bytes, 0).unwrap(); assert!(read_request(&mut encoded.as_slice()).is_err());
        }
    }
    fn desktop_frame() -> CapturedImage {
        let mut shot = frame();
        shot.display_id = "desktop".into();
        #[cfg(target_os = "macos")]
        {
            let logical = app_core::MacLogicalRect {
                x: 0.0,
                y: 0.0,
                width: 2.0,
                height: 1.0,
            };
            let proof = app_core::MacDisplayProof {
                id: 1,
                uuid: "00000000-0000-0000-0000-000000000001".into(),
                logical,
                pixel_width: 2,
                pixel_height: 1,
                rotation: 0.0,
            };
            let coordinates =
                app_core::MacDisplayCoordinates::from_filter(proof, logical, logical, 1.0).unwrap();
            shot.desktop_layout = Some(Arc::new(
                app_core::MacDesktopLayout::new(&[coordinates]).unwrap(),
            ));
        }
        shot
    }
    #[test]
    fn physical_desktop_protocol_keeps_one_exact_frame_and_rejects_other_sources() {
        let call = NativeCaptureCall::Desktop {
            options: Default::default(),
        };
        let mut encoded = Vec::new();
        request(&mut encoded, &call).unwrap();
        assert!(matches!(
            read_request(&mut encoded.as_slice()).unwrap(),
            NativeCaptureCall::Desktop { .. }
        ));
        let shot = desktop_frame();
        let expected_layout = shot.desktop_layout.clone();
        let expected = shot.pixels.clone();
        let mut encoded = Vec::new();
        reply(&mut encoded, NativeCaptureReply::Frames(vec![shot])).unwrap();
        let NativeCaptureReply::Frames(frames) =
            read_reply(&mut encoded.as_slice(), &call).unwrap()
        else {
            panic!("not frames");
        };
        assert_eq!(frames[0].pixels, expected);
        assert_eq!(frames[0].desktop_layout, expected_layout);
        let mut wrong = Vec::new();
        reply(&mut wrong, NativeCaptureReply::Frames(vec![frame()])).unwrap();
        assert!(read_reply(&mut wrong.as_slice(), &call).is_err());
        for bytes in [
            br#"{"operation":"desktop","options":[],"requests":[]}"#.as_slice(),
            br#"{"operation":"desktop","options":{"delay_ms":1}}"#,
        ] {
            let mut packet_bytes = Vec::new();
            packet(&mut packet_bytes, REQUEST, bytes, 0).unwrap();
            assert!(read_request(&mut packet_bytes.as_slice()).is_err());
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_desktop_protocol_rejects_missing_grid_before_reading_raw_pixels() {
        let call = NativeCaptureCall::Desktop {
            options: Default::default(),
        };
        let mut shot = desktop_frame();
        shot.desktop_layout = None;
        let mut bytes = Vec::new();
        reply(&mut bytes, NativeCaptureReply::Frames(vec![shot])).unwrap();
        let error = read_reply(&mut bytes.as_slice(), &call).err().unwrap();
        assert!(error.to_string().contains("omitted its output grid"));
    }

    #[test]
    fn edited_desktop_rasters_drop_grid_metadata_before_region_protocol_roundtrip() {
        let mut original = desktop_frame();
        if original.desktop_layout.is_none() {
            let logical = app_core::MacLogicalRect {
                x: 0.0,
                y: 0.0,
                width: 2.0,
                height: 1.0,
            };
            let proof = app_core::MacDisplayProof {
                id: 1,
                uuid: "00000000-0000-0000-0000-000000000001".into(),
                logical,
                pixel_width: 2,
                pixel_height: 1,
                rotation: 0.0,
            };
            let coordinates =
                app_core::MacDisplayCoordinates::from_filter(proof, logical, logical, 1.0).unwrap();
            original.desktop_layout = Some(Arc::new(
                app_core::MacDesktopLayout::new(&[coordinates]).unwrap(),
            ));
        }
        let area = app_core::PixelRegion {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let cropped = app_core::crop_snapshot(&original, area).unwrap();
        let mut document =
            app_core::editor_document::EditorDocument::from_image(original.clone()).unwrap();
        document.set_crop(Some(area)).unwrap();
        let rendered = document.snapshot().render(|| false).unwrap();
        for frame in [cropped, rendered] {
            assert!(frame.desktop_layout.is_none());
            let call = NativeCaptureCall::Batch {
                requests: vec![app_core::CaptureRequest::Region {
                    display_id: "desktop".into(),
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                }],
                options: Default::default(),
            };
            let mut bytes = Vec::new();
            reply(&mut bytes, NativeCaptureReply::Frames(vec![frame])).unwrap();
            let NativeCaptureReply::Frames(frames) =
                read_reply(&mut bytes.as_slice(), &call).unwrap()
            else {
                panic!("expected frames")
            };
            assert_eq!(frames[0].pixels.dimensions(), (1, 1));
            assert!(frames[0].desktop_layout.is_none());
        }
        assert_eq!(original.desktop_layout.as_ref().unwrap().width, 2);
        assert_eq!(original.pixels.dimensions(), (2, 1));
    }
}

#[cfg(test)]
mod portal_stream_tests {
    use super::*;
    #[test]
    fn portal_video_reply_and_pre_ready_denial_keep_their_typed_contract() {
        let request = app_core::PortalVideoRequest {
            stream: app_core::PortalStreamSource {
                source: app_core::PortalCaptureSource::Window,
                cursor: app_core::PortalCaptureCursor::Exclude,
                width: 16,
                height: 12,
            },
            seconds: 1,
            fps: 5,
        };
        let call = NativeCaptureCall::PortalVideo { request };
        let mut bytes = Vec::new();
        reply(
            &mut bytes,
            NativeCaptureReply::PortalVideo {
                summary: app_core::PortalVideoSummary {
                    frames: 5,
                    decoded_frames: 1,
                    repeated_frames: 4,
                    sample_span_ns: 800_000_000,
                },
            },
        )
        .unwrap();
        assert!(
            matches!(read_reply(&mut bytes.as_slice(), &call).unwrap(), NativeCaptureReply::PortalVideo { summary } if summary.frames == 5)
        );
        assert!(read_reply(&mut bytes.as_slice(), &NativeCaptureCall::Displays).is_err());
        let mut denied = Vec::new();
        failure(&mut denied, &app_core::CaptureError::PermissionDenied).unwrap();
        let error = expect_handshake(&mut denied.as_slice(), VIDEO_READY, &[1; 16]).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<app_core::CaptureError>(),
            Some(app_core::CaptureError::PermissionDenied)
        ));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod native_drop_wire_tests {
    use super::*;
    fn call(image: bool) -> NativeCaptureCall {
        NativeCaptureCall::NativeDrop {
            source: app_core::NativeDropSource::X11 {
                display: ":65534".into(),
                source: 7,
                target: 11,
                types: [1, 0, 0],
                extended: false,
                timestamp: 123,
            },
            image,
        }
    }
    #[test]
    fn original_encoded_body_and_negotiation_have_exact_wire_phases() {
        for (image, mime, body) in [
            (false, app_core::NativeDropMime::Png, &b""[..]),
            (
                true,
                app_core::NativeDropMime::Png,
                &b"private-synthetic-raster"[..],
            ),
            (
                false,
                app_core::NativeDropMime::UriList,
                &b"file:///private/not-opened/a%FF.png\r\n"[..],
            ),
        ] {
            let call = call(image);
            let mut wire = Vec::new();
            reply(
                &mut wire,
                NativeCaptureReply::NativeDrop(app_core::NativeDropReply {
                    mime,
                    bytes: body.to_vec(),
                    root_offset: Some([-10, 20]),
                }),
            )
            .unwrap();
            let NativeCaptureReply::NativeDrop(actual) =
                read_reply(&mut wire.as_slice(), &call).unwrap()
            else {
                panic!("wrong reply kind")
            };
            assert_eq!(actual.bytes, body);
            assert_eq!(actual.mime, mime);
            assert_eq!(actual.root_offset, Some([-10, 20]));
        }
    }
    #[test]
    fn oversized_or_wrong_phase_body_is_refused_before_body_read() {
        for (mime, bytes, image, offset) in [
            (
                app_core::NativeDropMime::Png,
                32 * 1024 * 1024 + 1,
                true,
                Some([0, 0]),
            ),
            (app_core::NativeDropMime::Png, 1, false, Some([0, 0])),
            (app_core::NativeDropMime::UriList, 1, true, Some([0, 0])),
            (app_core::NativeDropMime::Png, 1, true, None),
        ] {
            let mut wire = Vec::new();
            let value = serde_json::to_vec(&Meta::NativeDrop {
                mime,
                bytes,
                root_offset: offset,
            })
            .unwrap();
            packet(&mut wire, META, &value, 0).unwrap();
            let metadata_end = wire.len();
            let mut input = std::io::Cursor::new(wire);
            assert!(read_reply(&mut input, &call(image)).is_err());
            assert_eq!(input.position(), metadata_end as u64);
        }
    }
}
