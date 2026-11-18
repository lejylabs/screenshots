//! GDI calls are blocking and may enter third-party drivers: isolated helper only.
use super::{PrintPaper, print_rgb};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use std::{
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE, HWND},
    Graphics::{Gdi::*, Printing::*},
    Storage::Xps::*,
    System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
};

type Open =
    unsafe extern "system" fn(*const u16, *mut PRINTER_HANDLE, *const PRINTER_DEFAULTSW) -> i32;
type Close = unsafe extern "system" fn(PRINTER_HANDLE) -> i32;
type Get = unsafe extern "system" fn(PRINTER_HANDLE, u32, *mut u8, u32, *mut u32) -> i32;
type Properties = unsafe extern "system" fn(
    HWND,
    PRINTER_HANDLE,
    *const u16,
    *mut DEVMODEW,
    *const DEVMODEW,
    u32,
) -> i32;
struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
struct SpoolApi {
    _library: Library,
    open: Open,
    close: Close,
    get: Get,
    properties: Properties,
}
impl SpoolApi {
    fn new() -> Result<Self> {
        // Do not add an eager winspool import to ordinary application startup.
        let name: Vec<u16> = "winspool.drv".encode_utf16().chain(Some(0)).collect();
        let module =
            unsafe { LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        ensure!(!module.is_null(), "System print client is unavailable");
        let library = Library(module);
        let address = |name: &std::ffi::CStr| {
            unsafe { GetProcAddress(module, name.as_ptr().cast()) }
                .context("Required system print entry is unavailable")
        };
        // Each ABI matches the locked windows-sys 0.61.2 Printing declarations.
        let open = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Open>(address(
                c"OpenPrinterW",
            )?)
        };
        let close = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(address(
                c"ClosePrinter",
            )?)
        };
        let get = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Get>(address(
                c"GetPrinterW",
            )?)
        };
        let properties = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Properties>(address(
                c"DocumentPropertiesW",
            )?)
        };
        Ok(Self {
            _library: library,
            open,
            close,
            get,
            properties,
        })
    }
}
struct Printer<'a> {
    handle: PRINTER_HANDLE,
    api: &'a SpoolApi,
}
impl Drop for Printer<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.handle);
        }
    }
}
struct Dc {
    handle: HDC,
    open: bool,
}
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            // Best effort only: this does not promise cancellation at the printer.
            if self.open {
                AbortDoc(self.handle);
            }
            DeleteDC(self.handle);
        }
    }
}

fn paper_code(paper: PrintPaper) -> i16 {
    match paper {
        PrintPaper::A4 => DMPAPER_A4 as i16,
        PrintPaper::Letter => DMPAPER_LETTER as i16,
    }
}

fn port_in_buffer(buffer: &[usize], pointer: *const u16) -> Result<String> {
    let start = buffer.as_ptr() as usize;
    let end = start + std::mem::size_of_val(buffer);
    let address = pointer as usize;
    ensure!(
        address >= start && address < end && address % 2 == 0,
        "Invalid printer port response"
    );
    let count = ((end - address) / 2).min(1024);
    let text = unsafe { std::slice::from_raw_parts(pointer, count) };
    let length = text
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| anyhow::anyhow!("Printer port exceeds limit"))?;
    Ok(String::from_utf16(&text[..length])?)
}

fn allowed_port(port: &str) -> bool {
    let port = port.to_ascii_uppercase();
    !port.is_empty()
        && port.len() <= 256
        && port
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
        && ["IP_", "WSD-", "USB", "LPT", "COM", "TS"]
            .iter()
            .any(|prefix| port.starts_with(prefix))
}

