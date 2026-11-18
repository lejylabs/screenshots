//! Immediate file-URL pasteboard data, always on the isolated native main thread.
use anyhow::{Result, ensure};
use std::ffi::{CString, c_void};
type Cf = *const c_void;
type Board = *mut c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: Cf, bytes: *const i8, encoding: u32) -> Cf;
    fn CFDataCreate(allocator: Cf, bytes: *const u8, length: isize) -> Cf;
    fn CFRelease(object: Cf);
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn PasteboardCreate(name: Cf, board: *mut Board) -> i32;
    fn PasteboardClear(board: Board) -> i32;
    fn PasteboardPutItemFlavor(board: Board, item: usize, flavor: Cf, data: Cf, flags: u32) -> i32;
}
unsafe extern "C" {
    fn pthread_main_np() -> i32;
}
struct Owned(Cf);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
fn string(value: &str) -> Result<Owned> {
    let value = CString::new(value)?;
    let result = unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x08000100) };
    ensure!(!result.is_null(), "Could not prepare file pasteboard type");
    Ok(Owned(result))
}
pub(super) fn publish(uri: &str, name: &str) -> Result<()> {
    ensure!(
        uri.len() <= app_core::CLIPBOARD_FILE_BYTES * 3 && uri.starts_with("file:///"),
        "Invalid file pasteboard URL"
    );
    publish_bytes(uri.as_bytes(), "public.file-url", name)
}
pub(super) fn publish_bytes(bytes: &[u8], flavor: &str, name: &str) -> Result<()> {
    ensure!(
        unsafe { pthread_main_np() } != 0,
        "Pasteboard requires the native main thread"
    );
    let bound = match flavor {
        "public.file-url" => app_core::CLIPBOARD_FILE_BYTES * 3,
        "public.png" => app_core::MAX_CAPTURE_BYTES as usize + 16 * 1024 * 1024,
        "public.utf8-plain-text" => app_core::MAX_CLIPBOARD_TEXT_BYTES,
        _ => anyhow::bail!("Unsupported pasteboard output type"),
    };
    ensure!(bytes.len() <= bound, "Pasteboard output exceeds its bound");
    // Prepare every bounded native data object before the irreversible clear.
    let name = string(name)?;
    let flavor = string(flavor)?;
    let data = unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
    ensure!(!data.is_null(), "Could not prepare file pasteboard data");
    let data = Owned(data);
    let mut board = std::ptr::null_mut();
    ensure!(
        unsafe { PasteboardCreate(name.0, &mut board) } == 0 && !board.is_null(),
        "File pasteboard unavailable"
    );
    let _board = Owned(board.cast());
    ensure!(
        unsafe { PasteboardClear(board) } == 0,
        "Could not clear file pasteboard"
    );
    ensure!(
        unsafe { PasteboardPutItemFlavor(board, 1, flavor.0, data.0, 0) } == 0,
        "Could not publish file pasteboard data; content may already have changed"
    );
    Ok(())
}
