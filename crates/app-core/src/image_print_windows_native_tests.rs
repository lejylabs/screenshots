//! Owned memory-DC raster acceptance; document callbacks are controlled test faults.
//! No printer enumeration, Winspool, CreateDCW, file output, UI or network access.
#![allow(non_snake_case)]
use super::*;
use std::cell::RefCell;
use windows_sys::Win32::Graphics::Gdi as gdi;
use windows_sys::Win32::Storage::Xps as xps;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    RealStartDoc,
    StartDoc,
    StartPage,
    EndPage,
    EndDoc,
}

struct State {
    dc: HDC,
    caps: [i32; 8],
    fault: Fault,
    calls: Vec<&'static str>,
    document_valid: bool,
    real_start_result: Option<i32>,
    flushed: bool,
    deleted: bool,
}
thread_local! {
    static ACTIVE: RefCell<Option<State>> = const { RefCell::new(None) };
}

// A scoped per-thread override cannot escape a fixture or affect concurrent tests.
struct Scope;
impl Scope {
    fn enter(dc: HDC, caps: [i32; 8], fault: Fault) -> Self {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            assert!(active.is_none(), "nested native print fixture");
            *active = Some(State {
                dc,
                caps,
                fault,
                calls: Vec::with_capacity(8),
                document_valid: false,
                real_start_result: None,
                flushed: false,
                deleted: false,
            });
        });
        Self
    }
    fn finish(self) -> State {
        ACTIVE.with(|active| active.borrow_mut().take().unwrap())
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.borrow_mut().take());
    }
}
fn controlled<T>(dc: HDC, body: impl FnOnce(&mut State) -> T) -> Option<T> {
    ACTIVE.with(|active| {
        active
            .borrow_mut()
            .as_mut()
            .filter(|state| state.dc == dc)
            .map(body)
    })
}

pub(super) unsafe fn GetDeviceCaps(dc: HDC, cap: i32) -> i32 {
    controlled(dc, |state| {
        let index = [
            LOGPIXELSX,
            LOGPIXELSY,
            PHYSICALWIDTH,
            PHYSICALHEIGHT,
            PHYSICALOFFSETX,
            PHYSICALOFFSETY,
            HORZRES,
            VERTRES,
        ]
        .iter()
        .position(|value| *value as i32 == cap)
        .expect("unexpected print capability");
        state.caps[index]
    })
    .unwrap_or_else(|| unsafe { gdi::GetDeviceCaps(dc, cap) })
}
pub(super) unsafe fn StartDocW(dc: HDC, document: *const DOCINFOW) -> i32 {
    controlled(dc, |state| {
        state.calls.push("StartDoc");
        let document = unsafe { &*document };
        let title: Vec<u16> = "Screenshot".encode_utf16().chain(Some(0)).collect();
        state.document_valid = document.cbSize == size_of::<DOCINFOW>() as i32
            && !document.lpszDocName.is_null()
            && unsafe { std::slice::from_raw_parts(document.lpszDocName, title.len()) } == title
            && document.lpszOutput.is_null()
            && document.lpszDatatype.is_null()
            && document.fwType == 0;
        match state.fault {
            Fault::RealStartDoc => {
                let result = unsafe { xps::StartDocW(dc, document) };
                state.real_start_result = Some(result);
                result
            }
            Fault::StartDoc => -1,
            _ => 7, // Controlled document ID, never evidence of spool acceptance.
        }
    })
    .unwrap_or_else(|| unsafe { xps::StartDocW(dc, document) })
}
pub(super) unsafe fn StartPage(dc: HDC) -> i32 {
    controlled(dc, |state| {
        state.calls.push("StartPage");
        if state.fault == Fault::StartPage {
            -1
        } else {
            1
        }
    })
    .unwrap_or_else(|| unsafe { xps::StartPage(dc) })
}
pub(super) unsafe fn EndPage(dc: HDC) -> i32 {
    controlled(dc, |state| {
        state.calls.push("EndPage");
        if state.fault == Fault::EndPage { -1 } else { 1 }
    })
    .unwrap_or_else(|| unsafe { xps::EndPage(dc) })
}
pub(super) unsafe fn EndDoc(dc: HDC) -> i32 {
    controlled(dc, |state| {
        state.calls.push("EndDoc");
        if state.fault == Fault::EndDoc { -1 } else { 1 }
    })
    .unwrap_or_else(|| unsafe { xps::EndDoc(dc) })
}
pub(super) unsafe fn AbortDoc(dc: HDC) -> i32 {
    controlled(dc, |state| {
        state.calls.push("AbortDoc");
        1 // Controlled cleanup branch; a memory DC has no spool document to abort.
    })
    .unwrap_or_else(|| unsafe { xps::AbortDoc(dc) })
}
pub(super) unsafe fn DeleteDC(dc: HDC) -> i32 {
    // Flush the native drawing batch before examining DIB memory, even on failures.
    let result = controlled(dc, |state| {
        state.calls.push("DeleteDC");
        state.flushed = unsafe { gdi::GdiFlush() } != 0;
        let result = unsafe { gdi::DeleteDC(dc) };
        state.deleted = result != 0;
        result
    });
    result.unwrap_or_else(|| unsafe { gdi::DeleteDC(dc) })
}

