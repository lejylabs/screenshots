//! Bounded native-event bridge; receiving does not require a Tokio runtime.
use std::sync::{Mutex, Once, OnceLock};
use tokio::sync::mpsc;

pub enum DesktopEvent {
    Hotkey(global_hotkey::GlobalHotKeyEvent),
    Menu(tray_icon::menu::MenuEvent),
}

static SENDER: OnceLock<Mutex<Option<mpsc::Sender<DesktopEvent>>>> = OnceLock::new();
static INSTALL: Once = Once::new();

fn deliver(event: DesktopEvent) {
    if let Some(sender) = SENDER
        .get()
        .and_then(|slot| slot.lock().ok())
        .and_then(|slot| slot.clone())
    {
        if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(event) {
            tracing::warn!("desktop event queue full; action ignored, retry when idle");
        }
    }
}

pub fn desktop_events() -> mpsc::Receiver<DesktopEvent> {
    let (sender, receiver) = mpsc::channel(32);
    *SENDER.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(sender);
    INSTALL.call_once(|| {
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            |event: global_hotkey::GlobalHotKeyEvent| {
                if event.state == global_hotkey::HotKeyState::Pressed {
                    deliver(DesktopEvent::Hotkey(event));
                }
            },
        ));
        tray_icon::menu::MenuEvent::set_event_handler(Some(|event| {
            deliver(DesktopEvent::Menu(event))
        }));
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_delivers_and_stays_bounded() {
        let mut receiver = desktop_events();
        for id in 0..40 {
            deliver(DesktopEvent::Hotkey(global_hotkey::GlobalHotKeyEvent {
                id,
                state: global_hotkey::HotKeyState::Pressed,
            }));
        }
        for expected in 0..32 {
            assert!(
                matches!(receiver.try_recv(), Ok(DesktopEvent::Hotkey(event)) if event.id == expected)
            );
        }
        assert!(receiver.try_recv().is_err());
    }
}
