//! Explicit bounded clipboard snapshots. No watcher, owner change, or idle cache.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub const CLIPBOARD_FORMATS: usize = 256;
pub const CLIPBOARD_FORMAT_BYTES: usize = 128;
pub const CLIPBOARD_TEXT_BYTES: usize = 64 * 1024;
pub const CLIPBOARD_FILE_COUNT: usize = 32;
pub const CLIPBOARD_FILE_BYTES: usize = 4096;
pub const CLIPBOARD_FILES_BYTES: usize = 64 * 1024;
pub const CLIPBOARD_ENCODED_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeClipboardBackend {
    #[default]
    Automatic,
    FocusedX11,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardInspectKind {
    Formats,
    Text,
    Files,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardPathEncoding {
    UnixBytes,
    WindowsUtf16Le,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClipboardFileReference {
    pub display: String,
    pub encoding: ClipboardPathEncoding,
    pub native: Vec<u8>,
}
impl std::fmt::Debug for ClipboardFileReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardFileReference")
            .field("native_bytes", &self.native.len())
            .finish_non_exhaustive()
    }
}
impl ClipboardFileReference {
    pub(crate) fn from_path(path: &std::path::Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let native = path.as_os_str().as_bytes().to_vec();
            let value = Self {
                display: path.display().to_string(),
                encoding: ClipboardPathEncoding::UnixBytes,
                native,
            };
            value.validate()?;
            Ok(value)
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::from_utf16(&path.as_os_str().encode_wide().collect::<Vec<_>>())
        }
    }
    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn from_utf16(units: &[u16]) -> Result<Self> {
        ensure!(
            !units.contains(&0) && units.len() <= CLIPBOARD_FILE_BYTES / 2,
            "Invalid or oversized clipboard file reference"
        );
        let native = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        let value = Self {
            display: String::from_utf16_lossy(units),
            encoding: ClipboardPathEncoding::WindowsUtf16Le,
            native,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.native.is_empty()
                && self.native.len() <= CLIPBOARD_FILE_BYTES
                && self.display.len() <= CLIPBOARD_FILE_BYTES * 3,
            "Clipboard file reference exceeds its bound"
        );
        match self.encoding {
            ClipboardPathEncoding::UnixBytes => {
                ensure!(!self.native.contains(&0), "Clipboard file contains a NUL");
                ensure!(
                    self.display == String::from_utf8_lossy(&self.native),
                    "Invalid clipboard file display"
                );
            }
            ClipboardPathEncoding::WindowsUtf16Le => {
                ensure!(
                    self.native.len().is_multiple_of(2),
                    "Invalid native clipboard path encoding"
                );
                let units = self
                    .native
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|u| u16::from_le_bytes(*u))
                    .collect::<Vec<_>>();
                ensure!(
                    !units.contains(&0) && self.display == String::from_utf16_lossy(&units),
                    "Invalid clipboard file display"
                );
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipboardSnapshot {
    Formats { formats: Vec<String> },
    Text { text: String },
    Files { files: Vec<ClipboardFileReference> },
}
impl std::fmt::Debug for ClipboardSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Formats { formats } => f
                .debug_struct("ClipboardFormats")
                .field("count", &formats.len())
                .finish(),
            Self::Text { text } => f
                .debug_struct("ClipboardText")
                .field("bytes", &text.len())
                .finish(),
            Self::Files { files } => f
                .debug_struct("ClipboardFiles")
                .field("count", &files.len())
                .finish(),
        }
    }
}
impl ClipboardSnapshot {
    pub fn kind(&self) -> ClipboardInspectKind {
        match self {
            Self::Formats { .. } => ClipboardInspectKind::Formats,
            Self::Text { .. } => ClipboardInspectKind::Text,
            Self::Files { .. } => ClipboardInspectKind::Files,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Formats { formats } => {
                ensure!(
                    formats.len() <= CLIPBOARD_FORMATS,
                    "Too many clipboard formats"
                );
                ensure!(
                    formats.iter().all(|v| !v.is_empty()
                        && v.len() <= CLIPBOARD_FORMAT_BYTES
                        && !v.contains('\0')),
                    "Oversized or invalid clipboard format"
                );
            }
            Self::Text { text } => ensure!(
                text.len() <= CLIPBOARD_TEXT_BYTES && !text.contains('\0'),
                "Clipboard text exceeds64 KiB or contains a NUL"
            ),
            Self::Files { files } => {
                ensure!(
                    files.len() <= CLIPBOARD_FILE_COUNT,
                    "Too many clipboard files"
                );
                let mut total = 0usize;
                for file in files {
                    file.validate()?;
                    total = total
                        .checked_add(file.native.len())
                        .ok_or_else(|| anyhow::anyhow!("Clipboard reference size overflow"))?;
                }
                ensure!(
                    total <= CLIPBOARD_FILES_BYTES,
                    "Clipboard file references exceed64 KiB"
                );
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 1024 * 1024,
            "Clipboard metadata exceeds its packet bound"
        );
        Ok(())
    }
}
pub(crate) fn uri_files(bytes: &[u8]) -> Result<ClipboardSnapshot> {
    ensure!(
        bytes.len() <= CLIPBOARD_FILES_BYTES,
        "Clipboard file offer exceeds64 KiB"
    );
    let text = std::str::from_utf8(bytes)?;
    let mut files = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        ensure!(
            files.len() < CLIPBOARD_FILE_COUNT && line.len() <= CLIPBOARD_FILE_BYTES * 3,
            "Too many or oversized clipboard file references"
        );
        let uri = reqwest::Url::parse(line)?;
        ensure!(
            uri.scheme() == "file"
                && uri.username().is_empty()
                && uri.password().is_none()
                && uri.query().is_none()
                && uri.fragment().is_none(),
            "Clipboard file offer must contain file URLs"
        );
        let path = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("Unsupported clipboard file URL"))?;
        files.push(ClipboardFileReference::from_path(&path)?);
    }
    let value = ClipboardSnapshot::Files { files };
    value.validate()?;
    Ok(value)
}
#[cfg(any(target_os = "windows", test))]
pub(crate) fn drop_files(bytes: &[u8]) -> Result<ClipboardSnapshot> {
    ensure!(
        (22..=CLIPBOARD_FILES_BYTES + 20).contains(&bytes.len()),
        "Invalid or oversized clipboard file offer"
    );
    let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    ensure!(
        offset >= 20
            && offset <= bytes.len() - 2
            && wide != 0
            && (bytes.len() - offset).is_multiple_of(2),
        "Clipboard files require a valid UTF-16 DROPFILES offer"
    );
    let mut files = Vec::new();
    let mut units = Vec::new();
    let mut ended = false;
    for pair in bytes[offset..].as_chunks::<2>().0 {
        let unit = u16::from_le_bytes(*pair);
        if unit == 0 {
            if units.is_empty() {
                ended = true;
                break;
            }
            ensure!(
                files.len() < CLIPBOARD_FILE_COUNT,
                "Too many clipboard files"
            );
            files.push(ClipboardFileReference::from_utf16(&units)?);
            units.clear();
        } else {
            ensure!(
                units.len() < CLIPBOARD_FILE_BYTES / 2,
                "Clipboard file reference is too long"
            );
            units.push(unit);
        }
    }
    ensure!(
        ended && units.is_empty(),
        "Unterminated clipboard file list"
    );
    let value = ClipboardSnapshot::Files { files };
    value.validate()?;
    Ok(value)
}

