//! Xdnd metadata on Enter, raster acquisition only on Drop, through an installed worker.
use super::*;
use gpui::{NativeDropData, NativeDropRead, NativeDropSource};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[path = "../wayland/drop_metadata.rs"]
mod drop_metadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    sequence: u64,
    root: [i16; 2],
    timestamp: u32,
}
#[derive(Debug, Default)]
pub(crate) struct Xdnd {
    source: u32,
    target: u32,
    types: [u32; 3],
    extended: bool,
    generation: u64,
    timestamp: u32,
    root_position: [i16; 2],
    root_offset: Option<[i16; 2]>,
    position: Point<Pixels>,
    ready: bool,
    dropped: bool,
    reading: bool,
    image: bool,
    position_sequence: u64,
    pending_position: Option<Position>,
    accepted_position: Option<Position>,
    cancel: Option<Arc<AtomicBool>>,
    active: Arc<AtomicBool>,
}
struct Lease(Arc<AtomicBool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Xdnd {
    pub(super) fn retire(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.generation = self.generation.saturating_add(1);
        self.source = 0;
        self.target = 0;
        self.ready = false;
        self.dropped = false;
        self.reading = false;
        self.image = false;
        self.root_offset = None;
        self.pending_position = None;
        self.accepted_position = None;
    }
    pub(super) fn cancel_target(&mut self, target: u32) {
        if self.target == target {
            self.retire();
        }
    }
    fn matches(&self, source: u32, target: u32) -> bool {
        source != 0 && self.source == source && self.target == target
    }
    fn current(&self, generation: u64, target: u32) -> bool {
        self.generation == generation && self.target == target && self.source != 0
    }
    fn acknowledge_position(&mut self, expected: Position, accepted: bool) -> Option<Position> {
        if self.pending_position != Some(expected) {
            return None;
        }
        let position = self.pending_position.take()?;
        self.accepted_position = accepted.then_some(position);
        Some(position)
    }
    fn position(&mut self, window: &X11WindowStatePtr) -> Point<Pixels> {
        if let Some(offset) = self.root_offset {
            self.position = window.native_drop_position([
                i32::from(self.root_position[0]) + i32::from(offset[0]),
                i32::from(self.root_position[1]) + i32::from(offset[1]),
            ]);
        }
        self.position
    }
}

impl X11Client {
    pub(super) fn invalidate_drop_target(&self, target: u32) {
        let mut state = self.0.borrow_mut();
        if state.xdnd_state.target != target {
            return;
        }
        let window = state
            .windows
            .get(&target)
            .map(|reference| reference.window.clone());
        if state.xdnd_state.dropped {
            send_finished(&state, target, state.xdnd_state.source, false);
        }
        reply_pending_position(&mut state, false);
        state.xdnd_state.retire();
        drop(state);
        if let Some(window) = window {
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
        }
    }

    /// Returns true only for Xdnd messages, including refused/stale ones.
    pub(super) fn handle_drop_message(&self, event: &ClientMessageEvent) -> bool {
        let mut state = self.0.borrow_mut();
        let atoms = &state.atoms;
        let kind = event.type_;
        if ![
            atoms.XdndEnter,
            atoms.XdndLeave,
            atoms.XdndPosition,
            atoms.XdndDrop,
        ]
        .contains(&kind)
        {
            return false;
        }
        if event.format != 32 {
            return true;
        }
        let [source, first, second, third, fourth] = event.data.as_data32();
        let target = event.window;
        let Some(window) = state
            .windows
            .get(&target)
            .map(|reference| reference.window.clone())
        else {
            return true;
        };
        if kind == state.atoms.XdndEnter {
            let old = state
                .windows
                .get(&state.xdnd_state.target)
                .map(|reference| reference.window.clone());
            state.xdnd_state.retire();
            if (first >> 24) >= 3
                && source != 0
                && gpui::native_drop_reader().is_some()
                && state.xdnd_state.generation != u64::MAX
            {
                state.xdnd_state.source = source;
                state.xdnd_state.target = target;
                state.xdnd_state.types = [second, third, fourth];
                state.xdnd_state.extended = first & 1 != 0;
            }
            drop(state);
            if let Some(old) = old {
                old.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
            }
            return true;
        }
        if !state.xdnd_state.matches(source, target) {
            return true;
        }
        if kind == state.atoms.XdndLeave {
            // A source may leave after Drop while its promised bytes are still arriving.
            if state.xdnd_state.dropped {
                return true;
            }
            state.xdnd_state.retire();
            drop(state);
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
        } else if kind == state.atoms.XdndPosition {
            if state.xdnd_state.dropped {
                return true;
            }
            // A conforming source waits for Status before its next Position.
            // Never replace the one pending owner with an unacknowledged point.
            if state.xdnd_state.pending_position.is_some() {
                return true;
            }
            let Some(sequence) = state.xdnd_state.position_sequence.checked_add(1) else {
                state.xdnd_state.retire();
                return true;
            };
            state.xdnd_state.position_sequence = sequence;
            let native_position = Position {
                sequence,
                root: [(second >> 16) as u16 as i16, second as u16 as i16],
                timestamp: third,
            };
            state.xdnd_state.root_position = native_position.root;
            state.xdnd_state.timestamp = native_position.timestamp;
            state.xdnd_state.pending_position = Some(native_position);
            state.xdnd_state.accepted_position = None;
            let position = state.xdnd_state.position(&window);
            let generation = state.xdnd_state.generation;
            let valid = third != 0 && fourth != 0;
            let begin = valid && !state.xdnd_state.ready && !state.xdnd_state.reading;
            if !valid {
                reply_pending_position(&mut state, false);
            } else if state.xdnd_state.ready {
                reply_pending_position(&mut state, true);
            }
            // Otherwise the bounded metadata read owns the sole deferred Status.
            drop(state);
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Pending { position }));
            if begin {
                self.start_drop_read(generation, false);
            }
        } else if kind == state.atoms.XdndDrop {
            if state.xdnd_state.dropped {
                return true;
            }
            let accepted = state
                .xdnd_state
                .accepted_position
                .filter(|_| second != 0 && state.xdnd_state.pending_position.is_none());
            let Some(accepted) = accepted else {
                reply_pending_position(&mut state, false);
                send_finished(&state, target, source, false);
                state.xdnd_state.retire();
                drop(state);
                window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
                return true;
            };
            state.xdnd_state.root_position = accepted.root;
            state.xdnd_state.position(&window);
            state.xdnd_state.dropped = true;
            // XdndDrop's timestamp is data[2], not the last movement's timestamp.
            state.xdnd_state.timestamp = second;
            let generation = state.xdnd_state.generation;
            let ready = state.xdnd_state.ready;
            let image = state.xdnd_state.image;
            let reading = state.xdnd_state.reading;
            drop(state);
            if !reading {
                self.start_drop_read(generation, ready && image);
            }
        }
        true
    }

    fn start_drop_read(&self, generation: u64, image: bool) {
        let mut state = self.0.borrow_mut();
        let Some(reader) = gpui::native_drop_reader() else {
            return;
        };
        let Some(display) = state
            .graphical_environment
            .x11_display
            .as_ref()
            .and_then(|display| display.to_str())
            .map(Arc::<str>::from)
        else {
            return;
        };
        let drag = &mut state.xdnd_state;
        if drag.generation != generation || drag.source == 0 || drag.timestamp == 0 || drag.reading
        {
            return;
        }
        if drag
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // A completed canceled body still owns the sole native read slot.
            let (target, source, dropped) = (drag.target, drag.source, drag.dropped);
            if dropped {
                send_finished(&state, target, source, false);
            }
            reply_pending_position(&mut state, false);
            state.xdnd_state.retire();
            let window = state
                .windows
                .get(&target)
                .map(|reference| reference.window.clone());
            drop(state);
            if let Some(window) = window {
                window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
            }
            return;
        }
        let lease = Lease(drag.active.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        drag.cancel = Some(cancel.clone());
        drag.reading = true;
        let target = drag.target;
        let final_read = drag.dropped;
        let status_owner = drag.pending_position;
        let source = NativeDropSource::X11 {
            display,
            source: drag.source,
            target,
            types: drag.types,
            extended: drag.extended,
            timestamp: drag.timestamp,
        };
        let background = state.common.borrow().background_executor.clone();
        let foreground = state.common.borrow().foreground_executor.clone();
        let weak = Rc::downgrade(&self.0);
        drop(state);
        let read = background.spawn(async move {
            let value = reader(source, image, &cancel);
            (value, lease)
        });
        foreground
            .spawn(async move {
                let (value, lease) = read.await;
                let Some(client) = weak.upgrade() else {
                    return;
                };
                let client = X11Client(client);
                client.complete_drop_read(
                    generation,
                    target,
                    final_read,
                    status_owner,
                    value,
                    lease,
                );
            })
            .detach();
    }

    fn complete_drop_read(
        &self,
        generation: u64,
        target: u32,
        final_read: bool,
        status_owner: Option<Position>,
        value: Option<NativeDropRead>,
        lease: Lease,
    ) {
        let mut state = self.0.borrow_mut();
        if !state.xdnd_state.current(generation, target) {
            return;
        }
        if !final_read
            && (status_owner.is_none() || state.xdnd_state.pending_position != status_owner)
        {
            return;
        }
        let Some(window) = state
            .windows
            .get(&target)
            .map(|reference| reference.window.clone())
        else {
            state.xdnd_state.retire();
            return;
        };
        let result = value.and_then(|value| {
            state.xdnd_state.root_offset = value.root_offset;
            let position = state.xdnd_state.position(&window);
            match value.data {
                NativeDropData::Image { format, bytes } => {
                    state.xdnd_state.image = true;
                    Some((
                        FileDropEvent::ImageEntered {
                            position,
                            image: gpui::ExternalImageDrop {
                                format,
                                bytes: bytes.clone(),
                            },
                        },
                        bytes.is_none(),
                    ))
                }
                NativeDropData::UriList(bytes) => {
                    state.xdnd_state.image = false;
                    match drop_metadata::parse(&bytes).ok()? {
                        drop_metadata::Metadata::Files(paths) => {
                            Some((FileDropEvent::Entered { position, paths }, false))
                        }
                        drop_metadata::Metadata::Url(url) => {
                            Some((FileDropEvent::UrlEntered { position, url }, false))
                        }
                    }
                }
            }
        });
        let source = state.xdnd_state.source;
        let Some((entered, needs_image)) = result else {
            reply_position_owner(&mut state, status_owner, false);
            if state.xdnd_state.dropped {
                send_finished(&state, target, source, false);
            }
            state.xdnd_state.retire();
            drop(state);
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
            return;
        };
        state.xdnd_state.reading = false;
        state.xdnd_state.cancel.take();
        state.xdnd_state.ready = true;
        let dropped = state.xdnd_state.dropped;
        let position = state.xdnd_state.position;
        let submit = dropped && final_read && !needs_image;
        if submit {
            send_finished(&state, target, source, true);
            state.xdnd_state.retire();
        } else {
            reply_position_owner(&mut state, status_owner, true);
        }
        let terminal_generation = state.xdnd_state.generation;
        drop(state);
        window.handle_input(PlatformInput::FileDrop(entered));
        let terminal_current = self.0.borrow().xdnd_state.generation == terminal_generation;
        if submit && terminal_current {
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Submit { position }));
        }
        // Admission travels with bytes through their final foreground delivery.
        drop(lease);
        if dropped && (!final_read || needs_image) {
            let image = self.0.borrow().xdnd_state.image;
            self.start_drop_read(generation, image);
        }
    }
}

