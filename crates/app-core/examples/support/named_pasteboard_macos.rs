//! Test-only lifetime reference; no content is read, written, cleared or fabricated here.
use anyhow::ensure;
use std::ffi::{CString, c_void};
type Cf = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(allocator: Cf, bytes: *const i8, encoding: u32) -> Cf;
    fn CFRelease(value: Cf);
}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn PasteboardCreate(name: Cf, board: *mut Cf) -> i32;
}
unsafe extern "C" {
    fn pthread_main_np() -> i32;
}
pub struct Retained(Cf);
impl Retained {
    pub fn new(name: &str) -> anyhow::Result<Self> {
        let suffix = name
            .strip_prefix("screenshots.owned-image-text-fixture.")
            .or_else(|| name.strip_prefix("screenshots.owned-file-fixture."))
            .ok_or_else(|| anyhow::anyhow!("Only a private fixture pasteboard is admitted"))?;
        uuid::Uuid::parse_str(suffix)?;
        ensure!(
            unsafe { pthread_main_np() } != 0,
            "Private board requires the native main thread"
        );
        let bytes = CString::new(name)?;
        let name =
            unsafe { CFStringCreateWithCString(std::ptr::null(), bytes.as_ptr(), 0x08000100) };
        ensure!(!name.is_null(), "Private board name allocation failed");
        let mut board = Self(std::ptr::null());
        let status = unsafe { PasteboardCreate(name, &mut board.0) };
        unsafe {
            CFRelease(name);
        }
        ensure!(
            status == 0 && !board.0.is_null(),
            "Private board lifetime reference unavailable"
        );
        Ok(board)
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}
