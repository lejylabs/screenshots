//! Bounded explicit Win32 clipboard reads, in the isolated native worker only.
use super::*;
use windows_sys::Win32::System::{DataExchange::*, Memory::*};
struct Open;
impl Open {
    fn new() -> Result<Self> {
        ensure!(
            unsafe { OpenClipboard(std::ptr::null_mut()) } != 0,
            "Clipboard is busy or unavailable"
        );
        Ok(Self)
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
fn data(format: u32, max: usize) -> Result<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) };
    ensure!(!handle.is_null(), "Clipboard requested format unavailable");
    let length = unsafe { GlobalSize(handle) };
    ensure!(
        length > 0 && length <= max,
        "Clipboard native payload exceeds its transfer bound"
    );
    let pointer = unsafe { GlobalLock(handle) };
    ensure!(!pointer.is_null(), "Could not lock clipboard payload");
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }.to_vec();
    unsafe {
        GlobalUnlock(handle);
    }
    Ok(bytes)
}
fn validate_backend(backend: NativeClipboardBackend) -> Result<()> {
    ensure!(
        backend == NativeClipboardBackend::Automatic,
        "Focused X11 clipboard policy requires an X11 application"
    );
    Ok(())
}
fn check(deadline: Instant, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
    check()?;
    ensure!(
        Instant::now() < deadline,
        "Clipboard read deadline exceeded"
    );
    Ok(())
}
pub(super) fn inspect(
    kind: ClipboardInspectKind,
    backend: NativeClipboardBackend,
    deadline: Instant,
    callback: &crate::NativeCaptureCheck<'_>,
    _: &mut budget::Budget,
) -> Result<ClipboardSnapshot> {
    validate_backend(backend)?;
    check(deadline, callback)?;
    let _open = Open::new()?;
    let value = match kind {
        ClipboardInspectKind::Formats => {
            let mut format = 0;
            let mut formats = Vec::new();
            loop {
                check(deadline, callback)?;
                unsafe {
                    windows_sys::Win32::Foundation::SetLastError(0);
                }
                format = unsafe { EnumClipboardFormats(format) };
                if format == 0 {
                    ensure!(
                        unsafe { windows_sys::Win32::Foundation::GetLastError() } == 0,
                        "Could not enumerate clipboard formats"
                    );
                    break;
                }
                ensure!(
                    formats.len() < CLIPBOARD_FORMATS,
                    "Clipboard has too many formats"
                );
                let standard = match format {
                    1 => Some("CF_TEXT"),
                    2 => Some("CF_BITMAP"),
                    3 => Some("CF_METAFILEPICT"),
                    4 => Some("CF_SYLK"),
                    5 => Some("CF_DIF"),
                    6 => Some("CF_TIFF"),
                    7 => Some("CF_OEMTEXT"),
                    8 => Some("CF_DIB"),
                    9 => Some("CF_PALETTE"),
                    10 => Some("CF_PENDATA"),
                    11 => Some("CF_RIFF"),
                    12 => Some("CF_WAVE"),
                    13 => Some("CF_UNICODETEXT"),
                    14 => Some("CF_ENHMETAFILE"),
                    15 => Some("CF_HDROP"),
                    16 => Some("CF_LOCALE"),
                    17 => Some("CF_DIBV5"),
                    _ => None,
                };
                let label = if let Some(label) = standard {
                    label.to_owned()
                } else {
                    // One extra copied unit distinguishes an oversized/truncated
                    // name from an exactly 128-unit name plus its terminator.
                    let mut name = [0u16; CLIPBOARD_FORMAT_BYTES + 2];
                    let count = unsafe {
                        GetClipboardFormatNameW(format, name.as_mut_ptr(), name.len() as i32)
                    };
                    registered_format_label(&name, count)?
                        .unwrap_or_else(|| format!("native-format-{format}"))
                };
                formats.push(label);
            }
            ClipboardSnapshot::Formats { formats }
        }
        ClipboardInspectKind::Text => {
            let bytes = data(13, CLIPBOARD_TEXT_BYTES * 2 + 2)?;
            ensure!(
                bytes.len().is_multiple_of(2),
                "Invalid clipboard Unicode text extent"
            );
            let units = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes(*v))
                .take_while(|v| *v != 0)
                .collect::<Vec<_>>();
            ensure!(
                units.len() * 2 < bytes.len(),
                "Clipboard text has no terminator"
            );
            let text = String::from_utf16(&units)?;
            ensure!(
                text.len() <= CLIPBOARD_TEXT_BYTES,
                "Clipboard UTF-8 text exceeds64 KiB"
            );
            ClipboardSnapshot::Text { text }
        }
        ClipboardInspectKind::Files => drop_files(&data(15, CLIPBOARD_FILES_BYTES + 20)?)?,
    };
    check(deadline, callback)?;
    value.validate()?;
    Ok(value)
}
pub(super) fn image(
    backend: NativeClipboardBackend,
    deadline: Instant,
    callback: &crate::NativeCaptureCheck<'_>,
    budget: &mut budget::Budget,
) -> Result<image::RgbaImage> {
    validate_backend(backend)?;
    check(deadline, callback)?;
    let (bytes, png) = crate::clipboard_windows::read_payload_with_budget(&mut |png, _| {
        if !png {
            budget.dib_acquisition()?;
        }
        Ok(())
    })?;
    check(deadline, callback)?;
    budget.decoding()?;
    crate::clipboard_windows::decode_payload(&bytes, png)
}