fn reply_pending_position(state: &mut X11ClientState, accepted: bool) {
    let owner = state.xdnd_state.pending_position;
    reply_position_owner(state, owner, accepted);
}
fn reply_position_owner(state: &mut X11ClientState, expected: Option<Position>, accepted: bool) {
    let Some(expected) = expected else {
        return;
    };
    if state
        .xdnd_state
        .acknowledge_position(expected, accepted)
        .is_none()
    {
        return;
    }
    send_status(
        state,
        state.xdnd_state.target,
        state.xdnd_state.source,
        accepted,
    );
}

fn send_status(state: &X11ClientState, target: u32, source: u32, accepted: bool) {
    let event = ClientMessageEvent::new(
        32,
        source,
        state.atoms.XdndStatus,
        [
            target,
            if accepted { 3 } else { 2 },
            0,
            0,
            if accepted {
                state.atoms.XdndActionCopy
            } else {
                0
            },
        ],
    );
    let _ = state
        .xcb_connection
        .send_event(false, source, EventMask::NO_EVENT, event);
    let _ = state.xcb_connection.flush();
}
fn send_finished(state: &X11ClientState, target: u32, source: u32, accepted: bool) {
    let event = ClientMessageEvent::new(
        32,
        source,
        state.atoms.XdndFinished,
        [
            target,
            u32::from(accepted),
            if accepted {
                state.atoms.XdndActionCopy
            } else {
                0
            },
            0,
            0,
        ],
    );
    let _ = state
        .xcb_connection
        .send_event(false, source, EventMask::NO_EVENT, event);
    let _ = state.xcb_connection.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_completion_cannot_reply_again_or_change_a_final_refusal() {
        let mut drag = Xdnd::default();
        let hover = Position {
            sequence: 1,
            root: [10, 20],
            timestamp: 30,
        };
        let final_point = Position {
            sequence: 2,
            root: [40, 50],
            timestamp: 60,
        };
        drag.pending_position = Some(hover);
        assert_eq!(drag.acknowledge_position(hover, false), Some(hover));
        drag.pending_position = Some(final_point);
        assert_eq!(drag.acknowledge_position(hover, true), None);
        assert_eq!(drag.pending_position, Some(final_point));
        assert_eq!(
            drag.acknowledge_position(final_point, false),
            Some(final_point)
        );
        assert_eq!(drag.acknowledge_position(hover, true), None);
        assert_eq!(drag.accepted_position, None);
        let same_wire_point = Position {
            sequence: 3,
            ..final_point
        };
        drag.pending_position = Some(same_wire_point);
        assert_eq!(drag.acknowledge_position(final_point, true), None);
        assert_eq!(
            drag.acknowledge_position(same_wire_point, true),
            Some(same_wire_point)
        );
        assert_eq!(drag.acknowledge_position(hover, false), None);
        assert_eq!(drag.accepted_position, Some(same_wire_point));
        drag.retire();
        assert_eq!(drag.pending_position, None);
        assert_eq!(drag.accepted_position, None);
    }
    #[test]
    fn cancel_and_new_native_generation_keep_completed_body_admission() {
        let mut drag = Xdnd::default();
        drag.source = 7;
        drag.target = 11;
        assert!(drag.matches(7, 11));
        let generation = drag.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        drag.cancel = Some(cancel.clone());
        drag.active.store(true, Ordering::Release);
        let lease = Lease(drag.active.clone());
        drag.cancel_target(12);
        assert!(drag.current(generation, 11));
        drag.cancel_target(11);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!drag.current(generation, 11));
        for _ in 0..256 {
            drag.retire();
        }
        assert!(drag.active.load(Ordering::Acquire));
        drop(lease);
        assert!(!drag.active.load(Ordering::Acquire));
    }
}