// Printer capabilities are deliberate synthetic inputs, not memory-DC capabilities.
// Letter, independent 16/32 DPI, 1/8-inch offsets and 1/4-inch page border.
const CAPS: [i32; 8] = [16, 32, 136, 352, 2, 4, 132, 344];
const WIDTH: usize = 132;
const HEIGHT: usize = 344;
const SENTINEL: [u8; 4] = [19, 29, 39, 255];

struct MemoryPage {
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
}
impl MemoryPage {
    fn new() -> Result<Self> {
        let dc = unsafe { gdi::CreateCompatibleDC(null_mut()) };
        ensure!(!dc.is_null(), "owned memory DC creation failed");
        let mut page = Self {
            dc,
            bitmap: null_mut(),
            bits: null_mut(),
        };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: WIDTH as i32,
                biHeight: -(HEIGHT as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        page.bitmap =
            unsafe { gdi::CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        ensure!(
            !page.bitmap.is_null() && !bits.is_null(),
            "owned DIB creation failed"
        );
        page.bits = bits.cast();
        let previous = unsafe { gdi::SelectObject(dc, page.bitmap) };
        ensure!(
            !previous.is_null() && previous as isize != -1,
            "owned DIB selection failed"
        );
        for pixel in unsafe { std::slice::from_raw_parts_mut(page.bits, WIDTH * HEIGHT * 4) }
            .chunks_exact_mut(4)
        {
            pixel.copy_from_slice(&SENTINEL);
        }
        Ok(page)
    }
    fn take_dc(&mut self) -> HDC {
        std::mem::replace(&mut self.dc, null_mut())
    }
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        assert!(x < WIDTH && y < HEIGHT);
        unsafe { std::slice::from_raw_parts(self.bits.add((y * WIDTH + x) * 4), 4) }
            .try_into()
            .unwrap()
    }
}
impl Drop for MemoryPage {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                gdi::DeleteDC(self.dc);
            }
            if !self.bitmap.is_null() {
                assert!(
                    gdi::DeleteObject(self.bitmap) != 0,
                    "owned DIB cleanup failed"
                );
            }
        }
    }
}
fn synthetic_pixels() -> RgbaImage {
    RgbaImage::from_fn(8, 8, |x, y| {
        image::Rgba(match (x < 4, y < 4) {
            (true, true) => [255, 0, 0, 255],
            (false, true) => [0, 255, 0, 128],
            (true, false) => [0, 0, 255, 255],
            (false, false) => [4, 5, 6, 0],
        })
    })
}
fn run_case(
    pixels: &mut RgbaImage,
    paper: PrintPaper,
    caps: [i32; 8],
    fault: Fault,
) -> Result<(Result<u32>, bool, State, MemoryPage)> {
    let mut page = MemoryPage::new()?;
    let scope = Scope::enter(page.dc, caps, fault);
    let mut started = false;
    let result = print_device_pixels(pixels, paper, &mut started, page.take_dc());
    let state = scope.finish();
    ensure!(
        state.flushed && state.deleted,
        "native DC flush/cleanup failed"
    );
    Ok((result, started, state, page))
}