fn validated_mode(printer: &Printer<'_>, name: &[u16], paper: PrintPaper) -> Result<Vec<usize>> {
    let required = unsafe {
        (printer.api.properties)(
            null_mut(),
            printer.handle,
            name.as_ptr(),
            null_mut(),
            null(),
            0,
        )
    };
    ensure!(
        required >= size_of::<DEVMODEW>() as i32 && required <= 65536,
        "Printer mode exceeds limit"
    );
    let mut buffer = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
    let mode = buffer.as_mut_ptr().cast::<DEVMODEW>();
    ensure!(
        unsafe {
            (printer.api.properties)(
                null_mut(),
                printer.handle,
                name.as_ptr(),
                mode,
                null(),
                DM_OUT_BUFFER,
            )
        } == 1,
        "Printer mode could not be read without a dialog"
    );
    let settings = unsafe { &mut *mode };
    ensure!(
        usize::from(settings.dmSize) >= size_of::<DEVMODEW>()
            && usize::from(settings.dmSize) + usize::from(settings.dmDriverExtra)
                <= required as usize,
        "Invalid printer mode layout"
    );
    settings.dmFields &= !(DM_PAPERLENGTH | DM_PAPERWIDTH | DM_FORMNAME | DM_NUP);
    settings.dmFields |=
        DM_ORIENTATION | DM_PAPERSIZE | DM_SCALE | DM_COPIES | DM_DUPLEX | DM_COLLATE;
    unsafe {
        settings.Anonymous1.Anonymous1.dmOrientation = DMORIENT_PORTRAIT as i16;
        settings.Anonymous1.Anonymous1.dmPaperSize = paper_code(paper);
        settings.Anonymous1.Anonymous1.dmScale = 100;
        settings.Anonymous1.Anonymous1.dmCopies = 1;
    }
    settings.dmDuplex = DMDUP_SIMPLEX as i16;
    settings.dmCollate = DMCOLLATE_FALSE as i16;
    ensure!(
        unsafe {
            (printer.api.properties)(
                null_mut(),
                printer.handle,
                name.as_ptr(),
                mode,
                mode,
                DM_IN_BUFFER | DM_OUT_BUFFER,
            )
        } == 1,
        "Printer rejected explicit page settings"
    );
    let settings = unsafe { &*mode };
    let fields = DM_ORIENTATION | DM_PAPERSIZE | DM_SCALE | DM_COPIES | DM_DUPLEX | DM_COLLATE;
    let page = unsafe { settings.Anonymous1.Anonymous1 };
    ensure!(
        settings.dmFields & fields == fields
            && page.dmOrientation == DMORIENT_PORTRAIT as i16
            && page.dmPaperSize == paper_code(paper)
            && page.dmScale == 100
            && page.dmCopies == 1
            && settings.dmDuplex == DMDUP_SIMPLEX as i16
            && settings.dmCollate == DMCOLLATE_FALSE as i16,
        "Printer changed required page settings"
    );
    Ok(buffer)
}

/// Mutates only owned final pixels (RGBA to opaque BGRA); source files stay untouched.
/// `submission_started` is latched immediately before StartDoc, including its errors.
pub fn print_windows_pixels(
    image: &mut RgbaImage,
    name: &str,
    paper: PrintPaper,
    submission_started: &mut bool,
) -> Result<u32> {
    ensure!(
        !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control),
        "Invalid printer name"
    );
    ensure!(
        image.width() > 0
            && image.height() > 0
            && image.as_raw().len() as u64 <= crate::MAX_CAPTURE_BYTES,
        "Invalid print image"
    );
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let api = SpoolApi::new()?;
    let mut handle = PRINTER_HANDLE::default();
    ensure!(
        unsafe { (api.open)(name.as_ptr(), &mut handle, null()) } != 0,
        "Named printer is unavailable"
    );
    let printer = Printer { handle, api: &api };
    let mut required = 0;
    unsafe {
        (api.get)(printer.handle, 5, null_mut(), 0, &mut required);
    }
    ensure!(
        required as usize >= size_of::<PRINTER_INFO_5W>() && required <= 65536,
        "Printer response exceeds limit"
    );
    let mut info = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
    ensure!(
        unsafe {
            (api.get)(
                printer.handle,
                5,
                info.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } != 0,
        "Printer port could not be verified"
    );
    let port = unsafe { &*info.as_ptr().cast::<PRINTER_INFO_5W>() }.pPortName;
    let port = port_in_buffer(&info, port)?;
    let permitted = allowed_port(&port);
    // Only an explicitly instrumented build can admit its one baked-in owned sink.
    #[cfg(screenshots_native_print_acceptance)]
    let permitted = permitted || owned_fixture_port(&name, &port);
    ensure!(
        permitted,
        "Printer requires an unsupported or interactive port"
    );
    let mode = validated_mode(&printer, &name, paper)?;
    let handle = unsafe { CreateDCW(null(), name.as_ptr(), null(), mode.as_ptr().cast()) };
    ensure!(
        !handle.is_null(),
        "Printer device context could not be created"
    );
    print_device_pixels(image, paper, submission_started, handle)
}

