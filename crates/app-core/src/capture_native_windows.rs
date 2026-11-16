use crate::{
    CaptureCursor, CaptureMonitorBinding, CaptureMonitorIdentity, CaptureOptions,
    CaptureWindowFrame, DisplayInfo, capture_options::CaptureCursorImage,
};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Devices::Display::*,
    Foundation::{POINT, RECT},
    Graphics::{Dwm::*, Gdi::*},
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

pub(crate) struct DpiScope(DPI_AWARENESS_CONTEXT);
impl DpiScope {
    pub fn new() -> Result<Self> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure!(
            !previous.is_null(),
            "Cannot enter physical-pixel capture DPI context"
        );
        Ok(Self(previous))
    }
}
impl Drop for DpiScope {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
fn wide(value: &[u16]) -> Result<String> {
    let end = value
        .iter()
        .position(|value| *value == 0)
        .context("Native display name is not terminated")?;
    Ok(String::from_utf16(&value[..end])?)
}
pub(super) fn display_identities(displays: &[DisplayInfo]) -> Result<Vec<CaptureMonitorIdentity>> {
    let mut path_count = 0;
    let mut mode_count = 0;
    ensure!(
        unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        } == 0,
        "Cannot enumerate active display identities"
    );
    ensure!(
        (1..=64).contains(&path_count) && mode_count <= 256,
        "Native display identity metadata exceeds its bound"
    );
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    let capacity = (path_count, mode_count);
    ensure!(
        unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                null_mut(),
            )
        } == 0
            && path_count <= capacity.0
            && mode_count <= capacity.1,
        "Display topology changed during identity enumeration"
    );
    paths.truncate(path_count as usize);
    let monitors = xcap::Monitor::all()?;
    ensure!(
        monitors.len() <= 32,
        "Native monitor enumeration exceeds 32 displays"
    );
    let mut result = Vec::with_capacity(displays.len());
    for display in displays {
        let monitor = monitors
            .iter()
            .find(|monitor| monitor.id().is_ok_and(|id| id.to_string() == display.id))
            .context("Native display disappeared")?;
        let gdi_name = monitor.name()?;
        let mut matches = Vec::new();
        for path in &paths {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            ensure!(
                unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0,
                "Cannot resolve source display identity"
            );
            if !wide(&source.viewGdiDeviceName)?.eq_ignore_ascii_case(&gdi_name) {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            ensure!(
                unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } == 0,
                "Cannot resolve physical display identity"
            );
            let device = wide(&target.monitorDevicePath)?;
            ensure!(
                !device.is_empty() && device.len() <= 960,
                "Physical display identity is unavailable"
            );
            matches.push(format!("windows:{}", device.to_lowercase()));
        }
        ensure!(
            matches.len() == 1,
            "Native display identity is unavailable or ambiguous (including mirrored outputs)"
        );
        result.push(CaptureMonitorIdentity {
            display_id: display.id.clone(),
            binding: CaptureMonitorBinding::Physical {
                key: matches.pop().unwrap(),
            },
        });
    }
    Ok(result)
}
pub(super) fn validate_options(options: &CaptureOptions, window: bool) -> Result<()> {
    if window {
        super::validate_native_window_policy(options, true, true, false, false)?;
    }
    Ok(())
}
pub(super) fn validate_template_session() -> Result<()> {
    Ok(())
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}
struct Icon(HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DestroyIcon(self.0);
            }
        }
    }
}
struct DeviceContext(HDC);
impl Drop for DeviceContext {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(null_mut(), self.0);
        }
    }
}
fn bitmap_size(bitmap: HBITMAP) -> Result<(u32, u32)> {
    let mut info = BITMAP::default();
    ensure!(
        unsafe {
            GetObjectW(
                bitmap,
                size_of::<BITMAP>() as i32,
                (&mut info as *mut BITMAP).cast(),
            )
        } == size_of::<BITMAP>() as i32,
        "Cannot read native cursor bitmap metadata"
    );
    ensure!(
        (1..=1024).contains(&info.bmWidth) && (1..=2048).contains(&info.bmHeight),
        "Native cursor exceeds its bitmap bound"
    );
    Ok((info.bmWidth as u32, info.bmHeight as u32))
}
fn color_bitmap(dc: HDC, bitmap: HBITMAP, width: u32, height: u32) -> Result<RgbaImage> {
    ensure!(height <= 1024, "Native cursor exceeds 1024×1024");
    let mut bytes = vec![0; width as usize * height as usize * 4];
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    ensure!(
        unsafe {
            GetDIBits(
                dc,
                bitmap,
                0,
                height,
                bytes.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        } == height as i32,
        "Cannot read native color cursor bitmap"
    );
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RgbaImage::from_raw(width, height, bytes).context("Invalid native color cursor backing")
}
#[repr(C)]
struct MaskInfo {
    header: BITMAPINFOHEADER,
    colors: [RGBQUAD; 2],
}
fn mask_bitmap(dc: HDC, bitmap: HBITMAP, width: u32, height: u32) -> Result<(Vec<u8>, usize)> {
    let stride = (width as usize).div_ceil(32) * 4;
    let mut bytes = vec![0; stride * height as usize];
    let mut info = MaskInfo {
        header: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 1,
            biCompression: BI_RGB,
            biClrUsed: 2,
            ..Default::default()
        },
        colors: [
            RGBQUAD::default(),
            RGBQUAD {
                rgbBlue: 255,
                rgbGreen: 255,
                rgbRed: 255,
                rgbReserved: 0,
            },
        ],
    };
    ensure!(
        unsafe {
            GetDIBits(
                dc,
                bitmap,
                0,
                height,
                bytes.as_mut_ptr().cast(),
                (&mut info as *mut MaskInfo).cast(),
                DIB_RGB_COLORS,
            )
        } == height as i32,
        "Cannot read native cursor mask bitmap"
    );
    Ok((bytes, stride))
}
pub(super) fn apply_cursor(image: &mut RgbaImage, origin_x: i32, origin_y: i32) -> Result<()> {
    let _dpi = DpiScope::new()?;
    let mut cursor = CURSORINFO {
        cbSize: size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    ensure!(
        unsafe { GetCursorInfo(&mut cursor) } != 0,
        "Cannot query native cursor"
    );
    if cursor.flags & CURSOR_SHOWING == 0 {
        return Ok(());
    }
    let icon = Icon(unsafe { CopyIcon(cursor.hCursor) });
    ensure!(!icon.0.is_null(), "Cannot snapshot native cursor");
    let mut info = ICONINFO::default();
    ensure!(
        unsafe { GetIconInfo(icon.0, &mut info) } != 0,
        "Cannot query native cursor hotspot"
    );
    let mask = Bitmap(info.hbmMask);
    let color = Bitmap(info.hbmColor);
    let (width, mask_height) = bitmap_size(mask.0)?;
    let height = if color.0.is_null() {
        ensure!(
            mask_height.is_multiple_of(2),
            "Invalid monochrome cursor mask"
        );
        mask_height / 2
    } else {
        let dimensions = bitmap_size(color.0)?;
        ensure!(
            dimensions == (width, mask_height),
            "Mismatched native cursor mask"
        );
        dimensions.1
    };
    ensure!(
        height > 0 && height <= 1024 && info.xHotspot < width && info.yHotspot < height,
        "Invalid native cursor dimensions/hotspot"
    );
    let dc = DeviceContext(unsafe { GetDC(null_mut()) });
    ensure!(!dc.0.is_null(), "Cannot acquire cursor bitmap context");
    let mut pixels = if color.0.is_null() {
        None
    } else {
        Some(color_bitmap(dc.0, color.0, width, height)?)
    };
    if pixels
        .as_ref()
        .is_some_and(|pixels| pixels.pixels().any(|pixel| pixel[3] != 0))
    {
        let mut pixels = pixels.take().unwrap();
        for pixel in pixels.pixels_mut().filter(|pixel| pixel[3] == 0) {
            pixel.0[..3].fill(0);
        }
        return CaptureCursorImage {
            pixels,
            screen_x: cursor.ptScreenPos.x,
            screen_y: cursor.ptScreenPos.y,
            hotspot_x: info.xHotspot,
            hotspot_y: info.yHotspot,
        }
        .compose(image, origin_x, origin_y);
    }
    let (mask, stride) = mask_bitmap(dc.0, mask.0, width, mask_height)?;
    let left = i64::from(cursor.ptScreenPos.x) - i64::from(info.xHotspot) - i64::from(origin_x);
    let top = i64::from(cursor.ptScreenPos.y) - i64::from(info.yHotspot) - i64::from(origin_y);
    crate::capture_options::compose_mask_cursor(
        image,
        (left, top),
        width,
        height,
        &mask,
        stride,
        pixels.as_ref(),
    )
}
fn geometry(window: &xcap::Window) -> Result<([i32; 4], [i32; 4], u32)> {
    let hwnd = window.id()? as usize as windows_sys::Win32::Foundation::HWND;
    let mut frame = RECT::default();
    ensure!(
        unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut frame as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )
        } >= 0,
        "Cannot query physical native window frame"
    );
    let mut client = RECT::default();
    ensure!(
        unsafe { GetClientRect(hwnd, &mut client) } != 0,
        "Cannot query native client bounds"
    );
    let mut start = POINT {
        x: client.left,
        y: client.top,
    };
    let mut end = POINT {
        x: client.right,
        y: client.bottom,
    };
    ensure!(
        unsafe { ClientToScreen(hwnd, &mut start) } != 0
            && unsafe { ClientToScreen(hwnd, &mut end) } != 0,
        "Cannot map native client coordinates"
    );
    Ok((
        [frame.left, frame.top, frame.right, frame.bottom],
        [start.x, start.y, end.x, end.y],
        window.pid()?,
    ))
}
pub(super) fn capture_window(window: &xcap::Window, options: &CaptureOptions) -> Result<RgbaImage> {
    let _dpi = DpiScope::new()?;
    let before = geometry(window)?;
    let mut image = window.capture_image()?;
    let full = before.0;
    ensure!(
        i64::from(full[2]) - i64::from(full[0]) == i64::from(image.width())
            && i64::from(full[3]) - i64::from(full[1]) == i64::from(image.height()),
        "Native window capture disagrees with physical frame bounds"
    );
    let mut origin = (full[0], full[1]);
    if options.window_frame == CaptureWindowFrame::Client {
        let client = before.1;
        ensure!(
            client[0] >= full[0]
                && client[1] >= full[1]
                && client[2] <= full[2]
                && client[3] <= full[3]
                && client[0] < client[2]
                && client[1] < client[3],
            "Native client bounds do not fit captured frame"
        );
        image = image::imageops::crop_imm(
            &image,
            (client[0] - full[0]) as u32,
            (client[1] - full[1]) as u32,
            (client[2] - client[0]) as u32,
            (client[3] - client[1]) as u32,
        )
        .to_image();
        origin = (client[0], client[1]);
    }
    options.flatten_window(&mut image);
    if options.cursor == CaptureCursor::Include {
        apply_cursor(&mut image, origin.0, origin.1)?;
    }
    ensure!(
        geometry(window)? == before,
        "Native window moved, resized or changed process during capture"
    );
    Ok(image)
}
