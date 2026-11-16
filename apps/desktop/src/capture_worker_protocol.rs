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
const REQUEST: u32 = 3;
const META: u32 = 4;
const FRAME: u32 = 5;
const ERROR: u32 = 6;
const END: u32 = 7;
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
        Some("inventory" | "displays" | "windows") => &["operation"],
        Some("identities") => &["operation", "displays"],
        Some("batch") => &["operation", "requests", "options"],
        Some("focused_window" | "focused_display") => &["operation", "options"],
        Some("template") => &["operation", "template", "options"],
        Some("portal") => &["operation", "source", "cursor"],
        Some("image_url") => &["operation", "source"],
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
    let call: NativeCaptureCall = object(&bytes)?;
    call.validate()?;
    Ok(call)
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Meta {
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
    let (value, frames) = match reply {
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
    let reply = match value {
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
        Meta::Frames { count } => {
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
            NativeCaptureReply::Frames(frames)
        }
    };
    let (kind, length, pixels) = header(input)?;
    ensure!(
        kind == END && length == 0 && pixels == 0,
        "Native reply did not terminate exactly"
    );
    let mut extra = [0];
    ensure!(
        input.read(&mut extra)? == 0,
        "Native reply has trailing data"
    );
    call.validate_reply(&reply)?;
    Ok(reply)
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
}