#[test]
#[ignore = "owned Windows GDI memory DC only; no printer, UI or spool acceptance"]
fn windows_owned_gdi_pixels_geometry_and_cleanup() -> Result<()> {
    let mut pixels = synthetic_pixels();
    let allocation = pixels.as_raw().as_ptr();
    let (result, started, state, page) =
        run_case(&mut pixels, PrintPaper::Letter, CAPS, Fault::None)?;
    assert_eq!(result?, 7);
    assert!(started && state.document_valid);
    assert_eq!(
        state.calls,
        ["StartDoc", "StartPage", "EndPage", "EndDoc", "DeleteDC"]
    );
    // Independent physical fit: 8x8 square at 16x32 DPI becomes 128x256 device
    // pixels, centered in [4,132]x[8,344], minus offsets (2,4): (2,44).
    // Color checks are well inside constant quadrants, away from HALFTONE edges.
    for (x, y, expected) in [
        (34, 108, [0, 0, 255]),
        (98, 108, [127, 255, 127]),
        (34, 236, [255, 0, 0]),
        (98, 236, [255, 255, 255]),
    ] {
        assert_eq!(page.pixel(x, y)[..3], expected);
    }
    // Every pixel outside the exact rectangle remains untouched. Alpha is ignored
    // inside: BI_RGB's destination high byte has no alpha contract.
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if !(2..130).contains(&x) || !(44..300).contains(&y) {
                assert_eq!(page.pixel(x, y), SENTINEL, "outside page fit at {x},{y}");
            } else {
                assert_ne!(
                    page.pixel(x, y)[..3],
                    SENTINEL[..3],
                    "missing fit pixel at {x},{y}"
                );
            }
        }
    }
    // Top-down source orientation, opaque BGRA, rounded white composition, same allocation.
    assert_eq!(&pixels.as_raw()[..4], &[0, 0, 255, 255]);
    assert_eq!(&pixels.as_raw()[16..20], &[127, 255, 127, 255]);
    assert_eq!(&pixels.as_raw()[128..132], &[255, 0, 0, 255]);
    assert_eq!(&pixels.as_raw()[144..148], &[255, 255, 255, 255]);
    assert_eq!(pixels.as_raw().as_ptr(), allocation);
    Ok(())
}

#[test]
#[ignore = "owned Windows GDI only; controlled document faults and real memory-DC StartDoc refusal"]
fn windows_owned_gdi_failures_latch_and_cleanup() -> Result<()> {
    for (fault, expected) in [
        (Fault::RealStartDoc, vec!["StartDoc", "DeleteDC"]),
        (Fault::StartDoc, vec!["StartDoc", "DeleteDC"]),
        (
            Fault::StartPage,
            vec!["StartDoc", "StartPage", "AbortDoc", "DeleteDC"],
        ),
        (
            Fault::EndPage,
            vec!["StartDoc", "StartPage", "EndPage", "AbortDoc", "DeleteDC"],
        ),
        (
            Fault::EndDoc,
            vec![
                "StartDoc",
                "StartPage",
                "EndPage",
                "EndDoc",
                "AbortDoc",
                "DeleteDC",
            ],
        ),
    ] {
        let (result, started, state, _) =
            run_case(&mut synthetic_pixels(), PrintPaper::Letter, CAPS, fault)?;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("outcome is unknown")
        );
        assert!(started && state.document_valid);
        assert_eq!(state.calls, expected);
        if fault == Fault::RealStartDoc {
            assert!(state.real_start_result.unwrap() <= 0);
        }
    }
    for (caps, paper, message) in [
        (
            [0, 32, 136, 352, 2, 4, 132, 344],
            PrintPaper::Letter,
            "Invalid printer geometry",
        ),
        (CAPS, PrintPaper::A4, "requested paper"),
        (
            [16, 32, 136, 352, 0, 0, 1, 1],
            PrintPaper::Letter,
            "no usable page area",
        ),
    ] {
        let mut pixels = synthetic_pixels();
        let original = pixels.clone();
        let (result, started, state, _) = run_case(&mut pixels, paper, caps, Fault::None)?;
        assert!(result.unwrap_err().to_string().contains(message));
        assert!(!started && !state.document_valid);
        assert_eq!(state.calls, ["DeleteDC"]);
        assert_eq!(pixels, original);
    }
    Ok(())
}
