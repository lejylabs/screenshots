//! Immediate PNG then DIBV5, with one application-owned native representation at a time.
//! Native calls run only in the existing owned helper/Job and its private window station.
use anyhow::{Context, Result, ensure};
use std::{ffi::c_void, io::Read};
type Handle = *mut c_void;
const MOVEABLE: u32 = 0x0002;
const ZERO_INIT: u32 = 0x0040;
const HEADER: usize = 124;
const ALLOC_SLACK: usize = 64 * 1024;
const DIBV5: u32 = 17;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    fn GlobalReAlloc(memory: Handle, bytes: usize, flags: u32) -> Handle;
    fn GlobalSize(memory: Handle) -> usize;
    fn GlobalLock(memory: Handle) -> *mut c_void;
    fn GlobalUnlock(memory: Handle) -> i32;
    fn GlobalFree(memory: Handle) -> Handle;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
}
#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: *const c_void,
    ) -> Handle;
    fn DestroyWindow(window: Handle) -> i32;
    fn OpenClipboard(window: Handle) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, memory: Handle) -> Handle;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}
struct Global {
    handle: Handle,
    capacity: usize,
}
impl Global {
    fn new(bytes: usize) -> Result<Self> {
        let handle = unsafe { GlobalAlloc(MOVEABLE | ZERO_INIT, bytes) };
        ensure!(!handle.is_null(), "Native clipboard allocation failed");
        let value = Self {
            handle,
            capacity: unsafe { GlobalSize(handle) },
        };
        #[cfg(test)]
        stats::allocated(value.capacity);
        ensure!(
            value.capacity >= bytes && value.capacity <= bytes + ALLOC_SLACK,
            "Native clipboard allocation exceeds its admitted extent"
        );
        Ok(value)
    }
    fn access<T>(&mut self, f: impl FnOnce(&mut [u8]) -> Result<T>) -> Result<T> {
        let pointer = unsafe { GlobalLock(self.handle) };
        ensure!(
            !pointer.is_null(),
            "Native clipboard allocation could not be locked"
        );
        let result =
            f(unsafe { std::slice::from_raw_parts_mut(pointer.cast::<u8>(), self.capacity) });
        let unlocked = unlock(self.handle);
        result.and_then(|value| {
            unlocked?;
            Ok(value)
        })
    }
    fn shrink_in_place(&mut self, length: usize) -> Result<()> {
        ensure!(
            length > 0 && length <= self.capacity,
            "Invalid native PNG shrink extent"
        );
        let before = unsafe { GlobalLock(self.handle) };
        ensure!(
            !before.is_null(),
            "Native PNG could not be locked for shrink"
        );
        // A locked movable block without GMEM_MOVEABLE may only be reallocated in place.
        let resized = unsafe { GlobalReAlloc(self.handle, length, 0) };
        if resized.is_null() {
            unlock(self.handle)?;
            anyhow::bail!("In-place native PNG shrink is unavailable; clipboard was not opened");
        }
        self.handle = resized;
        unlock(self.handle)?;
        let after = unsafe { GlobalLock(self.handle) };
        ensure!(!after.is_null(), "Shrunk native PNG could not be locked");
        let same_address = before == after;
        unlock(self.handle)?;
        let previous_capacity = self.capacity;
        self.capacity = unsafe { GlobalSize(self.handle) };
        #[cfg(test)]
        stats::resized(previous_capacity, self.capacity);
        let _ = previous_capacity;
        ensure!(
            same_address && self.capacity >= length && self.capacity <= length + ALLOC_SLACK,
            "Native PNG could not shrink in place to its encoded extent; clipboard was not opened"
        );
        Ok(())
    }
    fn transfer(&mut self, format: u32) -> Result<()> {
        ensure!(
            !unsafe { SetClipboardData(format, self.handle) }.is_null(),
            "Native clipboard format transfer failed; contents may already have changed"
        );
        // Only a successful SetClipboardData transfers this allocation to the OS.
        self.handle = std::ptr::null_mut();
        #[cfg(test)]
        stats::transferred(format == DIBV5, self.capacity);
        Ok(())
    }
}
fn unlock(handle: Handle) -> Result<()> {
    unsafe {
        SetLastError(0);
    }
    let locked = unsafe { GlobalUnlock(handle) };
    ensure!(
        locked == 0 && unsafe { GetLastError() } == 0,
        "Native clipboard allocation did not fully unlock"
    );
    Ok(())
}
impl Drop for Global {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let freed = unsafe { GlobalFree(self.handle) }.is_null();
            #[cfg(test)]
            stats::freed(freed, self.capacity);
            let _ = freed; // Actual helper reap remains the final authority after native cleanup failure.
        }
    }
}
struct Window(Handle);
impl Window {
    fn new() -> Result<Self> {
        let class = [
            b'S' as u16,
            b'T' as u16,
            b'A' as u16,
            b'T' as u16,
            b'I' as u16,
            b'C' as u16,
            0,
        ];
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                -3isize as Handle,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        ensure!(
            !window.is_null(),
            "Native message-only clipboard owner window is unavailable"
        );
        Ok(Self(window))
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
struct Open(bool);
impl Open {
    fn new(window: &Window) -> Result<Self> {
        ensure!(
            unsafe { OpenClipboard(window.0) } != 0,
            "Native clipboard is busy or unavailable"
        );
        Ok(Self(true))
    }
    fn close(&mut self) -> Result<()> {
        ensure!(
            unsafe { CloseClipboard() } != 0,
            "Native clipboard close failed; contents may already have changed"
        );
        self.0 = false;
        Ok(())
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CloseClipboard();
            }
        }
    }
}

