//! Absolute one-shot waitable timers and an operation-owned console cancellation event.
use anyhow::{Result, ensure};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Console::*, Threading::*},
};
struct Control {
    event: OwnedHandle,
    stop: Arc<AtomicBool>,
}
static CONTROL: OnceLock<Mutex<Option<Weak<Control>>>> = OnceLock::new();
unsafe extern "system" fn console(signal: u32) -> i32 {
    if !matches!(
        signal,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        return 0;
    }
    let value = CONTROL
        .get()
        .and_then(|m| m.lock().ok()?.as_ref()?.upgrade());
    if let Some(control) = value {
        control.stop.store(true, Ordering::SeqCst);
        unsafe {
            SetEvent(control.event.as_raw_handle());
        }
        return 1;
    }
    0
}
fn timer() -> Result<OwnedHandle> {
    let handle = unsafe { CreateWaitableTimerW(std::ptr::null(), 0, std::ptr::null()) };
    ensure!(
        !handle.is_null(),
        "Could not create schedule waitable timer"
    );
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
fn arm(timer: &OwnedHandle, ms: Option<i64>, wake: bool) -> Result<()> {
    let Some(ms) = ms else {
        ensure!(
            unsafe { CancelWaitableTimer(timer.as_raw_handle()) } != 0,
            "Could not disarm native schedule timer"
        );
        // Cancellation preserves an expired signal; consume it from this auto-reset timer.
        let state = unsafe { WaitForSingleObject(timer.as_raw_handle(), 0) };
        ensure!(
            state == WAIT_OBJECT_0 || state == WAIT_TIMEOUT,
            "Could not drain disarmed schedule timer"
        );
        return Ok(());
    };
    let due = ms
        .checked_mul(10000)
        .and_then(|n| n.checked_add(116444736000000000))
        .ok_or_else(|| anyhow::anyhow!("Schedule UTC overflows native timer"))?;
    unsafe {
        SetLastError(ERROR_SUCCESS);
    }
    ensure!(
        unsafe {
            SetWaitableTimer(
                timer.as_raw_handle(),
                &due,
                0,
                None,
                std::ptr::null(),
                i32::from(wake),
            )
        } != 0,
        "Could not arm native schedule timer: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        !wake || unsafe { GetLastError() } != ERROR_NOT_SUPPORTED,
        "Explicit schedule wake is unsupported by this system"
    );
    Ok(())
}
pub(super) struct Native {
    timer: OwnedHandle,
    alarm: Option<OwnedHandle>,
    control: Arc<Control>,
}
impl Native {
    pub fn new(stop: Arc<AtomicBool>) -> Result<Self> {
        let raw = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        ensure!(
            !raw.is_null(),
            "Could not create schedule cancellation event"
        );
        let control = Arc::new(Control {
            event: unsafe { OwnedHandle::from_raw_handle(raw) },
            stop,
        });
        {
            let mut slot = CONTROL
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|_| anyhow::anyhow!("Schedule cancellation state was poisoned"))?;
            ensure!(
                slot.as_ref().and_then(Weak::upgrade).is_none(),
                "Only one native scheduler can run in this process"
            );
            *slot = Some(Arc::downgrade(&control));
        }
        let native = Self {
            timer: timer()?,
            alarm: None,
            control,
        };
        ensure!(
            unsafe { SetConsoleCtrlHandler(Some(console), 1) } != 0,
            "Could not register scheduler console cancellation"
        );
        Ok(native)
    }
    pub fn arm(&mut self, next: Option<i64>, wake: Option<i64>) -> Result<()> {
        arm(&self.timer, next, false)?;
        if wake.is_some() && self.alarm.is_none() {
            self.alarm = Some(timer()?);
        }
        if let Some(alarm) = &self.alarm {
            arm(alarm, wake, true)?;
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<()> {
        if self.control.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let handles = [
            self.control.event.as_raw_handle(),
            self.timer.as_raw_handle(),
            self.alarm
                .as_ref()
                .map_or(std::ptr::null_mut(), AsRawHandle::as_raw_handle),
        ];
        let count = if self.alarm.is_some() { 3 } else { 2 };
        let result = unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, INFINITE) };
        ensure!(
            (WAIT_OBJECT_0..WAIT_OBJECT_0 + count).contains(&result),
            "Native schedule wait failed: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            SetConsoleCtrlHandler(Some(console), 0);
            CancelWaitableTimer(self.timer.as_raw_handle());
            if let Some(a) = &self.alarm {
                CancelWaitableTimer(a.as_raw_handle());
            }
        }
        if let Some(slot) = CONTROL.get() {
            if let Ok(mut slot) = slot.lock() {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_or_paused_schedule_timer_cannot_keep_idle_wait_ready() {
        let handle = timer().unwrap();
        arm(&handle, Some(0), false).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        arm(&handle, None, false).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 25) },
            WAIT_TIMEOUT
        );
    }
    #[test]
    fn simultaneous_normal_and_alarm_signals_are_drained_when_disarmed() {
        let normal = timer().unwrap();
        let alarm = timer().unwrap();
        arm(&normal, Some(0), false).unwrap();
        arm(&alarm, Some(0), false).unwrap();
        let handles = [normal.as_raw_handle(), alarm.as_raw_handle()];
        let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 1000) };
        assert!(result == WAIT_OBJECT_0 || result == WAIT_OBJECT_0 + 1);
        arm(&normal, None, false).unwrap();
        arm(&alarm, None, false).unwrap();
        assert_eq!(
            unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 25) },
            WAIT_TIMEOUT
        );
    }
}
