//! Operation-local calibration and native topology witness; no resident work.
use super::DpiScope;
use crate::{
    DisplayGeometry,
    display_pixels::{PhysicalMode, calibrate},
};
use anyhow::{Context, Result, ensure};
use std::{
    marker::PhantomData,
    mem::size_of,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};

struct Window(HWND, PhantomData<Rc<()>>);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

pub(crate) fn monitor_geometry(monitor: &xcap::Monitor) -> Result<DisplayGeometry> {
    let _dpi = DpiScope::new()?;
    let name = monitor.name()?;
    ensure!(
        !name.is_empty() && name.len() < 128 && !name.chars().any(char::is_control),
        "Invalid native display device name"
    );
    let name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    ensure!(
        unsafe { EnumDisplaySettingsW(name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) } != 0,
        "Cannot read physical display mode"
    );
    let required = DM_POSITION | DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION;
    ensure!(
        mode.dmFields & required == required,
        "Incomplete physical display mode"
    );
    let oriented = unsafe { mode.Anonymous1.Anonymous2 };
    let rotation = match oriented.dmDisplayOrientation {
        DMDO_DEFAULT => 0.0,
        DMDO_90 => 90.0,
        DMDO_180 => 180.0,
        DMDO_270 => 270.0,
        _ => anyhow::bail!("Unknown physical display rotation"),
    };
    let physical = PhysicalMode {
        x: oriented.dmPosition.x,
        y: oriented.dmPosition.y,
        width: mode.dmPelsWidth,
        height: mode.dmPelsHeight,
        rotation,
    };
    crate::validate_image_size(physical.width, physical.height)?;
    let center = POINT {
        x: i32::try_from(i64::from(physical.x) + i64::from(physical.width) / 2)?,
        y: i32::try_from(i64::from(physical.y) + i64::from(physical.height) / 2)?,
    };
    let native = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) };
    ensure!(!native.is_null(), "Physical display disappeared");
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    ensure!(
        unsafe { GetMonitorInfoW(native, (&mut info as *mut MONITORINFOEXW).cast()) } != 0,
        "Cannot read physical monitor rectangle"
    );
    let end = info
        .szDevice
        .iter()
        .position(|c| *c == 0)
        .context("Unterminated native device name")?;
    ensure!(
        info.szDevice[..end] == name[..name.len() - 1],
        "Physical monitor device is ambiguous or changed"
    );
    const STATIC: &[u16] = &[83, 84, 65, 84, 73, 67, 0];
    let window = Window(
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                STATIC.as_ptr(),
                ptr::null(),
                WS_POPUP,
                center.x,
                center.y,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        },
        PhantomData,
    );
    ensure!(!window.0.is_null(), "Cannot calibrate native monitor DPI");
    ensure!(
        unsafe { MonitorFromWindow(window.0, MONITOR_DEFAULTTONULL) } == native,
        "Calibration window changed monitor"
    );
    let dpi = unsafe { GetDpiForWindow(window.0) };
    let backend = PhysicalMode {
        x: monitor.x()?,
        y: monitor.y()?,
        width: monitor.width()?,
        height: monitor.height()?,
        rotation: monitor.rotation()?,
    };
    let rect = info.monitorInfo.rcMonitor;
    Ok(calibrate(
        physical,
        [rect.left, rect.top, rect.right, rect.bottom],
        backend,
        dpi,
    )?)
}

unsafe extern "system" fn witness_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if matches!(
            message,
            WM_DISPLAYCHANGE | WM_DEVICECHANGE | WM_SETTINGCHANGE | WM_POWERBROADCAST
        ) {
            let changed = GetWindowLongPtrW(window, GWLP_USERDATA) as *const AtomicBool;
            if let Some(changed) = changed.as_ref() {
                changed.store(true, Ordering::Release);
            }
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}
type TopologyId = unsafe extern "system" fn() -> u32;
fn generation() -> Option<(TopologyId, u32)> {
    const USER32: &[u16] = &[117, 115, 101, 114, 51, 50, 46, 100, 108, 108, 0];
    unsafe {
        let module = GetModuleHandleW(USER32.as_ptr());
        if module.is_null() {
            return None;
        }
        let function = GetProcAddress(module, c"GetCurrentMonitorTopologyId".as_ptr().cast())?;
        let function: TopologyId = std::mem::transmute(function);
        Some((function, function()))
    }
}

