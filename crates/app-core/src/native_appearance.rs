//! Native Win32 popup menus follow a dark app appearance.
//!
//! Tray and system `HMENU` popups stay classic-light unless the process opts in
//! through uxtheme `SetPreferredAppMode` (ordinal 135) and `FlushMenuThemes`
//! (ordinal 136). Those entry points are undocumented; missing them leaves the
//! stock light menu. Explorer jump lists still follow the system, not the app.
//! Palette-colored owner-draw stays out: macOS/Linux will use native dark menus
//! the same way.

use std::sync::OnceLock;
use windows_sys::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
    },
    w,
};

const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

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

#[cfg(test)]
mod tests {
    #[test]
    fn dark_native_menus_resolve_on_current_windows() {
        assert!(
            super::apply_native_menus(true),
            "Windows 10 1903+ exposes SetPreferredAppMode and FlushMenuThemes"
        );
    }
}
