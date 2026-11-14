//! Native Win32 popup menus follow the in-app appearance.
//!
//! System `HMENU` chrome stays classic-light unless the process opts in through
//! uxtheme `SetPreferredAppMode` (ordinal 135) and `FlushMenuThemes` (136).
//! The tray popup is owner-drawn with the current palette so Dark, Oldschool
//! and Classic keep their own surface, text and hover. Explorer jump lists
//! still follow the system, not the app.

use crate::UiTheme;
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;
use windows_sys::{
    Win32::{
        Foundation::{FALSE, HMODULE, HWND, LPARAM, LRESULT, SIZE, TRUE, WPARAM},
        Graphics::Gdi::{
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW,
            CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT,
            DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, FW_NORMAL,
            FillRect, GetTextExtentPoint32W, HBRUSH, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode,
            SetTextColor, TRANSPARENT,
        },
        System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        UI::{
            Controls::{
                DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_GRAYED, ODS_SELECTED, ODT_MENU,
            },
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                GetMenuItemCount, GetMenuItemInfoW, MENUINFO, MENUITEMINFOW, MFT_OWNERDRAW,
                MFT_SEPARATOR, MIIM_DATA, MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIM_BACKGROUND,
                SetMenuInfo, SetMenuItemInfoW, WM_DRAWITEM, WM_MEASUREITEM,
            },
        },
    },
    w,
};

const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;
const TRAY_MENU_SUBCLASS_ID: usize = 0x5348_4F54;
const MENU_FONT_PX: i32 = 14;
const MENU_ITEM_HEIGHT_PX: i32 = 28;
const MENU_PAD_X_PX: i32 = 12;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

/// Tray popup colors taken from the product palette (`surface` / `text` / `muted` / `hover`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMenuColors {
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub hover: u32,
}

impl NativeMenuColors {
    pub fn for_theme(theme: UiTheme) -> Self {
        match theme {
            UiTheme::Default => Self {
                background: 0x1b1b1b,
                text: 0xededed,
                muted: 0xb0b0b0,
                hover: 0x323232,
            },
            UiTheme::Oldschool => Self {
                background: 0x282d35,
                text: 0xf0f2f5,
                muted: 0xc2cad6,
                hover: 0x404b5b,
            },
            UiTheme::Classic => Self {
                background: 0x2f343e,
                text: 0xdce0e5,
                muted: 0xa9afbc,
                hover: 0x363c46,
            },
        }
    }
}

fn uxtheme() -> HMODULE {
    static MODULE: OnceLock<usize> = OnceLock::new();
    *MODULE.get_or_init(|| {
        let handle = unsafe {
            LoadLibraryExW(
                w!("uxtheme.dll"),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        handle as usize
    }) as HMODULE
}

/// Make native popup menus dark or light to match the current appearance.
///
/// Returns whether both uxtheme entry points resolved. Older Windows keeps the
/// classic light menu when they are missing.
pub fn apply_native_menus(dark: bool) -> bool {
    let module = uxtheme();
    if module.is_null() {
        return false;
    }
    let Some(set_mode) = (unsafe { GetProcAddress(module, 135usize as *const u8) }) else {
        return false;
    };
    let Some(flush) = (unsafe { GetProcAddress(module, 136usize as *const u8) }) else {
        return false;
    };
    let set_mode = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, SetPreferredAppMode>(set_mode)
    };
    let flush = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushMenuThemes>(flush)
    };
    unsafe {
        set_mode(if dark { FORCE_DARK } else { FORCE_LIGHT });
        flush();
    }
    true
}

pub struct TrayMenuChrome {
    _subclass: TrayMenuSubclass,
    paint: Box<RefCell<TrayMenuPaint>>,
}

struct TrayMenuSubclass {
    hwnd: HWND,
}

struct TrayMenuPaint {
    hwnd: HWND,
    hmenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    colors: NativeMenuColors,
    labels: HashMap<u32, Vec<u16>>,
    background: HBRUSH,
    hover: HBRUSH,
}