pub(crate) struct TopologyGuard {
    window: HWND,
    class: Vec<u16>,
    changed: Box<AtomicBool>,
    generation: Option<(TopologyId, u32)>,
    _dpi: DpiScope,
    _thread: PhantomData<Rc<()>>,
}
impl TopologyGuard {
    pub(crate) fn new() -> Result<Self> {
        Self::with_generation(generation)
    }
    fn with_generation(lookup: impl FnOnce() -> Option<(TopologyId, u32)>) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dpi = DpiScope::new()?;
        let class = format!(
            "ScreenshotsCaptureWitness-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
        let mut guard = Self {
            window: ptr::null_mut(),
            class,
            changed: Box::new(AtomicBool::new(false)),
            generation: None,
            _dpi: dpi,
            _thread: PhantomData,
        };
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let definition = WNDCLASSW {
            hInstance: instance,
            lpszClassName: guard.class.as_ptr(),
            lpfnWndProc: Some(witness_proc),
            ..Default::default()
        };
        ensure!(
            unsafe { RegisterClassW(&definition) } != 0,
            "Cannot register native topology witness"
        );
        guard.window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                guard.class.as_ptr(),
                ptr::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                (&mut *guard.changed as *mut AtomicBool).cast(),
            )
        };
        ensure!(
            !guard.window.is_null(),
            "Cannot create native topology witness"
        );
        ensure!(
            unsafe { GetWindowLongPtrW(guard.window, GWLP_USERDATA) }
                == (&*guard.changed as *const AtomicBool) as isize,
            "Native topology witness was not attached"
        );
        guard.drain()?;
        guard.changed.store(false, Ordering::Release);
        guard.generation = lookup();
        Ok(guard)
    }
    fn drain(&self) -> Result<()> {
        let mut message = MSG::default();
        for _ in 0..256 {
            if unsafe { PeekMessageW(&mut message, self.window, 0, 0, PM_REMOVE) } == 0 {
                return Ok(());
            }
            if message.message == WM_QUIT {
                unsafe {
                    PostQuitMessage(message.wParam as i32);
                }
                anyhow::bail!("Native capture thread is quitting");
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        anyhow::bail!("Native topology message storm exceeds its bound")
    }
    pub(crate) fn check(&self) -> Result<()> {
        self.drain()?;
        ensure!(
            unsafe { IsWindow(self.window) } != 0 && !self.changed.load(Ordering::Acquire),
            "Native display topology or power state changed during capture"
        );
        if let Some((function, initial)) = self.generation {
            ensure!(
                unsafe { function() } == initial,
                "Native display topology generation changed during capture"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_native_display_observations_agree_in_a_per_monitor_scope() -> Result<()> {
        let witness = TopologyGuard::new()?;
        let monitors = xcap::Monitor::all()?;
        ensure!(
            (1..=32).contains(&monitors.len()),
            "Native fixture needs a display"
        );
        for monitor in monitors {
            let geometry = monitor_geometry(&monitor)?;
            ensure!(geometry.width > 0 && geometry.height > 0);
            ensure!(geometry.scale_factor.is_finite() && geometry.scale_factor > 0.0);
        }
        witness.check()
    }
    #[test]
    fn fallback_witness_rejects_aba_between_identity_capture_and_verify_phases() -> Result<()> {
        let witness = TopologyGuard::with_generation(|| None)?;
        let identity_before = "physical-monitor-A";
        witness.check()?;
        // Both final identity observations can be A while a replacement B was
        // captured in between; only the continuously owned event witness sees it.
        ensure!(unsafe { PostMessageW(witness.window, WM_DISPLAYCHANGE, 0, 0) } != 0);
        let captured_identity = "physical-monitor-B";
        ensure!(unsafe { PostMessageW(witness.window, WM_DISPLAYCHANGE, 0, 0) } != 0);
        let identity_after = "physical-monitor-A";
        assert_eq!(identity_before, identity_after);
        assert_ne!(captured_identity, identity_before);
        assert!(witness.check().is_err());
        Ok(())
    }
    #[test]
    fn native_nonqueued_notification_is_observed_without_a_posted_message() -> Result<()> {
        let witness = TopologyGuard::with_generation(|| None)?;
        let window = witness.window as usize;
        assert!(
            std::thread::spawn(move || unsafe {
                SendNotifyMessageW(window as HWND, WM_DISPLAYCHANGE, 0, 0) != 0
            })
            .join()
            .unwrap()
        );
        assert!(witness.check().is_err());
        Ok(())
    }
    #[test]
    fn queued_message_storm_fails_closed_and_owned_class_is_released() -> Result<()> {
        let witness = TopologyGuard::with_generation(|| None)?;
        let class = witness.class.clone();
        for _ in 0..257 {
            ensure!(unsafe { PostMessageW(witness.window, WM_NULL, 0, 0) } != 0);
        }
        assert!(witness.check().is_err());
        drop(witness);
        let mut definition = WNDCLASSW::default();
        assert_eq!(
            unsafe {
                GetClassInfoW(
                    GetModuleHandleW(ptr::null()),
                    class.as_ptr(),
                    &mut definition,
                )
            },
            0
        );
        Ok(())
    }
    #[test]
    fn changed_generation_rejects_an_identical_restored_layout() -> Result<()> {
        static CURRENT: AtomicU64 = AtomicU64::new(1);
        unsafe extern "system" fn current() -> u32 {
            CURRENT.load(Ordering::Acquire) as u32
        }
        let witness = TopologyGuard::with_generation(|| Some((current, 1)))?;
        CURRENT.store(3, Ordering::Release);
        assert!(witness.check().is_err());
        Ok(())
    }
}
impl Drop for TopologyGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.window.is_null() {
                DestroyWindow(self.window);
            }
            UnregisterClassW(self.class.as_ptr(), GetModuleHandleW(ptr::null()));
        }
    }
}
