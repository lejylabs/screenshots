//! Immediate file-URL pasteboard data, always on the isolated native main thread.
use anyhow::{Result, ensure};
use std::ffi::{CString, c_void};
type Cf = *const c_void;
type Board = *mut c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: Cf, bytes: *const i8, encoding: u32) -> Cf;
    fn CFDataCreate(allocator: Cf, bytes: *const u8, length: isize) -> Cf;
    fn CFDataCreateWithBytesNoCopy(
        allocator: Cf,
        bytes: *const u8,
        length: isize,
        deallocator: Cf,
    ) -> Cf;
    fn CFDataGetBytePtr(data: Cf) -> *const u8;
    fn CFDataGetLength(data: Cf) -> isize;
    static kCFAllocatorMalloc: Cf;
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
    fn malloc_size(pointer: *const c_void) -> usize;
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
    publish_data(Owned(data), flavor.0, name.0)
}
fn publish_data(data: Owned, flavor: Cf, name: Cf) -> Result<()> {
    let mut board = std::ptr::null_mut();
    ensure!(
        unsafe { PasteboardCreate(name, &mut board) } == 0 && !board.is_null(),
        "File pasteboard unavailable"
    );
    let _board = Owned(board.cast());
    ensure!(
        unsafe { PasteboardClear(board) } == 0,
        "Could not clear file pasteboard"
    );
    ensure!(
        unsafe { PasteboardPutItemFlavor(board, 1, flavor, data.0, 0) } == 0,
        "Could not publish file pasteboard data; content may already have changed"
    );
    Ok(())
}

/// One native backing buffer; its actual usable allocation is checked, never flattened to Vec.
pub(super) struct ImageData {
    bytes: *mut u8,
    length: usize,
    limit: usize,
    allocation: usize,
}
impl ImageData {
    pub(super) fn new(limit: usize) -> Result<Self> {
        ensure!(
            limit > 0 && limit <= app_core::MAX_CAPTURE_BYTES as usize + 16 * 1024 * 1024,
            "Invalid native PNG preparation bound"
        );
        let bytes = unsafe { libc::malloc(limit) }.cast::<u8>();
        ensure!(!bytes.is_null(), "Native PNG backing allocation failed");
        let value = Self {
            bytes,
            length: 0,
            limit,
            allocation: unsafe { malloc_size(bytes.cast()) },
        };
        ensure!(
            value.allocation >= limit && value.allocation <= limit + 64 * 1024,
            "Native PNG backing allocation exceeds its admitted extent"
        );
        Ok(value)
    }
    pub(super) fn append(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(
            self.length
                .checked_add(bytes.len())
                .is_some_and(|n| n <= self.limit),
            "Native PNG stream exceeds its admitted extent"
        );
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.bytes.add(self.length), bytes.len());
        }
        self.length += bytes.len();
        Ok(())
    }
    pub(super) fn header(&self) -> Result<[u8; 33]> {
        ensure!(self.length >= 33, "Native PNG header is incomplete");
        let mut header = [0; 33];
        unsafe {
            std::ptr::copy_nonoverlapping(self.bytes, header.as_mut_ptr(), header.len());
        }
        Ok(header)
    }
    pub(super) fn allocation_bytes(&self) -> usize {
        self.allocation
    }
    pub(super) fn finish(mut self) -> Result<PreparedImage> {
        ensure!(
            unsafe { pthread_main_np() } != 0,
            "Pasteboard requires the native main thread"
        );
        let header = self.header()?;
        ensure!(
            &header[..8] == b"\x89PNG\r\n\x1a\n"
                && &header[12..16] == b"IHDR"
                && header[24..26] == [8, 6],
            "Invalid prepared native PNG"
        );
        // Every non-null return owns the input, even when CF chooses to copy or discard it.
        let original = self.bytes;
        let data = unsafe {
            CFDataCreateWithBytesNoCopy(
                std::ptr::null(),
                original,
                self.length as isize,
                kCFAllocatorMalloc,
            )
        };
        ensure!(!data.is_null(), "Native PNG data wrapper unavailable");
        self.bytes = std::ptr::null_mut();
        let data = Owned(data);
        ensure!(
            unsafe { CFDataGetLength(data.0) } == self.length as isize,
            "Native CFData changed the prepared image extent; clipboard publication was not attempted"
        );
        let aliases_backing = unsafe { CFDataGetBytePtr(data.0) } == original.cast_const();
        // Small implementation-selected copies fit the remaining preparation slack; large ones do not.
        ensure!(
            aliases_backing || self.length <= 8 * 1024 * 1024,
            "Native CFData copied a large image backing buffer; clipboard publication was not attempted"
        );
        Ok(PreparedImage { data })
    }
}
pub(super) struct PreparedImage {
    data: Owned,
}
impl PreparedImage {
    pub(super) fn publish(self, name: &str) -> Result<()> {
        let name = string(name)?;
        let flavor = string("public.png")?;
        publish_data(self.data, flavor.0, name.0)
    }
}
impl Drop for ImageData {
    fn drop(&mut self) {
        if !self.bytes.is_null() {
            unsafe {
                libc::free(self.bytes.cast());
            }
        }
    }
}
