//! Main-app services only. Construct after launch ownership and platform startup.
use crate::{CaptureCommand, CaptureHotkeys, TrayCommand};
use anyhow::Result;
use std::{future::Future, pin::Pin};
use tokio::sync::mpsc;
#[cfg(any(target_os = "macos", test))]
#[path = "unix_desktop_input_gate.rs"]
mod input_gate;

#[cfg(target_os = "linux")]
#[path = "unix_desktop_services_linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "unix_desktop_services_macos.rs"]
mod platform;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum UnixDesktopAction {
    Capture(CaptureCommand),
    Tray(TrayCommand),
}

#[cfg_attr(target_os = "linux", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(target_os = "linux", serde(deny_unknown_fields))]
pub enum UnixDesktopEvent {
    #[cfg(target_os = "linux")]
    Stamped {
        action: UnixDesktopAction,
        generation: u64,
    },
    Action(UnixDesktopAction),
    TrayStatus {
        available: bool,
        status: String,
    },
    ShortcutStatus(String),
    #[cfg(target_os = "macos")]
    Hotkey {
        event: global_hotkey::GlobalHotKeyEvent,
        generation: u64,
    },
    #[cfg(target_os = "macos")]
    Menu {
        event: tray_icon::menu::MenuEvent,
        generation: u64,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnixShortcutReport {
    pub status: String,
    pub outcome: std::result::Result<(), String>,
}
impl UnixShortcutReport {
    pub(super) fn new(status: String, outcome: Result<()>) -> Self {
        Self {
            status: bounded_status(&status),
            outcome: outcome.map_err(|error| bounded_status(&format!("{error:#}"))),
        }
    }
    fn unavailable(message: &str) -> Self {
        Self {
            status: bounded_status(message),
            outcome: Err(bounded_status(message)),
        }
    }
}
pub type UnixShortcutOperation = Pin<Box<dyn Future<Output = UnixShortcutReport>>>;

/// Not cloneable: a single controller owns registrations through cleanup.
pub struct UnixDesktopServices(platform::Services);
impl UnixDesktopServices {
    /// `wayland_session` includes original inherited authority, even after GPUI takes its fd.
    /// `x11_display` comes from GPUI's preserved environment, never from an XWayland fallback.
    pub fn start(
        owner: std::sync::Arc<std::sync::Mutex<crate::UnixSingleInstance>>,
        wayland_session: bool,
        x11_display: Option<String>,
    ) -> Result<(Self, mpsc::Receiver<UnixDesktopEvent>)> {
        let (services, events) = platform::Services::start(owner, wayland_session, x11_display)?;
        Ok((Self(services), events))
    }
    pub fn initialize(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        self.0.initialize(settings)
    }
    pub fn prepare(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        self.0.prepare(settings)
    }
    pub fn finish(&mut self, commit: bool) -> UnixShortcutOperation {
        self.0.finish(commit)
    }
    pub fn set_quitting(&mut self, quitting: bool) {
        self.0.set_quitting(quitting);
    }
    pub fn set_repeat_region_available(&self, available: bool) {
        self.0.set_repeat_region_available(available);
    }
    pub fn shortcut_status(&self) -> String {
        bounded_status(&self.0.shortcut_status())
    }
    pub fn tray_status(&self) -> String {
        bounded_status(&self.0.tray_status())
    }
    pub fn tray_available(&self) -> bool {
        self.0.tray_available()
    }
    pub fn action(&mut self, event: UnixDesktopEvent) -> Option<UnixDesktopAction> {
        self.0.action(event)
    }
    /// Linux joins on an existing background executor; macOS drops on this native thread.
    #[cfg(target_os = "linux")]
    pub fn shutdown(self) -> impl Future<Output = Result<()>> + Send {
        self.0.shutdown()
    }
}

pub(super) fn bounded_status(value: &str) -> String {
    let mut result = String::with_capacity(value.len().min(4096));
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if result.len() + character.len_utf8() > 4093 {
            result.push('…');
            break;
        }
        result.push(character);
    }
    result
}

pub(super) fn deliver(sender: &mpsc::Sender<UnixDesktopEvent>, event: UnixDesktopEvent) {
    let event = match event {
        UnixDesktopEvent::TrayStatus { available, status } => UnixDesktopEvent::TrayStatus {
            available,
            status: bounded_status(&status),
        },
        UnixDesktopEvent::ShortcutStatus(status) => {
            UnixDesktopEvent::ShortcutStatus(bounded_status(&status))
        }
        event => event,
    };
    #[cfg(target_os = "linux")]
    let event = match platform::emit_to_helper(event) {
        Ok(()) => return,
        Err(event) => event,
    };
    // Native callbacks never wait for input, storage, or the foreground executor.
    let _ = sender.try_send(event);
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub use platform::{X11ShortcutRegistrar, shortcut_portal_owner, shortcut_portal_request};

#[cfg(test)]
mod status_tests {
    #[test]
    fn status_retention_is_bounded_and_keeps_utf8_valid() {
        let status = super::bounded_status(&format!("A\n{}", "🙂".repeat(5000)));
        assert!(status.len() <= 4096);
        assert!(status.starts_with("A "));
        assert!(status.ends_with('…'));
        assert!(!status.chars().any(char::is_control));
    }
}

#[cfg(target_os = "linux")]
#[doc(hidden)]
pub use platform::{
    LinuxDesktopProcess, LinuxDesktopProcessOwner, LinuxTaskPortal, install_linux_desktop_spawner,
    serve_linux_desktop_child,
};