pub(super) struct Prepared {
    png: Global,
    width: u32,
    height: u32,
    rgba_bytes: usize,
}
pub(super) fn prepare(
    reader: &mut impl Read,
    width: u32,
    height: u32,
    rgba_bytes: usize,
) -> Result<Prepared> {
    app_core::validate_image_size(width, height)?;
    ensure!(
        u64::from(width) * u64::from(height) * 4 == rgba_bytes as u64,
        "Invalid native clipboard image extent"
    );
    #[cfg(test)]
    {
        ensure!(
            stats::APP_BYTES.load(std::sync::atomic::Ordering::Acquire) == 0,
            "Another native image preparation still owns data"
        );
        stats::PEAK_APP_BYTES.store(0, std::sync::atomic::Ordering::Release);
    }
    let limit = rgba_bytes
        .checked_add(16 * 1024 * 1024)
        .context("Native PNG extent overflow")?;
    let mut png = Global::new(limit)?;
    checkpoint(1)?;
    let mut length = 0usize;
    loop {
        let mut size = [0; 4];
        reader.read_exact(&mut size)?;
        let size = u32::from_be_bytes(size) as usize;
        if size == 0 {
            break;
        }
        ensure!(
            size <= 64 * 1024 && length.checked_add(size).is_some_and(|n| n <= limit),
            "Invalid native PNG stream extent"
        );
        png.access(|bytes| {
            reader.read_exact(&mut bytes[length..length + size])?;
            Ok(())
        })?;
        length += size;
    }
    png.access(|bytes| {
        ensure!(
            length >= 33
                && &bytes[..8] == b"\x89PNG\r\n\x1a\n"
                && &bytes[12..16] == b"IHDR"
                && bytes[16..20] == width.to_be_bytes()
                && bytes[20..24] == height.to_be_bytes()
                && bytes[24..26] == [8, 6],
            "Invalid native PNG stream header"
        );
        Ok(())
    })?;
    png.shrink_in_place(length)?;
    checkpoint(2)?;
    Ok(Prepared {
        png,
        width,
        height,
        rgba_bytes,
    })
}
pub(super) struct Publishing {
    dib: Global,
    open: Open,
    _window: Window,
    width: u32,
    height: u32,
    #[cfg(test)]
    previous_transfers: usize,
}
impl Prepared {
    /// Called only after the producer's publication gate; subsequent failures are partial output.
    pub(super) fn begin(mut self) -> Result<Publishing> {
        checkpoint(3)?;
        #[cfg(test)]
        let previous_transfers = stats::TRANSFERRED.load(std::sync::atomic::Ordering::Acquire);
        let format = unsafe {
            RegisterClipboardFormatW([b'P' as u16, b'N' as u16, b'G' as u16, 0].as_ptr())
        };
        ensure!(format != 0, "Native PNG format registration failed");
        let window = Window::new()?;
        let open = Open::new(&window)?;
        ensure!(
            unsafe { EmptyClipboard() } != 0,
            "Native clipboard clear failed; contents may already have changed"
        );
        checkpoint(4)?;
        self.png.transfer(format)?;
        checkpoint(5)?;
        // PNG is OS-owned before allocating DIBV5; no child RGBA or flipped bitmap exists.
        let mut dib = Global::new(self.rgba_bytes + HEADER)?;
        dib.access(|bytes| {
            bytes[..HEADER].copy_from_slice(&dib_header(self.width, self.height, self.rgba_bytes));
            Ok(())
        })?;
        checkpoint(6)?;
        Ok(Publishing {
            dib,
            open,
            _window: window,
            width: self.width,
            height: self.height,
            #[cfg(test)]
            previous_transfers,
        })
    }
}
fn dib_header(width: u32, height: u32, rgba_bytes: usize) -> [u8; HEADER] {
    let mut header = [0; HEADER];
    for (offset, value) in [
        (0, HEADER as u32),
        (4, width),
        (8, height),
        (16, 3),
        (20, rgba_bytes as u32),
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&32u16.to_le_bytes());
    header
}
impl Publishing {
    pub(super) fn read_rows(&mut self, reader: &mut impl Read) -> Result<()> {
        let row = self.width as usize * 4;
        self.dib.access(|bytes| {
            for source_y in 0..self.height as usize {
                let start = HEADER + (self.height as usize - 1 - source_y) * row;
                for chunk in bytes[start..start + row].chunks_mut(64 * 1024) {
                    reader.read_exact(chunk)?;
                    for pixel in chunk.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                }
            }
            Ok(())
        })?;
        checkpoint(7)?;
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<()> {
        self.dib.transfer(DIBV5)?;
        checkpoint(8)?;
        self.open.close()?;
        #[cfg(test)]
        {
            ensure!(
                stats::TRANSFERRED.load(std::sync::atomic::Ordering::Acquire)
                    == self.previous_transfers + 2,
                "Native image publication did not transfer exactly PNG and DIBV5"
            );
            ensure!(
                stats::PEAK_APP_BYTES.load(std::sync::atomic::Ordering::Acquire)
                    <= self.width as usize * self.height as usize * 4
                        + 16 * 1024 * 1024
                        + ALLOC_SLACK,
                "Native PNG and DIB staging exceeded the admitted single-representation peak"
            );
        }
        Ok(())
    }
}
#[cfg(not(test))]
fn checkpoint(_: u8) -> Result<()> {
    Ok(())
}
#[cfg(test)]
fn checkpoint(stage: u8) -> Result<()> {
    ensure!(
        stats::FAULT.load(std::sync::atomic::Ordering::Acquire) != stage,
        "Private native image fault injection"
    );
    Ok(())
}
#[cfg(test)]
pub(super) mod stats {
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    pub static FAULT: AtomicU8 = AtomicU8::new(0);
    pub static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
    pub static FREED: AtomicUsize = AtomicUsize::new(0);
    pub static TRANSFERRED: AtomicUsize = AtomicUsize::new(0);
    pub static PNG: AtomicUsize = AtomicUsize::new(0);
    pub static DIB: AtomicUsize = AtomicUsize::new(0);
    pub static FREE_FAILED: AtomicUsize = AtomicUsize::new(0);
    pub static APP_BYTES: AtomicUsize = AtomicUsize::new(0);
    pub static PEAK_APP_BYTES: AtomicUsize = AtomicUsize::new(0);
    pub static SHRUNK_TAIL_BYTES: AtomicUsize = AtomicUsize::new(0);
    pub fn allocated(bytes: usize) {
        ALLOCATED.fetch_add(1, Ordering::Release);
        let live = APP_BYTES.fetch_add(bytes, Ordering::AcqRel) + bytes;
        PEAK_APP_BYTES.fetch_max(live, Ordering::Release);
    }
    pub fn resized(old: usize, new: usize) {
        if old >= new {
            APP_BYTES.fetch_sub(old - new, Ordering::AcqRel);
            SHRUNK_TAIL_BYTES.fetch_add(old - new, Ordering::Release);
        } else {
            let live = APP_BYTES.fetch_add(new - old, Ordering::AcqRel) + new - old;
            PEAK_APP_BYTES.fetch_max(live, Ordering::Release);
        }
    }
    pub fn freed(success: bool, bytes: usize) {
        if success {
            FREED.fetch_add(1, Ordering::Release);
            APP_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        } else {
            FREE_FAILED.fetch_add(1, Ordering::Release);
        }
    }
    pub fn transferred(dib: bool, bytes: usize) {
        APP_BYTES.fetch_sub(bytes, Ordering::AcqRel);
        TRANSFERRED.fetch_add(1, Ordering::Release);
        if dib {
            DIB.fetch_add(1, Ordering::Release);
        } else {
            PNG.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetClipboardData(format: u32) -> Handle;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}
#[cfg(test)]
pub(super) struct Formats {
    pub png: Option<Vec<u8>>,
    pub dib: Option<Vec<u8>>,
}
#[cfg(test)]
pub(super) fn read_owned_formats() -> Result<Formats> {
    // Caller is the dedicated fixture controller after selecting its fresh private station.
    let window = Window::new()?;
    let mut open = Open::new(&window)?;
    fn copy(format: u32, bound: usize) -> Result<Option<Vec<u8>>> {
        if unsafe { IsClipboardFormatAvailable(format) } == 0 {
            return Ok(None);
        }
        let memory = unsafe { GetClipboardData(format) };
        ensure!(!memory.is_null(), "Owned clipboard format unavailable");
        let size = unsafe { GlobalSize(memory) };
        ensure!(
            size > 0 && size <= bound,
            "Owned clipboard format exceeds fixture bound"
        );
        let pointer = unsafe { GlobalLock(memory) };
        ensure!(
            !pointer.is_null(),
            "Owned clipboard format could not be locked"
        );
        let mut bytes = Vec::new();
        let reserved = bytes.try_reserve_exact(size);
        if reserved.is_ok() {
            bytes.extend_from_slice(unsafe {
                std::slice::from_raw_parts(pointer.cast::<u8>(), size)
            });
        }
        unsafe {
            GlobalUnlock(memory);
        }
        reserved?;
        Ok(Some(bytes))
    }
    let png_type =
        unsafe { RegisterClipboardFormatW([b'P' as u16, b'N' as u16, b'G' as u16, 0].as_ptr()) };
    ensure!(png_type != 0, "Owned PNG format unavailable");
    let png = copy(
        png_type,
        app_core::MAX_CAPTURE_BYTES as usize + 16 * 1024 * 1024 + ALLOC_SLACK,
    )?;
    let dib = copy(
        DIBV5,
        app_core::MAX_CAPTURE_BYTES as usize + HEADER + ALLOC_SLACK,
    )?;
    open.close()?;
    Ok(Formats { png, dib })
}