impl TrayMenuChrome {
    pub fn install(
        hwnd: HWND,
        hmenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
        theme: UiTheme,
    ) -> Option<Self> {
        if hwnd.is_null() || hmenu.is_null() {
            return None;
        }
        apply_native_menus(theme.is_dark());
        let paint = Box::new(RefCell::new(TrayMenuPaint::new(hwnd, hmenu, theme)));
        let data = paint.as_ref() as *const RefCell<TrayMenuPaint> as usize;
        let attached = unsafe {
            SetWindowSubclass(hwnd, Some(tray_menu_subclass), TRAY_MENU_SUBCLASS_ID, data)
        } != FALSE;
        if !attached {
            return None;
        }
        paint.borrow_mut().enable_owner_draw();
        Some(Self {
            _subclass: TrayMenuSubclass { hwnd },
            paint,
        })
    }

    pub fn set_appearance(&self, theme: UiTheme) {
        apply_native_menus(theme.is_dark());
        self.paint
            .borrow_mut()
            .set_colors(NativeMenuColors::for_theme(theme));
    }
}

impl Drop for TrayMenuSubclass {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe {
                RemoveWindowSubclass(self.hwnd, Some(tray_menu_subclass), TRAY_MENU_SUBCLASS_ID);
            }
        }
    }
}

impl TrayMenuPaint {
    fn new(
        hwnd: HWND,
        hmenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
        theme: UiTheme,
    ) -> Self {
        let colors = NativeMenuColors::for_theme(theme);
        Self {
            hwnd,
            hmenu,
            background: solid_brush(colors.background),
            hover: solid_brush(colors.hover),
            colors,
            labels: HashMap::new(),
        }
    }

    fn set_colors(&mut self, colors: NativeMenuColors) {
        if self.colors == colors {
            return;
        }
        self.colors = colors;
        let background = solid_brush(colors.background);
        let hover = solid_brush(colors.hover);
        let old_background = self.background;
        let old_hover = self.hover;
        self.background = background;
        self.hover = hover;
        apply_menu_background(self.hmenu, self.background);
        delete_brush(old_background);
        delete_brush(old_hover);
    }

    fn enable_owner_draw(&mut self) {
        let count = unsafe { GetMenuItemCount(self.hmenu) };
        if count <= 0 {
            apply_menu_background(self.hmenu, self.background);
            return;
        }
        for index in 0..count as u32 {
            let mut buffer = vec![0u16; 256];
            let mut info = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID | MIIM_FTYPE | MIIM_STRING,
                dwTypeData: buffer.as_mut_ptr(),
                cch: (buffer.len() - 1) as u32,
                ..Default::default()
            };
            if unsafe { GetMenuItemInfoW(self.hmenu, index, TRUE, &mut info) } == FALSE {
                continue;
            }
            if info.fType & MFT_SEPARATOR != 0 {
                continue;
            }
            let mut text = buffer[..info.cch as usize].to_vec();
            text.push(0);
            self.labels.insert(info.wID, text);
            let update = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                fType: MFT_OWNERDRAW,
                dwItemData: info.wID as usize,
                ..Default::default()
            };
            unsafe { SetMenuItemInfoW(self.hmenu, index, TRUE, &update) };
        }
        apply_menu_background(self.hmenu, self.background);
    }
}

impl Drop for TrayMenuPaint {
    fn drop(&mut self) {
        delete_brush(self.background);
        delete_brush(self.hover);
    }
}

fn solid_brush(rgb: u32) -> HBRUSH {
    unsafe { CreateSolidBrush(colorref(rgb)) }
}

fn delete_brush(brush: HBRUSH) {
    if !brush.is_null() {
        unsafe { DeleteObject(brush) };
    }
}

fn apply_menu_background(hmenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU, brush: HBRUSH) {
    if brush.is_null() {
        return;
    }
    let info = MENUINFO {
        cbSize: size_of::<MENUINFO>() as u32,
        fMask: MIM_BACKGROUND,
        hbrBack: brush,
        ..Default::default()
    };
    unsafe { SetMenuInfo(hmenu, &info) };
}

fn colorref(rgb: u32) -> u32 {
    let red = (rgb >> 16) & 0xff;
    let green = (rgb >> 8) & 0xff;
    let blue = rgb & 0xff;
    red | (green << 8) | (blue << 16)
}

fn window_dpi(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 96 } else { dpi as i32 }
}

fn scale(px: i32, dpi: i32) -> i32 {
    (px * dpi + 48) / 96
}

fn menu_font(dpi: i32) -> windows_sys::Win32::Graphics::Gdi::HFONT {
    unsafe {
        CreateFontW(
            -scale(MENU_FONT_PX, dpi),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32 | FF_DONTCARE as u32,
            w!("Segoe UI"),
        )
    }
}

