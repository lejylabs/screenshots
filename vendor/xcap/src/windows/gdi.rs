use std::mem;

use image::RgbaImage;
use scopeguard::guard;
use windows::Win32::{
    Foundation::{GetLastError, HWND, RECT},
    Graphics::{
        Dwm::DwmIsCompositionEnabled,
        Gdi::{
            BITMAPINFO, BITMAPINFOHEADER, BLACKNESS, BitBlt, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetWindowDC,
            HBITMAP, HDC, PatBlt, ReleaseDC, SRCCOPY, SelectObject,
        },
    },
    Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
    UI::{
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            SetThreadDpiAwarenessContext,
        },
        WindowsAndMessaging::{GetDesktopWindow, GetWindowRect, GetWindowThreadProcessId},
    },
};

use crate::{
    error::{XCapError, XCapResult},
    platform::utils::get_window_bounds,
};

use super::utils::{bgra_to_rgba, get_os_major_version};

fn to_rgba_image(
    hdc_mem: HDC,
    h_bitmap: HBITMAP,
    width: i32,
    height: i32,
) -> XCapResult<RgbaImage> {
    let buffer_size = super::window_crop::bitmap_bytes(width, height).map_err(XCapError::new)?;
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biSizeImage: buffer_size as u32,
            biCompression: 0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut buffer = vec![0u8; buffer_size];

    unsafe {
        let is_failed = GetDIBits(
            hdc_mem,
            h_bitmap,
            0,
            height as u32,
            Some(buffer.as_mut_ptr().cast()),
            &mut bitmap_info,
            DIB_RGB_COLORS,
        ) != height;

        if is_failed {
            return Err(XCapError::new("Get RGBA data failed"));
        }
    };

    RgbaImage::from_raw(width as u32, height as u32, bgra_to_rgba(buffer))
        .ok_or_else(|| XCapError::new("RgbaImage::from_raw failed"))
}

fn delete_bitmap_object(val: HBITMAP) {
    unsafe {
        let succeed = DeleteObject(val.into()).as_bool();

        if !succeed {
            log::error!("DeleteObject({:?}) failed: {:?}", val, GetLastError());
        }
    }
}

pub(super) fn capture_monitor(x: i32, y: i32, width: i32, height: i32) -> XCapResult<RgbaImage> {
    super::window_crop::bitmap_bytes(width, height).map_err(XCapError::new)?;
    if x.checked_add(width).is_none() || y.checked_add(height).is_none() {
        return Err(XCapError::new("GDI monitor coordinates overflow"));
    }
    unsafe {
        let hwnd = GetDesktopWindow();
        let scope_guard_hdc_desktop_window = guard(GetWindowDC(Some(hwnd)), |val| {
            if ReleaseDC(Some(hwnd), val) != 1 {
                log::error!("ReleaseDC({:?}) failed: {:?}", val, GetLastError());
            }
        });
        if scope_guard_hdc_desktop_window.0.is_null() {
            return Err(XCapError::new("Desktop DC is unavailable"));
        }

        let scope_guard_mem = guard(
            CreateCompatibleDC(Some(*scope_guard_hdc_desktop_window)),
            |val| {
                if !DeleteDC(val).as_bool() {
                    log::error!("DeleteDC({:?}) failed: {:?}", val, GetLastError());
                }
            },
        );
        if scope_guard_mem.0.is_null() {
            return Err(XCapError::new("Monitor bitmap DC is unavailable"));
        }

        let scope_guard_h_bitmap = guard(
            CreateCompatibleBitmap(*scope_guard_hdc_desktop_window, width, height),
            delete_bitmap_object,
        );
        if scope_guard_h_bitmap.0.is_null() {
            return Err(XCapError::new("Monitor bitmap allocation failed"));
        }

        let previous = SelectObject(*scope_guard_mem, (*scope_guard_h_bitmap).into());
        if previous.0.is_null() || previous.0 as isize == -1 {
            return Err(XCapError::new("Cannot select monitor bitmap"));
        }
        let selection = guard(previous, |previous| {
            SelectObject(*scope_guard_mem, previous);
        });

        BitBlt(
            *scope_guard_mem,
            0,
            0,
            width,
            height,
            Some(*scope_guard_hdc_desktop_window),
            x,
            y,
            SRCCOPY,
        )?;

        drop(selection);
        to_rgba_image(*scope_guard_mem, *scope_guard_h_bitmap, width, height)
    }
}