#[path = "clipboard_input_budget.rs"]
mod budget;
/// Holds the existing probed +96 MiB native acquisition ceiling in an owned
/// authenticated receiver process. It never opens a clipboard or raises decode limits.
#[cfg(target_os = "windows")]
pub struct NativeAcquisitionBudget {
    _resource: budget::Budget,
}
#[cfg(target_os = "windows")]
pub fn native_acquisition_budget() -> Result<NativeAcquisitionBudget> {
    Ok(NativeAcquisitionBudget {
        _resource: budget::Budget::enter()?,
    })
}
#[cfg(target_os = "linux")]
#[path = "clipboard_input_x11.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "clipboard_input_windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "clipboard_input_macos.rs"]
mod platform;

fn admit_backend_context(
    backend: NativeClipboardBackend,
) -> std::result::Result<(), crate::CaptureError> {
    #[cfg(target_os = "linux")]
    if backend == NativeClipboardBackend::Automatic && crate::linux_wayland_session() {
        return Err(crate::CaptureError::Clipboard);
    }
    let _ = backend;
    Ok(())
}
pub fn capture_clipboard_image_with_cancel(
    backend: NativeClipboardBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> std::result::Result<crate::CapturedImage, crate::CaptureError> {
    check()?;
    admit_backend_context(backend)?;
    let call = crate::NativeCaptureCall::ClipboardImage { backend };
    let executor = crate::native_capture_job::executor().ok_or(
        crate::CaptureError::Unsupported("Clipboard import requires the isolated desktop executor"),
    )?;
    let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
    call.validate_reply(&reply)?;
    check()?;
    let crate::NativeCaptureReply::Frames(mut frames) = reply else {
        return Err(crate::CaptureError::Backend(
            "Invalid clipboard image reply".into(),
        ));
    };
    frames.pop().ok_or(crate::CaptureError::EmptyFrame)
}
pub fn read_clipboard_image() -> Result<image::RgbaImage> {
    let frame = capture_clipboard_image_with_cancel(NativeClipboardBackend::Automatic, &|| Ok(()))?;
    Arc::try_unwrap(frame.pixels).map_err(|_| anyhow::anyhow!("Clipboard pixels are still shared"))
}
pub fn inspect_clipboard_with_cancel(
    kind: ClipboardInspectKind,
    backend: NativeClipboardBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> std::result::Result<ClipboardSnapshot, crate::CaptureError> {
    check()?;
    admit_backend_context(backend)?;
    let call = crate::NativeCaptureCall::ClipboardInspect { kind, backend };
    let executor =
        crate::native_capture_job::executor().ok_or(crate::CaptureError::Unsupported(
            "Clipboard inspection requires the isolated desktop executor",
        ))?;
    let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
    call.validate_reply(&reply)?;
    check()?;
    let crate::NativeCaptureReply::Clipboard(value) = reply else {
        return Err(crate::CaptureError::Backend(
            "Invalid clipboard metadata reply".into(),
        ));
    };
    Ok(value)
}
pub(crate) fn image_inline(
    backend: NativeClipboardBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> std::result::Result<crate::CapturedImage, crate::CaptureError> {
    let started = Instant::now();
    check()?;
    let deadline = started + Duration::from_secs(4);
    let mut resource = budget::Budget::enter().map_err(failure)?;
    let pixels = platform::image(backend, deadline, check, &mut resource);
    check()?;
    let pixels = pixels.map_err(failure)?;
    crate::validate_image_size(pixels.width(), pixels.height())?;
    Ok(crate::CapturedImage {
        desktop_layout: None,
        display_id: "clipboard:image".into(),
        window_id: None,
        width: pixels.width(),
        height: pixels.height(),
        png_path: PathBuf::new(),
        pixels: Arc::new(pixels),
        elapsed_ms: started.elapsed().as_millis(),
        source_format: crate::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    })
}
pub(crate) fn inspect_inline(
    kind: ClipboardInspectKind,
    backend: NativeClipboardBackend,
    check: &crate::NativeCaptureCheck<'_>,
) -> std::result::Result<ClipboardSnapshot, crate::CaptureError> {
    let mut resource = budget::Budget::enter().map_err(failure)?;
    let value = platform::inspect(
        kind,
        backend,
        Instant::now() + Duration::from_secs(4),
        check,
        &mut resource,
    )
    .map_err(failure)?;
    value.validate().map_err(failure)?;
    check()?;
    Ok(value)
}
pub(crate) fn native_drop_inline(
    source: &crate::NativeDropSource,
    image: bool,
    originating_pid: u32,
    check: &crate::NativeCaptureCheck<'_>,
) -> std::result::Result<crate::NativeDropReply, crate::CaptureError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        source.validate()?;
        let _resource = budget::Budget::enter().map_err(failure)?;
        let result = platform::native_drop(
            source,
            image,
            originating_pid,
            Instant::now() + Duration::from_secs(4),
            check,
        )
        .map_err(failure)?;
        result.validate(source, image).map_err(failure)?;
        check()?;
        Ok(result)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, image, originating_pid, check);
        Err(crate::CaptureError::Unsupported(
            "Unix native drop acquisition requires Unix",
        ))
    }
}