unsafe extern "system" fn tray_menu_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if data != 0 {
        let paint = unsafe { &*(data as *const RefCell<TrayMenuPaint>) };
        match msg {
            WM_MEASUREITEM => {
                if measure_item(hwnd, lparam, &paint.borrow()) {
                    return 1;
                }
            }
            WM_DRAWITEM => {
                if draw_item(lparam, &paint.borrow()) {
                    return 1;
                }
            }
            _ => {}
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn measure_item(hwnd: HWND, lparam: LPARAM, paint: &TrayMenuPaint) -> bool {
    if lparam == 0 {
        return false;
    }
    let measure = unsafe { &mut *(lparam as *mut MEASUREITEMSTRUCT) };
    if measure.CtlType != ODT_MENU {
        return false;
    }
    let dpi = window_dpi(hwnd);
    measure.itemHeight = scale(MENU_ITEM_HEIGHT_PX, dpi) as u32;
    let mut width = scale(160, dpi);
    if let Some(text) = paint.labels.get(&measure.itemID) {
        let hdc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        if !hdc.is_null() {
            let font = menu_font(dpi);
            let previous = if font.is_null() {
                std::ptr::null_mut()
            } else {
                unsafe { SelectObject(hdc, font) }
            };
            let mut size = SIZE { cx: 0, cy: 0 };
            let chars = text.len().saturating_sub(1) as i32;
            if unsafe { GetTextExtentPoint32W(hdc, text.as_ptr(), chars, &mut size) } != FALSE {
                width = size.cx + scale(MENU_PAD_X_PX, dpi) * 2;
            }
            if !font.is_null() {
                unsafe { SelectObject(hdc, previous) };
                unsafe { DeleteObject(font) };
            }
            unsafe { DeleteDC(hdc) };
        }
    }
    measure.itemWidth = width.max(0) as u32;
    true
}

fn draw_item(lparam: LPARAM, paint: &TrayMenuPaint) -> bool {
    if lparam == 0 {
        return false;
    }
    let draw = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
    if draw.CtlType != ODT_MENU {
        return false;
    }
    let selected = draw.itemState & ODS_SELECTED != 0;
    let disabled = draw.itemState & (ODS_DISABLED | ODS_GRAYED) != 0;
    let brush = if selected {
        paint.hover
    } else {
        paint.background
    };
    if !brush.is_null() {
        unsafe { FillRect(draw.hDC, &draw.rcItem, brush) };
    }
    let Some(text) = paint.labels.get(&draw.itemID) else {
        return true;
    };
    let dpi = window_dpi(paint.hwnd);
    let font = menu_font(dpi);
    let previous = if font.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { SelectObject(draw.hDC, font) }
    };
    unsafe { SetBkMode(draw.hDC, TRANSPARENT as i32) };
    let color = if disabled {
        paint.colors.muted
    } else {
        paint.colors.text
    };
    unsafe { SetTextColor(draw.hDC, colorref(color)) };
    let mut rect = draw.rcItem;
    let pad = scale(MENU_PAD_X_PX, dpi);
    rect.left += pad;
    rect.right -= pad;
    let chars = text.len().saturating_sub(1) as i32;
    unsafe {
        DrawTextW(
            draw.hDC,
            text.as_ptr(),
            chars,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        )
    };
    if !font.is_null() {
        unsafe { SelectObject(draw.hDC, previous) };
        unsafe { DeleteObject(font) };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_native_menus_resolve_on_current_windows() {
        assert!(
            apply_native_menus(true),
            "Windows 10 1903+ exposes SetPreferredAppMode and FlushMenuThemes"
        );
    }

    #[test]
    fn tray_menu_colors_are_distinct_per_appearance() {
        let dark = NativeMenuColors::for_theme(UiTheme::Default);
        let oldschool = NativeMenuColors::for_theme(UiTheme::Oldschool);
        let classic = NativeMenuColors::for_theme(UiTheme::Classic);
        assert_ne!(dark, oldschool);
        assert_ne!(dark, classic);
        assert_ne!(oldschool, classic);
        assert_eq!(dark.background, 0x1b1b1b);
        assert_eq!(classic.background, 0x2f343e);
    }

    #[test]
    fn colorref_swaps_rgb_to_bgr() {
        assert_eq!(colorref(0x74ade8), 0x00e8ad74);
    }
}