// Owns this DC from entry through every success/error return.
fn print_device_pixels(
    image: &mut RgbaImage,
    paper: PrintPaper,
    submission_started: &mut bool,
    handle: HDC,
) -> Result<u32> {
    let mut dc = Dc {
        handle,
        open: false,
    };
    let cap = |value: u32| unsafe { GetDeviceCaps(handle, value as i32) };
    let (dpi_x, dpi_y) = (cap(LOGPIXELSX), cap(LOGPIXELSY));
    let (physical_x, physical_y) = (cap(PHYSICALWIDTH), cap(PHYSICALHEIGHT));
    let (offset_x, offset_y) = (cap(PHYSICALOFFSETX), cap(PHYSICALOFFSETY));
    let (print_x, print_y) = (cap(HORZRES), cap(VERTRES));
    ensure!(
        (1..=19200).contains(&dpi_x)
            && (1..=19200).contains(&dpi_y)
            && physical_x > 0
            && physical_y > 0
            && print_x > 0
            && print_y > 0
            && offset_x >= 0
            && offset_y >= 0,
        "Invalid printer geometry"
    );
    let (paper_width, paper_height) = paper.points();
    ensure!(
        (f64::from(physical_x) / f64::from(dpi_x) - paper_width / 72.0).abs() <= 0.03
            && (f64::from(physical_y) / f64::from(dpi_y) - paper_height / 72.0).abs() <= 0.03,
        "Printer did not select the requested paper"
    );
    let left = f64::from(offset_x).max(f64::from(dpi_x) / 4.0);
    let top = f64::from(offset_y).max(f64::from(dpi_y) / 4.0);
    let right = (f64::from(offset_x) + f64::from(print_x))
        .min(f64::from(physical_x) - f64::from(dpi_x) / 4.0);
    let bottom = (f64::from(offset_y) + f64::from(print_y))
        .min(f64::from(physical_y) - f64::from(dpi_y) / 4.0);
    ensure!(
        right > left && bottom > top,
        "Printer has no usable page area"
    );
    let scale = ((right - left) / f64::from(image.width()) / f64::from(dpi_x))
        .min((bottom - top) / f64::from(image.height()) / f64::from(dpi_y));
    let width = (f64::from(image.width()) * scale * f64::from(dpi_x)).floor() as i32;
    let height = (f64::from(image.height()) * scale * f64::from(dpi_y)).floor() as i32;
    ensure!(
        width > 0 && height > 0,
        "Print image becomes smaller than a device pixel"
    );
    let x = (left + (right - left - f64::from(width)) / 2.0).floor() as i32 - offset_x;
    let y = (top + (bottom - top - f64::from(height)) / 2.0).floor() as i32 - offset_y;
    for pixel in image.chunks_exact_mut(4) {
        let [red, green, blue] = print_rgb(pixel);
        pixel.copy_from_slice(&[blue, green, red, 255]);
    }
    let title: Vec<u16> = "Screenshot".encode_utf16().chain(Some(0)).collect();
    let document = DOCINFOW {
        cbSize: size_of::<DOCINFOW>() as i32,
        lpszDocName: title.as_ptr(),
        ..Default::default()
    };
    *submission_started = true;
    let id = unsafe { StartDocW(handle, &document) };
    ensure!(id > 0, "Spool submission failed; outcome is unknown");
    dc.open = true;
    ensure!(
        unsafe { StartPage(handle) } > 0,
        "Spool page failed; outcome is unknown"
    );
    let bitmap = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: image.width() as i32,
            biHeight: -(image.height() as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    ensure!(
        unsafe { SetStretchBltMode(handle, HALFTONE) } != 0,
        "Spool scaling failed; outcome is unknown"
    );
    unsafe {
        SetBrushOrgEx(handle, 0, 0, null_mut());
    }
    let rows = unsafe {
        StretchDIBits(
            handle,
            x,
            y,
            width,
            height,
            0,
            0,
            image.width() as i32,
            image.height() as i32,
            image.as_raw().as_ptr().cast(),
            &bitmap,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    ensure!(
        rows > 0 && rows != GDI_ERROR as i32,
        "Spool pixels failed; outcome is unknown"
    );
    ensure!(
        unsafe { EndPage(handle) } > 0 && unsafe { EndDoc(handle) } > 0,
        "Spool completion failed; outcome is unknown"
    );
    dc.open = false;
    Ok(id as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interactive_and_file_ports_are_never_opened() {
        for port in [
            "FILE:",
            "PORTPROMPT:",
            "XPSPORT:",
            "C:\\output.pdf",
            "USB001,USB002",
            "",
        ] {
            assert!(!allowed_port(port));
        }
        for port in ["USB001", "IP_127.0.0.1", "WSD-1234", "LPT1:"] {
            assert!(allowed_port(port));
        }
    }
}

// Only test binaries can substitute document callbacks/capabilities. Raster calls stay native.
#[cfg(test)]
#[path = "image_print_windows_native_tests.rs"]
mod native_fixture;
#[cfg(test)]
use native_fixture::{AbortDoc, DeleteDC, EndDoc, EndPage, GetDeviceCaps, StartDocW, StartPage};

// No runtime switch: both exact values are required at compile time in the CI build.
#[cfg(screenshots_native_print_acceptance)]
fn owned_fixture_port(name: &[u16], port: &str) -> bool {
    let expected_name = env!("SCREENSHOTS_NATIVE_PRINT_QUEUE");
    let expected_port = env!("SCREENSHOTS_NATIVE_PRINT_PORT");
    name.iter()
        .copied()
        .eq(expected_name.encode_utf16().chain(Some(0)))
        && port == expected_port
}