fn failure(error: anyhow::Error) -> crate::CaptureError {
    use crate::CaptureError;
    match error.downcast_ref::<CaptureError>() {
        Some(CaptureError::ImageColorPolicy) => CaptureError::ImageColorPolicy,
        Some(CaptureError::NativeCanceled | CaptureError::Canceled) => CaptureError::NativeCanceled,
        Some(CaptureError::NativeTimeout) => CaptureError::NativeTimeout,
        _ => CaptureError::Clipboard,
    }
}

#[cfg(any(target_os = "windows", test))]
fn registered_format_label(name: &[u16], count: i32) -> Result<Option<String>> {
    if count == 0 {
        return Ok(None);
    }
    ensure!(
        count > 0 && count as usize <= CLIPBOARD_FORMAT_BYTES,
        "Clipboard format name is too long"
    );
    let count = count as usize;
    ensure!(
        name.get(count) == Some(&0),
        "Clipboard format name has no terminator"
    );
    let label = String::from_utf16(&name[..count])?;
    ensure!(
        label.len() <= CLIPBOARD_FORMAT_BYTES,
        "Clipboard format label exceeds its UTF-8 bound"
    );
    Ok(Some(label))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_hdr_paste_fixture_preserves_static_color_policy_classification() {
        let bytes = include_bytes!("../tests/fixtures/color/hdr-pq-cicp.png");
        let error =
            crate::decode_editor_image_bytes_with_backing_and_cancel(bytes, bytes.len(), &|| false)
                .unwrap_err();
        let error = failure(error);
        assert!(matches!(error, crate::CaptureError::ImageColorPolicy));
        assert!(error.to_string().contains("SDR sRGB"));
        let canceled =
            crate::decode_editor_image_bytes_with_backing_and_cancel(bytes, bytes.len(), &|| true)
                .unwrap_err();
        assert!(matches!(
            failure(canceled),
            crate::CaptureError::NativeCanceled
        ));
    }
    #[test]
    fn snapshots_are_bounded_private_metadata_and_never_debug_content() -> Result<()> {
        let value = ClipboardSnapshot::Text {
            text: "private-synthetic-value".into(),
        };
        value.validate()?;
        assert!(!format!("{value:?}").contains("private-synthetic-value"));
        assert!(
            ClipboardSnapshot::Text {
                text: "x".repeat(CLIPBOARD_TEXT_BYTES + 1)
            }
            .validate()
            .is_err()
        );
        assert!(
            ClipboardSnapshot::Formats {
                formats: vec!["x".into(); CLIPBOARD_FORMATS + 1]
            }
            .validate()
            .is_err()
        );
        assert!(
            ClipboardSnapshot::Formats {
                formats: vec!["x".repeat(CLIPBOARD_FORMAT_BYTES + 1)]
            }
            .validate()
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn file_list_parser_retains_native_utf16_and_enforces_termination_and_extent() -> Result<()> {
        let units = [
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
            b'.' as u16,
            b'p' as u16,
            b'n' as u16,
            b'g' as u16,
        ];
        let mut bytes = vec![0u8; 20];
        bytes[..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        for unit in units.into_iter().chain([0, 0]) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let ClipboardSnapshot::Files { files } = drop_files(&bytes)? else {
            unreachable!()
        };
        assert_eq!(files.len(), 1);
        assert!(files[0].native.windows(2).any(|v| v == [0, 0xd8]));
        assert!(drop_files(&bytes[..bytes.len() - 2]).is_err());
        bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(drop_files(&bytes).is_err());
        Ok(())
    }
    #[test]
    fn file_uri_list_never_opens_referenced_files_and_rejects_remote_content() {
        // A file URL needs a drive letter to map to a Windows path.
        #[cfg(windows)]
        let missing: &[u8] = b"file:///C:/definitely-nonexistent/input.png\r\n";
        #[cfg(not(windows))]
        let missing: &[u8] = b"file:///definitely-nonexistent/input.png\r\n";
        assert!(uri_files(missing).is_ok());
        assert!(uri_files(b"https://example.invalid/image.png\r\n").is_err());
        assert!(uri_files(&vec![b'x'; CLIPBOARD_FILES_BYTES + 1]).is_err());
    }
    #[test]
    fn registered_format_name_rejects_native_truncation_and_utf8_overflow() {
        // Model GetClipboardFormatNameW: it copies at most buffer_len-1 units
        // and reports the copied length, excluding its null terminator.
        for length in [128, 129, 256] {
            let mut name = [0u16; CLIPBOARD_FORMAT_BYTES + 2];
            let copied = length.min(name.len() - 1);
            name[..copied].fill(b'x' as u16);
            let result = registered_format_label(&name, copied as i32);
            assert_eq!(result.is_ok(), length == 128);
        }
        let mut name = [0u16; CLIPBOARD_FORMAT_BYTES + 2];
        name[..128].fill(0x4e00);
        assert!(registered_format_label(&name, 128).is_err());
        name[128] = b'x' as u16;
        assert!(registered_format_label(&name, 128).is_err());
        assert_eq!(registered_format_label(&name, 0).unwrap(), None);
    }
}

#[cfg(all(target_os = "macos", feature = "native-file-clipboard-fixture"))]
#[doc(hidden)]
pub fn inspect_owned_macos_file_pasteboard(name: &str) -> Result<ClipboardSnapshot> {
    platform::inspect_owned_named_files(name)
}