pub(super) fn capture_window(hwnd: HWND) -> XCapResult<RgbaImage> {
    unsafe {
        let mut previous_dpi =
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if previous_dpi.0.is_null() {
            previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
        }
        if previous_dpi.0.is_null() {
            return Err(XCapError::new("Physical window DPI context is unavailable"));
        }
        let _dpi = guard(previous_dpi, |context| {
            SetThreadDpiAwarenessContext(context);
        });
        let window_bounds = get_window_bounds(hwnd)?;
        let mut window_rect = RECT::default();
        GetWindowRect(hwnd, &mut window_rect)?;
        let rect = |value: RECT| [value.left, value.top, value.right, value.bottom];
        let raw = rect(window_rect);
        let visible = rect(window_bounds);
        super::window_crop::physical_crop(raw, visible).map_err(XCapError::new)?;
        let bitmap_width = window_rect.right - window_rect.left;
        let bitmap_height = window_rect.bottom - window_rect.top;
        super::window_crop::bitmap_bytes(bitmap_width, bitmap_height).map_err(XCapError::new)?;
        let mut process = 0;
        if GetWindowThreadProcessId(hwnd, Some(&mut process)) == 0 || process == 0 {
            return Err(XCapError::new("Window process is unavailable"));
        }

        let scope_guard_hdc_window = guard(GetWindowDC(Some(hwnd)), |val| {
            if ReleaseDC(Some(hwnd), val) != 1 {
                log::error!("ReleaseDC({:?}) failed: {:?}", val, GetLastError());
            }
        });
        if scope_guard_hdc_window.0.is_null() {
            return Err(XCapError::new("Window DC is unavailable"));
        }

        let scope_guard_hdc_mem = guard(CreateCompatibleDC(Some(*scope_guard_hdc_window)), |val| {
            if !DeleteDC(val).as_bool() {
                log::error!("DeleteDC({:?}) failed: {:?}", val, GetLastError());
            }
        });
        if scope_guard_hdc_mem.0.is_null() {
            return Err(XCapError::new("Window bitmap DC is unavailable"));
        }
        let scope_guard_h_bitmap = guard(
            CreateCompatibleBitmap(*scope_guard_hdc_window, bitmap_width, bitmap_height),
            delete_bitmap_object,
        );
        if scope_guard_h_bitmap.0.is_null() {
            return Err(XCapError::new("Window bitmap allocation failed"));
        }

        let previous_object = SelectObject(*scope_guard_hdc_mem, (*scope_guard_h_bitmap).into());
        if previous_object.0.is_null() || previous_object.0 as isize == -1 {
            return Err(XCapError::new("Cannot select window bitmap"));
        }
        let selection = guard(previous_object, |previous| {
            SelectObject(*scope_guard_hdc_mem, previous);
        });
        if !PatBlt(
            *scope_guard_hdc_mem,
            0,
            0,
            bitmap_width,
            bitmap_height,
            BLACKNESS,
        )
        .as_bool()
        {
            return Err(XCapError::new("Cannot clear window bitmap"));
        }

        let mut is_success = false;

        if get_os_major_version() >= 8 {
            is_success = PrintWindow(hwnd, *scope_guard_hdc_mem, PRINT_WINDOW_FLAGS(2)).as_bool();
        }

        if !is_success && DwmIsCompositionEnabled()?.as_bool() {
            is_success = PrintWindow(hwnd, *scope_guard_hdc_mem, PRINT_WINDOW_FLAGS(0)).as_bool();
        }

        if !is_success {
            is_success = PrintWindow(hwnd, *scope_guard_hdc_mem, PRINT_WINDOW_FLAGS(4)).as_bool();
        }

        if !is_success {
            is_success = BitBlt(
                *scope_guard_hdc_mem,
                0,
                0,
                bitmap_width,
                bitmap_height,
                Some(*scope_guard_hdc_window),
                0,
                0,
                SRCCOPY,
            )
            .is_ok();
        }

        if !is_success {
            return Err(XCapError::new("Capture window failed"));
        }

        drop(selection);

        let image = to_rgba_image(
            *scope_guard_hdc_mem,
            *scope_guard_h_bitmap,
            bitmap_width,
            bitmap_height,
        )?;

        let mut current_rect = RECT::default();
        GetWindowRect(hwnd, &mut current_rect)?;
        let current_bounds = get_window_bounds(hwnd)?;
        let mut current_process = 0;
        if rect(current_rect) != raw
            || rect(current_bounds) != visible
            || GetWindowThreadProcessId(hwnd, Some(&mut current_process)) == 0
            || current_process != process
        {
            return Err(XCapError::new(
                "Window geometry or process changed during GDI capture",
            ));
        }
        super::window_crop::crop_gdi_window(image, raw, visible).map_err(XCapError::new)
    }
}
