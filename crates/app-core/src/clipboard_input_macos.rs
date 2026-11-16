//! Main-thread Pasteboard Manager input in the bounded isolated worker.
use super::*;
use std::ffi::{CString, c_void};
type Cf = *const c_void;
type Board = *mut c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: Cf, bytes: *const i8, encoding: u32) -> Cf;
    fn CFStringGetLength(string: Cf) -> isize;
    fn CFStringGetCString(string: Cf, buffer: *mut i8, size: isize, encoding: u32) -> u8;
    fn CFArrayGetCount(array: Cf) -> isize;
    fn CFArrayGetValueAtIndex(array: Cf, index: isize) -> Cf;
    fn CFDataGetLength(data: Cf) -> isize;
    fn CFDataGetBytePtr(data: Cf) -> *const u8;
    fn CFRelease(object: Cf);
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn PasteboardCreate(name: Cf, board: *mut Board) -> i32;
    fn PasteboardSynchronize(board: Board) -> u32;
    fn PasteboardGetItemCount(board: Board, count: *mut u32) -> i32;
    fn PasteboardGetItemIdentifier(board: Board, index: u32, item: *mut usize) -> i32;
    fn PasteboardCopyItemFlavors(board: Board, item: usize, flavors: *mut Cf) -> i32;
    fn PasteboardGetItemFlavorFlags(board: Board, item: usize, flavor: Cf, flags: *mut u32) -> i32;
    fn PasteboardCopyItemFlavorData(board: Board, item: usize, flavor: Cf, data: *mut Cf) -> i32;
}
unsafe extern "C" {
    fn pthread_main_np() -> i32;
}
struct Owned(Cf);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}
fn string(value: &str) -> Result<Owned> {
    let bytes = CString::new(value)?;
    let value = unsafe { CFStringCreateWithCString(std::ptr::null(), bytes.as_ptr(), 0x08000100) };
    ensure!(!value.is_null(), "Could not prepare pasteboard type");
    Ok(Owned(value))
}
fn label(value: Cf) -> Result<String> {
    ensure!(!value.is_null(), "Invalid pasteboard type");
    let length = unsafe { CFStringGetLength(value) };
    ensure!(
        length > 0 && length <= CLIPBOARD_FORMAT_BYTES as isize,
        "Pasteboard type label exceeds its bound"
    );
    let mut bytes = [0u8; CLIPBOARD_FORMAT_BYTES + 1];
    ensure!(
        unsafe {
            CFStringGetCString(
                value,
                bytes.as_mut_ptr().cast(),
                bytes.len() as isize,
                0x08000100,
            )
        } != 0,
        "Pasteboard type label exceeds its UTF-8 bound"
    );
    let length = bytes
        .iter()
        .position(|v| *v == 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid pasteboard label"))?;
    Ok(std::str::from_utf8(&bytes[..length])?.to_owned())
}
struct Session {
    board: Owned,
    deadline: Instant,
}
impl Session {
    fn general(backend: NativeClipboardBackend, deadline: Instant) -> Result<Self> {
        ensure!(
            backend == NativeClipboardBackend::Automatic,
            "Focused X11 clipboard policy requires X11"
        );
        Self::named("com.apple.pasteboard.clipboard", deadline)
    }
    fn named(name: &str, deadline: Instant) -> Result<Self> {
        ensure!(
            unsafe { pthread_main_np() } != 0,
            "Native pasteboard access requires the worker main thread"
        );
        let name = string(name)?;
        let mut board = std::ptr::null_mut();
        ensure!(
            unsafe { PasteboardCreate(name.0, &mut board) } == 0 && !board.is_null(),
            "Pasteboard unavailable"
        );
        unsafe {
            PasteboardSynchronize(board);
        }
        Ok(Self {
            board: Owned(board.cast()),
            deadline,
        })
    }
    fn board(&self) -> Board {
        self.board.0.cast_mut()
    }
    fn check(&self, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
        check()?;
        ensure!(
            Instant::now() < self.deadline,
            "Pasteboard deadline exceeded"
        );
        ensure!(
            unsafe { PasteboardSynchronize(self.board()) } & 1 == 0,
            "Pasteboard changed during transfer"
        );
        Ok(())
    }
    fn items(&self, check: &crate::NativeCaptureCheck<'_>) -> Result<Vec<usize>> {
        self.check(check)?;
        let mut count = 0;
        ensure!(
            unsafe { PasteboardGetItemCount(self.board(), &mut count) } == 0
                && count as usize <= CLIPBOARD_FILE_COUNT,
            "Pasteboard item count exceeds its bound"
        );
        let mut items = Vec::with_capacity(count as usize);
        for index in 1..=count {
            let mut item = 0;
            ensure!(
                unsafe { PasteboardGetItemIdentifier(self.board(), index, &mut item) } == 0,
                "Could not inspect pasteboard item"
            );
            items.push(item);
        }
        Ok(items)
    }
    fn has(&self, item: usize, name: &str) -> Result<bool> {
        let name = string(name)?;
        let mut flags = 0;
        Ok(unsafe { PasteboardGetItemFlavorFlags(self.board(), item, name.0, &mut flags) } == 0)
    }
    fn data(
        &self,
        item: usize,
        name: &str,
        max: usize,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<u8>> {
        self.check(check)?;
        let name = string(name)?;
        let mut raw = std::ptr::null();
        ensure!(
            unsafe { PasteboardCopyItemFlavorData(self.board(), item, name.0, &mut raw) } == 0
                && !raw.is_null(),
            "Pasteboard data unavailable"
        );
        let data = Owned(raw);
        let length = unsafe { CFDataGetLength(data.0) };
        ensure!(
            length >= 0 && length as usize <= max,
            "Pasteboard data exceeds its admitted extent"
        );
        let pointer = unsafe { CFDataGetBytePtr(data.0) };
        ensure!(length == 0 || !pointer.is_null(), "Invalid pasteboard data");
        let bytes = if length == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(pointer, length as usize) }.to_vec()
        };
        self.check(check)?;
        Ok(bytes)
    }
}
pub(super) fn inspect(
    kind: ClipboardInspectKind,
    backend: NativeClipboardBackend,
    deadline: Instant,
    check: &crate::NativeCaptureCheck<'_>,
    _: &mut budget::Budget,
) -> Result<ClipboardSnapshot> {
    let session = Session::general(backend, deadline)?;
    let items = session.items(check)?;
    let value = match kind {
        ClipboardInspectKind::Formats => {
            let mut formats = Vec::new();
            for item in items {
                session.check(check)?;
                let mut raw = std::ptr::null();
                ensure!(
                    unsafe { PasteboardCopyItemFlavors(session.board(), item, &mut raw) } == 0
                        && !raw.is_null(),
                    "Pasteboard types unavailable"
                );
                let array = Owned(raw);
                let count = unsafe { CFArrayGetCount(array.0) };
                ensure!(
                    count >= 0 && count as usize <= CLIPBOARD_FORMATS,
                    "Too many pasteboard types"
                );
                for index in 0..count {
                    let name = label(unsafe { CFArrayGetValueAtIndex(array.0, index) })?;
                    if !formats.contains(&name) {
                        ensure!(
                            formats.len() < CLIPBOARD_FORMATS,
                            "Too many pasteboard types"
                        );
                        formats.push(name);
                    }
                }
            }
            ClipboardSnapshot::Formats { formats }
        }
        ClipboardInspectKind::Text => {
            ensure!(items.len() == 1, "Paste text requires one clipboard item");
            let item = items[0];
            let text = if session.has(item, "public.utf8-plain-text")? {
                String::from_utf8(session.data(
                    item,
                    "public.utf8-plain-text",
                    CLIPBOARD_TEXT_BYTES,
                    check,
                )?)?
            } else {
                let name = if session.has(item, "public.utf16-external-plain-text")? {
                    "public.utf16-external-plain-text"
                } else {
                    "public.utf16-plain-text"
                };
                let bytes = session.data(item, name, CLIPBOARD_TEXT_BYTES * 2 + 2, check)?;
                ensure!(
                    bytes.len().is_multiple_of(2),
                    "Invalid pasteboard UTF-16 extent"
                );
                let big = bytes.starts_with(&[0xfe, 0xff]);
                let offset = usize::from(big || bytes.starts_with(&[0xff, 0xfe])) * 2;
                let units = bytes[offset..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|u| {
                        if big {
                            u16::from_be_bytes(*u)
                        } else {
                            u16::from_le_bytes(*u)
                        }
                    })
                    .collect::<Vec<_>>();
                String::from_utf16(&units)?
            };
            ClipboardSnapshot::Text { text }
        }
        ClipboardInspectKind::Files => {
            let mut files = Vec::new();
            for item in items {
                ensure!(
                    session.has(item, "public.file-url")?,
                    "Pasteboard item is not a file reference"
                );
                let mut bytes =
                    session.data(item, "public.file-url", CLIPBOARD_FILE_BYTES * 3 + 1, check)?;
                if bytes.last() == Some(&0) {
                    bytes.pop();
                }
                let ClipboardSnapshot::Files { files: mut next } = uri_files(&bytes)? else {
                    unreachable!()
                };
                ensure!(next.len() == 1, "Pasteboard file item must name one file");
                files.append(&mut next);
            }
            ClipboardSnapshot::Files { files }
        }
    };
    session.check(check)?;
    value.validate()?;
    Ok(value)
}
pub(super) fn image(
    backend: NativeClipboardBackend,
    deadline: Instant,
    check: &crate::NativeCaptureCheck<'_>,
    budget: &mut budget::Budget,
) -> Result<image::RgbaImage> {
    let session = Session::general(backend, deadline)?;
    let items = session.items(check)?;
    ensure!(items.len() == 1, "Paste image requires one clipboard item");
    let mut selected = None;
    for name in [
        "public.png",
        "public.jpeg",
        "org.webmproject.webp",
        "com.microsoft.bmp",
        "public.tiff",
    ] {
        if session.has(items[0], name)? {
            selected = Some(name);
            break;
        }
    }
    let name =
        selected.ok_or_else(|| anyhow::anyhow!("Clipboard contains no supported image type"))?;
    let bytes = session.data(items[0], name, CLIPBOARD_ENCODED_BYTES, check)?;
    drop(session);
    budget.decoding()?;
    check()?;
    crate::decode_editor_image_bytes(&bytes)
}
