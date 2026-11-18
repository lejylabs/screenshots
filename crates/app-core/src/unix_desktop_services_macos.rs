//! Carbon and NSStatusItem are owned by the existing Cocoa/GPUI main loop.
use super::input_gate::{CaptureInputGate, QUIT, TRANSACTION};
use super::*;
use crate::{CaptureHotkey, SpikeTray};
use std::{
    collections::HashSet,
    rc::Rc,
    sync::{Mutex, Once, OnceLock},
};

static EVENTS: OnceLock<Mutex<Option<mpsc::Sender<UnixDesktopEvent>>>> = OnceLock::new();
static INSTALL: Once = Once::new();
static INPUT: OnceLock<std::sync::Arc<CaptureInputGate>> = OnceLock::new();
fn input() -> &'static std::sync::Arc<CaptureInputGate> {
    INPUT.get_or_init(|| std::sync::Arc::new(CaptureInputGate::new()))
}
static DOWN: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
fn native_event(event: UnixDesktopEvent) {
    if let Some(sender) = EVENTS
        .get()
        .and_then(|s| s.lock().ok())
        .and_then(|s| s.clone())
    {
        deliver(&sender, event);
    }
}

pub(super) struct Services {
    hotkeys: CaptureHotkey,
    tray: Option<SpikeTray>,
    tray_status: String,
    gate: std::sync::Arc<CaptureInputGate>,
    // The dependency marks its Carbon manager Send; Cocoa ownership is stricter.
    _main_thread: std::marker::PhantomData<Rc<()>>,
    _owner: std::sync::Arc<Mutex<crate::UnixSingleInstance>>,
}
impl Services {
    pub(super) fn start(
        owner: std::sync::Arc<Mutex<crate::UnixSingleInstance>>,
        _: bool,
        _: Option<String>,
    ) -> Result<(Self, mpsc::Receiver<UnixDesktopEvent>)> {
        anyhow::ensure!(
            objc2_foundation::MainThreadMarker::new().is_some(),
            "Native desktop services require the Cocoa main thread"
        );
        let gate = input().clone();
        gate.new_incarnation();
        if let Some(down) = DOWN.get() {
            down.lock().unwrap().clear();
        }
        let (sender, receiver) = mpsc::channel(32);
        *EVENTS.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(sender.clone());
        INSTALL.call_once(|| {
            global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
                |event: global_hotkey::GlobalHotKeyEvent| {
                    let mut down = DOWN
                        .get_or_init(|| Mutex::new(HashSet::new()))
                        .lock()
                        .unwrap();
                    if event.state == global_hotkey::HotKeyState::Released {
                        down.remove(&event.id);
                        return;
                    }
                    if down.len() >= 12 || !down.insert(event.id) {
                        return;
                    }
                    drop(down);
                    if let Some(generation) = input().capture_stamp() {
                        native_event(UnixDesktopEvent::Hotkey { event, generation });
                    }
                },
            ));
            tray_icon::menu::MenuEvent::set_event_handler(Some(|event| {
                native_event(UnixDesktopEvent::Menu {
                    event,
                    generation: input().epoch(),
                })
            }));
        });
        let hotkeys = CaptureHotkey::unregistered()?;
        let (tray, status) = match SpikeTray::new() {
            Ok(tray) => (Some(tray), "Tray ready".to_owned()),
            Err(error) => (None, format!("Tray unavailable: {error:#}; Close quits")),
        };
        deliver(
            &sender,
            UnixDesktopEvent::TrayStatus {
                available: tray.is_some(),
                status: status.clone(),
            },
        );
        Ok((
            Self {
                _owner: owner,
                hotkeys,
                tray,
                tray_status: status,
                gate,
                _main_thread: std::marker::PhantomData,
            },
            receiver,
        ))
    }
    fn report(&self, result: Result<()>) -> UnixShortcutOperation {
        let report = UnixShortcutReport::new(self.hotkeys.status().into(), result);
        Box::pin(std::future::ready(report))
    }
    pub(super) fn initialize(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        // CaptureHotkeys validation admits only normal Carbon keys, never media-event taps.
        self.gate.set(TRANSACTION, true);
        let result = self.hotkeys.register_settings(settings);
        self.terminal();
        self.report(result)
    }
    pub(super) fn prepare(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        self.gate.set(TRANSACTION, true);
        let result = self.hotkeys.prepare(settings);
        if result.is_err() {
            self.terminal();
        }
        self.report(result)
    }
    pub(super) fn finish(&mut self, commit: bool) -> UnixShortcutOperation {
        self.gate.set(TRANSACTION, true);
        let result = self.hotkeys.finish(commit);
        self.terminal();
        self.report(result)
    }
    fn terminal(&self) {
        self.gate.set(TRANSACTION, false);
        if let Some(down) = DOWN.get() {
            let active = self.hotkeys.registered_ids();
            down.lock().unwrap().retain(|id| active.contains(id));
        }
    }
    pub(super) fn set_quitting(&mut self, quitting: bool) {
        self.gate.set(QUIT, quitting);
    }
    pub(super) fn set_repeat_region_available(&self, available: bool) {
        if let Some(tray) = &self.tray {
            tray.set_repeat_region_available(available);
        }
    }
    pub(super) fn shortcut_status(&self) -> String {
        self.hotkeys.status().to_owned()
    }
    pub(super) fn tray_status(&self) -> String {
        self.tray_status.clone()
    }
    pub(super) fn tray_available(&self) -> bool {
        self.tray.is_some()
    }
    pub(super) fn action(&mut self, event: UnixDesktopEvent) -> Option<UnixDesktopAction> {
        match event {
            UnixDesktopEvent::Hotkey { event, generation } => {
                if !self.gate.accepts(generation, false) {
                    return None;
                }
                self.hotkeys.command(event).map(UnixDesktopAction::Capture)
            }
            UnixDesktopEvent::Menu { event, generation } => {
                let command = self.tray.as_ref()?.command(event)?;
                let allowed = match command {
                    TrayCommand::Show => true,
                    TrayCommand::Quit => self.gate.accepts_quit(generation),
                    _ => self.gate.accepts(generation, false),
                };
                allowed.then_some(UnixDesktopAction::Tray(command))
            }
            UnixDesktopEvent::Action(action)
                if matches!(action, UnixDesktopAction::Tray(TrayCommand::Show)) =>
            {
                Some(action)
            }
            _ => None,
        }
    }
}
impl Drop for Services {
    fn drop(&mut self) {
        self.gate.set(QUIT, true);
        if let Some(down) = DOWN.get() {
            down.lock().unwrap().clear();
        }
        if let Some(slot) = EVENTS.get() {
            *slot.lock().unwrap() = None;
        }
        self.tray.take();
        // hotkeys follow immediately, on the same Cocoa thread.
    }
}
