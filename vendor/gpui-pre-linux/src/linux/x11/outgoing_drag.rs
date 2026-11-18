//! One event-driven Xdnd file source. No file access, hidden renderer, thread or idle timer.
use super::*;
use calloop::timer::{TimeoutAction, Timer};
use gpui::{ExternalDragPayload, FileDragPaths};
use std::{ffi::c_void, ptr, sync::Arc};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::{COPY_FROM_PARENT, NONE, x11_utils::TryParse};

const GESTURE_TIMEOUT: Duration = Duration::from_secs(30);
const DROP_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_DEPTH: u8 = 64;
const MAX_REQUESTS: u8 = 8;

#[link(name = "xcb")]
unsafe extern "C" {
    fn xcb_poll_for_reply64(
        connection: *mut c_void,
        sequence: u64,
        reply: *mut *mut c_void,
        error: *mut *mut c_void,
    ) -> i32;
    fn xcb_discard_reply64(connection: *mut c_void, sequence: u64);
}

#[derive(Clone, Copy)]
enum Kind {
    Owner,
    PointerGrab,
    KeyboardGrab,
    Locate(u8),
    Aware(u32, u8),
    Proxy(u32, u8),
    ProxySelf(u32, u32, u8),
    ProxyAware(u32, u32, u8),
}
struct Pending {
    connection: Rc<XCBConnection>,
    sequence: u64,
    kind: Kind,
}
impl Pending {
    fn poll(&self) -> Option<Result<Vec<u8>, ()>> {
        let connection =
            as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(&*self.connection)
                .cast();
        let (mut reply, mut error) = (ptr::null_mut(), ptr::null_mut());
        // XCB's public nonblocking poll API; every outstanding reply has <=4 value bytes.
        if unsafe { xcb_poll_for_reply64(connection, self.sequence, &mut reply, &mut error) } == 0 {
            return None;
        }
        let result = if reply.is_null() || !error.is_null() {
            Err(())
        } else {
            let header = unsafe { std::slice::from_raw_parts(reply.cast::<u8>(), 32) };
            let units = u32::from_ne_bytes(header[4..8].try_into().unwrap());
            if header[0] != 1 || units > 1 {
                Err(())
            } else {
                Ok(unsafe {
                    std::slice::from_raw_parts(reply.cast::<u8>(), 32 + units as usize * 4)
                }
                .to_vec())
            }
        };
        unsafe {
            libc::free(reply);
            libc::free(error);
        }
        Some(result)
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let connection =
            as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(&*self.connection)
                .cast();
        unsafe {
            xcb_discard_reply64(connection, self.sequence);
        }
    }
}

macro_rules! query {
    ($drag:expr, $request:expr, $kind:expr) => {{
        let Ok(cookie) = $request else {
            return false;
        };
        let sequence = cookie.sequence_number();
        // Pending replaces the cookie's discard owner; its Rc keeps the connection alive.
        std::mem::forget(cookie);
        $drag.pending = Some(Pending {
            connection: $drag.connection.clone(),
            sequence,
            kind: $kind,
        });
    }};
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    point: [i16; 2],
    time: u32,
    final_position: bool,
}
#[derive(Clone, Copy)]
struct AwaitingStatus {
    target: u32,
    position: Position,
}

pub(super) struct Drag {
    connection: Rc<XCBConnection>,
    handle: LoopHandle<'static, ()>,
    timer: Option<RegistrationToken>,
    pub(super) generation: u64,
    pub(super) origin: u32,
    source: u32,
    leave_atom: u32,
    root: u32,
    bytes: Arc<[u8]>,
    start: u32,
    pointer_device: u16,
    time: u32,
    position: [i16; 2],
    released: bool,
    dropped: bool,
    grabs: bool,
    discovery: Option<Position>,
    awaiting_status: Option<AwaitingStatus>,
    target: Option<(u32, u32, u32)>, // logical target, validated proxy, protocol version
    candidate: Option<(u32, u32, u32)>,
    pending: Option<Pending>,
    requests: u8,
    deadline: Instant,
}

impl Drag {
    fn begin(
        state: &X11ClientState,
        origin: u32,
        generation: u64,
        timestamp: u32,
        pointer_device: u16,
        position: [i16; 2],
        bytes: Arc<[u8]>,
    ) -> Option<Self> {
        let connection = state.xcb_connection.clone();
        let source = connection.generate_id().ok()?;
        let root = connection.setup().roots[state.x_root_index].root;
        connection
            .create_window(
                0,
                source,
                root,
                0,
                0,
                1,
                1,
                0,
                xproto::WindowClass::INPUT_ONLY,
                COPY_FROM_PARENT,
                &xproto::CreateWindowAux::new(),
            )
            .ok()?;
        let mut drag = Self {
            connection,
            handle: state.loop_handle.clone(),
            timer: None,
            generation,
            origin,
            source,
            leave_atom: state.atoms.XdndLeave,
            root,
            bytes,
            start: timestamp,
            pointer_device,
            time: timestamp,
            position,
            released: false,
            dropped: false,
            grabs: false,
            discovery: None,
            awaiting_status: None,
            target: None,
            candidate: None,
            pending: None,
            requests: 0,
            deadline: Instant::now() + GESTURE_TIMEOUT,
        };
        drag.connection
            .change_property32(
                xproto::PropMode::REPLACE,
                source,
                state.atoms.XdndActionList,
                AtomEnum::ATOM,
                &[state.atoms.XdndActionCopy],
            )
            .ok()?;
        drag.connection
            .set_selection_owner(source, state.atoms.XdndSelection, timestamp)
            .ok()?;
        if !drag.begin_owner_check(&state.atoms) {
            return None;
        }
        Some(drag)
    }
    fn arm_timer(&mut self, weak: Weak<RefCell<X11ClientState>>) -> bool {
        if let Some(timer) = self.timer.take() {
            self.handle.remove(timer);
        }
        let generation = self.generation;
        let timer =
            self.handle
                .insert_source(Timer::from_deadline(self.deadline), move |_, _, _| {
                    if let Some(client) = weak.upgrade() {
                        X11Client(client).end_external_drag(generation);
                    }
                    TimeoutAction::Drop
                });
        let Ok(timer) = timer else {
            return false;
        };
        self.timer = Some(timer);
        true
    }
    fn begin_owner_check(&mut self, atoms: &XcbAtoms) -> bool {
        query!(
            self,
            self.connection.get_selection_owner(atoms.XdndSelection),
            Kind::Owner
        );
        self.connection.flush().is_ok()
    }
    fn desired_position(&self) -> Position {
        Position {
            point: self.position,
            time: self.time,
            final_position: self.released,
        }
    }
    fn begin_discovery(&mut self) -> bool {
        if self.awaiting_status.is_some() {
            return true;
        }
        self.discovery = Some(self.desired_position());
        self.candidate = None;
        self.locate(self.root, 0)
    }
    fn locate(&mut self, window: u32, depth: u8) -> bool {
        if depth >= MAX_DEPTH {
            return false;
        }
        let Some(position) = self.discovery else {
            return false;
        };
        // XTranslateCoordinates returns the child containing this saved point;
        // querying the live pointer would make post-release movement change the drop.
        query!(
            self,
            self.connection.translate_coordinates(
                self.root,
                window,
                position.point[0],
                position.point[1]
            ),
            Kind::Locate(depth)
        );
        self.connection.flush().is_ok()
    }
    fn property(&mut self, window: u32, atom: u32, kind: Kind) -> bool {
        query!(
            self,
            self.connection
                .get_property(false, window, atom, AtomEnum::ANY, 0, 1),
            kind
        );
        self.connection.flush().is_ok()
    }
    fn poll(&mut self, atoms: &XcbAtoms) -> bool {
        if Instant::now() >= self.deadline {
            return false;
        }
        let Some(pending) = self.pending.as_ref() else {
            return true;
        };
        let Some(result) = pending.poll() else {
            return true;
        };
        let pending = self.pending.take().unwrap();
        let Ok(bytes) = result else {
            return false;
        };
        match pending.kind {
            Kind::Owner => {
                let Ok((reply, _)) = xproto::GetSelectionOwnerReply::try_parse(&bytes) else {
                    return false;
                };
                if reply.owner != self.source {
                    return false;
                }
                if self.released {
                    return self.begin_discovery();
                }
                self.grabs = true;
                // Continue the real XI2 press grab; a core grab conflicts with it.
                query!(
                    self,
                    self.connection.xinput_xi_grab_device(
                        self.origin,
                        self.start,
                        NONE,
                        self.pointer_device,
                        xproto::GrabMode::ASYNC,
                        xproto::GrabMode::ASYNC,
                        xinput::GrabOwner::NO_OWNER,
                        &[u32::from(
                            xinput::XIEventMask::BUTTON_RELEASE | xinput::XIEventMask::MOTION
                        )]
                    ),
                    Kind::PointerGrab
                );
                self.connection.flush().is_ok()
            }
            Kind::PointerGrab => {
                let Ok((reply, _)) = xinput::XIGrabDeviceReply::try_parse(&bytes) else {
                    return false;
                };
                if reply.status != xproto::GrabStatus::SUCCESS {
                    self.grabs = false;
                    return false;
                }
                if self.released {
                    return self.begin_discovery();
                }
                self.grabs = true;
                query!(
                    self,
                    self.connection.grab_keyboard(
                        false,
                        self.origin,
                        self.start,
                        xproto::GrabMode::ASYNC,
                        xproto::GrabMode::ASYNC
                    ),
                    Kind::KeyboardGrab
                );
                self.connection.flush().is_ok()
            }
            Kind::KeyboardGrab => {
                let Ok((reply, _)) = xproto::GrabKeyboardReply::try_parse(&bytes) else {
                    return false;
                };
                if reply.status != xproto::GrabStatus::SUCCESS {
                    return false;
                }
                self.begin_discovery()
            }
            Kind::Locate(depth) => {
                let Ok((reply, _)) = xproto::TranslateCoordinatesReply::try_parse(&bytes) else {
                    return false;
                };
                if !reply.same_screen {
                    return false;
                }
                if reply.child == NONE {
                    return self.discovered(atoms);
                }
                // Source helper is unmapped and cannot become a discovery target.
                self.property(
                    reply.child,
                    atoms.XdndAware,
                    Kind::Aware(reply.child, depth + 1),
                )
            }
            Kind::Aware(window, depth) => {
                if let Some(version) =
                    word(&bytes, AtomEnum::ATOM.into()).filter(|version| *version >= 3)
                {
                    self.candidate = Some((window, window, version.min(5)));
                }
                // A validated proxy may advertise awareness on behalf of its logical window.
                self.property(window, atoms.XdndProxy, Kind::Proxy(window, depth))
            }
            Kind::Proxy(window, depth) => {
                if let Some(proxy) =
                    word(&bytes, AtomEnum::WINDOW.into()).filter(|proxy| *proxy != NONE)
                {
                    self.property(
                        proxy,
                        atoms.XdndProxy,
                        Kind::ProxySelf(window, proxy, depth),
                    )
                } else {
                    self.locate(window, depth)
                }
            }
            Kind::ProxySelf(window, proxy, depth) => {
                if word(&bytes, AtomEnum::WINDOW.into()) == Some(proxy) {
                    self.property(
                        proxy,
                        atoms.XdndAware,
                        Kind::ProxyAware(window, proxy, depth),
                    )
                } else {
                    self.locate(window, depth)
                }
            }
            Kind::ProxyAware(window, proxy, depth) => {
                if let Some(version) =
                    word(&bytes, AtomEnum::ATOM.into()).filter(|version| *version >= 3)
                {
                    self.candidate = Some((window, proxy, version.min(5)));
                }
                self.locate(window, depth)
            }
        }
    }
    fn send(&self, logical: u32, proxy: u32, kind: u32, data: [u32; 5]) -> bool {
        self.connection
            .send_event(
                false,
                proxy,
                EventMask::NO_EVENT,
                ClientMessageEvent::new(32, logical, kind, data),
            )
            .is_ok()
            && self.connection.flush().is_ok()
    }
    fn discovered(&mut self, atoms: &XcbAtoms) -> bool {
        let Some(position) = self.discovery.take() else {
            return false;
        };
        if position != self.desired_position() {
            // Coalesce input that arrived during the bounded native walk.
            return self.begin_discovery();
        }
        if self.awaiting_status.is_some() {
            return false;
        }
        if self.target != self.candidate {
            if let Some((window, proxy, _)) = self.target.take() {
                self.send(window, proxy, atoms.XdndLeave, [self.source, 0, 0, 0, 0]);
            }
            self.target = self.candidate;
            if let Some((window, proxy, version)) = self.target {
                if !self.send(
                    window,
                    proxy,
                    atoms.XdndEnter,
                    [self.source, version << 24, atoms.TextUriList, 0, 0],
                ) {
                    return false;
                }
            }
        }
        if let Some((window, proxy, _)) = self.target {
            let packed =
                (u32::from(position.point[0] as u16) << 16) | u32::from(position.point[1] as u16);
            // XdndStatus carries no position ID: never send a second Position
            // before consuming the sole reply to this one.
            self.awaiting_status = Some(AwaitingStatus {
                target: window,
                position,
            });
            self.send(
                window,
                proxy,
                atoms.XdndPosition,
                [self.source, 0, packed, position.time, atoms.XdndActionCopy],
            )
        } else {
            !self.released
        }
    }
    fn status(&mut self, atoms: &XcbAtoms, data: [u32; 5]) -> bool {
        let Some(waiting) = self.awaiting_status else {
            return true;
        };
        if data[0] != waiting.target {
            return true;
        }
        self.awaiting_status = None;
        if waiting.position.final_position {
            if waiting.position != self.desired_position() || self.dropped {
                return false;
            }
            let Some((target, proxy, _)) = self.target else {
                return false;
            };
            if target != waiting.target || data[1] & 1 == 0 || data[4] != atoms.XdndActionCopy {
                return false;
            }
            self.dropped = true;
            self.send(
                target,
                proxy,
                atoms.XdndDrop,
                [self.source, 0, waiting.position.time, 0, 0],
            )
        } else if waiting.position != self.desired_position() {
            // A late hover accept/refusal only drains the old exchange. The
            // final saved point still needs its own discovery and acknowledgment.
            self.begin_discovery()
        } else {
            true
        }
    }
    fn motion(&mut self, position: [i16; 2], time: u32) -> bool {
        if self.released || self.dropped {
            return true;
        }
        self.position = position;
        self.time = time;
        if self.pending.is_none() && self.awaiting_status.is_none() {
            self.begin_discovery()
        } else {
            true
        }
    }
    fn release(&mut self, position: [i16; 2], time: u32) -> bool {
        if self.released {
            return true;
        }
        self.released = true;
        self.position = position;
        self.time = time;
        self.deadline = self.deadline.min(Instant::now() + DROP_TIMEOUT);
        self.ungrab();
        // Keep one outstanding native query/Status owner. Its completion will
        // coalesce to this immutable final point without consulting the cursor.
        if self.pending.is_none() && self.awaiting_status.is_none() {
            self.begin_discovery()
        } else {
            true
        }
    }
    fn ungrab(&mut self) {
        if self.grabs {
            self.grabs = false;
            let _ = self
                .connection
                .xinput_xi_ungrab_device(x11rb::CURRENT_TIME, self.pointer_device);
            let _ = self.connection.ungrab_keyboard(x11rb::CURRENT_TIME);
            let _ = self.connection.flush();
        }
    }
    fn selection(&mut self, atoms: &XcbAtoms, request: &xproto::SelectionRequestEvent) -> bool {
        if self.requests == MAX_REQUESTS {
            return false;
        }
        self.requests += 1;
        let mut property = NONE;
        let valid_time = request.time != 0
            && (request.time.wrapping_sub(self.start) as i32) >= 0
            && (!self.released || (self.time.wrapping_sub(request.time) as i32) >= 0);
        if valid_time {
            let output = if request.property == NONE {
                request.target
            } else {
                request.property
            };
            let written = if request.target == atoms.TextUriList {
                self.connection
                    .change_property8(
                        xproto::PropMode::REPLACE,
                        request.requestor,
                        output,
                        atoms.TextUriList,
                        &self.bytes,
                    )
                    .is_ok()
            } else if request.target == atoms.TARGETS {
                self.connection
                    .change_property32(
                        xproto::PropMode::REPLACE,
                        request.requestor,
                        output,
                        AtomEnum::ATOM,
                        &[atoms.TARGETS, atoms.TIMESTAMP, atoms.TextUriList],
                    )
                    .is_ok()
            } else if request.target == atoms.TIMESTAMP {
                self.connection
                    .change_property32(
                        xproto::PropMode::REPLACE,
                        request.requestor,
                        output,
                        AtomEnum::INTEGER,
                        &[self.start],
                    )
                    .is_ok()
            } else {
                false
            };
            if written {
                property = output;
            }
        }
        let event = xproto::SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property,
        };
        self.connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, event)
            .is_ok()
            && self.connection.flush().is_ok()
    }
}
impl Drop for Drag {
    fn drop(&mut self) {
        self.pending.take();
        if !self.dropped {
            if let Some((target, proxy, _)) = self.target {
                self.send(target, proxy, self.leave_atom, [self.source, 0, 0, 0, 0]);
            }
        }
        if let Some(timer) = self.timer.take() {
            self.handle.remove(timer);
        }
        self.ungrab();
        // Destroying our helper releases only our ownership; never clear another client's selection.
        let _ = self.connection.destroy_window(self.source);
        let _ = self.connection.flush();
    }
}

fn word(bytes: &[u8], expected: u32) -> Option<u32> {
    let (reply, _) = xproto::GetPropertyReply::try_parse(bytes).ok()?;
    (reply.format == 32
        && reply.type_ == expected
        && reply.bytes_after == 0
        && reply.value_len == 1)
        .then(|| reply.value32().and_then(|mut values| values.next()))
        .flatten()
}

impl X11Client {
    pub(in crate::linux::x11) fn start_external_drag(
        &self,
        origin: u32,
        payload: &ExternalDragPayload,
    ) -> bool {
        let ExternalDragPayload::Files(paths) = payload;
        let Some(bytes) = file_uri_list(paths) else {
            return false;
        };
        let mut state = self.0.borrow_mut();
        let Some((press_origin, timestamp, time, position, pointer_device)) =
            state.external_drag_press
        else {
            return false;
        };
        if state.external_drag.is_some()
            || press_origin != origin
            || timestamp == 0
            || !state.windows.contains_key(&origin)
        {
            return false;
        }
        let Some(generation) = state.external_drag_generation.checked_add(1) else {
            return false;
        };
        state.external_drag_generation = generation;
        let Some(mut drag) = Drag::begin(
            &state,
            origin,
            generation,
            timestamp,
            pointer_device,
            position,
            bytes,
        ) else {
            return false;
        };
        drag.time = time;
        if !drag.arm_timer(Rc::downgrade(&self.0)) {
            return false;
        }
        state.external_drag = Some(drag);
        true
    }
    pub(super) fn poll_external_drag(&self) {
        let mut state = self.0.borrow_mut();
        let Some(mut drag) = state.external_drag.take() else {
            return;
        };
        if drag.poll(&state.atoms) {
            state.external_drag = Some(drag);
        } else {
            let origin = drag.origin;
            drop(drag);
            drop(state);
            self.external_drag_ended(origin);
        }
    }
    pub(super) fn end_external_drag(&self, generation: u64) {
        let mut state = self.0.borrow_mut();
        if !state
            .external_drag
            .as_ref()
            .is_some_and(|drag| drag.generation == generation)
        {
            return;
        }
        let drag = state.external_drag.take().unwrap();
        let origin = drag.origin;
        drop(drag);
        drop(state);
        self.external_drag_ended(origin);
    }
    fn external_drag_ended(&self, origin: u32) {
        if let Some(window) = self.get_window(origin) {
            window.handle_input(PlatformInput::FileDrop(FileDropEvent::Ended));
        }
    }
    pub(super) fn handle_external_drag_event(&self, event: &Event) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(mut drag) = state.external_drag.take() else {
            return false;
        };
        let mut keep = true;
        let handled = match event {
            Event::XinputMotion(event)
                if event.event == drag.origin && event.deviceid == drag.pointer_device =>
            {
                keep = drag.motion(
                    [(event.root_x >> 16) as i16, (event.root_y >> 16) as i16],
                    event.time,
                );
                true
            }
            Event::XinputButtonRelease(event)
                if event.event == drag.origin
                    && event.detail == 1
                    && event.deviceid == drag.pointer_device =>
            {
                keep = drag.release(
                    [(event.root_x >> 16) as i16, (event.root_y >> 16) as i16],
                    event.time,
                ) && drag.arm_timer(Rc::downgrade(&self.0));
                state.external_drag_press = None;
                true
            }

            Event::MotionNotify(event) if event.event == drag.origin => {
                keep = drag.motion([event.root_x, event.root_y], event.time);
                true
            }
            Event::ButtonRelease(event) if event.event == drag.origin && event.detail == 1 => {
                keep = drag.release([event.root_x, event.root_y], event.time)
                    && drag.arm_timer(Rc::downgrade(&self.0));
                state.external_drag_press = None;
                true
            }
            Event::SelectionRequest(event)
                if event.owner == drag.source && event.selection == state.atoms.XdndSelection =>
            {
                keep = drag.selection(&state.atoms, event);
                true
            }
            Event::SelectionClear(event)
                if event.owner == drag.source && event.selection == state.atoms.XdndSelection =>
            {
                keep = false;
                true
            }
            Event::ClientMessage(event) if event.window == drag.source && event.format == 32 => {
                let data = event.data.as_data32();
                if let Some((target, _, _)) = drag.target {
                    if event.type_ == state.atoms.XdndStatus {
                        keep = drag.status(&state.atoms, data);
                    } else if data[0] == target
                        && event.type_ == state.atoms.XdndFinished
                        && drag.dropped
                    {
                        keep = false;
                    }
                }
                true
            }
            Event::KeyPress(event) if event.event == drag.origin => {
                if keystroke_from_xkb(&state.xkb, state.modifiers, event.detail.into()).key
                    == "escape"
                {
                    keep = false;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        let origin = drag.origin;
        if keep && Instant::now() < drag.deadline {
            state.external_drag = Some(drag);
            drop(state);
        } else {
            drop(drag);
            drop(state);
            self.external_drag_ended(origin);
        }
        handled
    }
}

fn file_uri_list(paths: &FileDragPaths) -> Option<Arc<[u8]>> {
    use std::os::unix::ffi::OsStrExt;
    if paths.entries().is_empty() || paths.entries().len() > 32 {
        return None;
    }
    let mut list = Vec::new();
    let mut total = 0usize;
    for (path, _) in paths.entries() {
        let bytes = path.as_os_str().as_bytes();
        total = total.checked_add(bytes.len())?;
        if !path.is_absolute() || bytes.len() > 4096 || bytes.contains(&0) || total > 64 * 1024 {
            return None;
        }
        let url = http_client::Url::from_file_path(path).ok()?;
        if url.as_str().len() > 12 * 1024
            || list.len().checked_add(url.as_str().len() + 2)? > 64 * 1024
        {
            return None;
        }
        list.reserve_exact(url.as_str().len() + 2);
        list.extend_from_slice(url.as_str().as_bytes());
        list.extend_from_slice(b"\r\n");
    }
    Some(list.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_words_refuse_wrong_type_truncation_and_partial_properties() {
        let mut bytes = vec![0; 36];
        bytes[0] = 1;
        bytes[1] = 32;
        bytes[4..8].copy_from_slice(&1u32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&u32::from(AtomEnum::ATOM).to_ne_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_ne_bytes());
        bytes[32..36].copy_from_slice(&5u32.to_ne_bytes());
        assert_eq!(word(&bytes, AtomEnum::ATOM.into()), Some(5));
        assert_eq!(word(&bytes, AtomEnum::WINDOW.into()), None);
        assert_eq!(word(&bytes[..32], AtomEnum::ATOM.into()), None);
        bytes[12..16].copy_from_slice(&4u32.to_ne_bytes());
        assert_eq!(word(&bytes, AtomEnum::ATOM.into()), None);
    }
    #[test]
    fn native_outgoing_file_uri_offer_is_bounded_and_all_or_nothing() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        assert_eq!(
            &*file_uri_list(&FileDragPaths::new([(
                PathBuf::from(OsString::from_vec(b"/private/a \xff.png".to_vec())),
                false
            ),]))
            .unwrap(),
            b"file:///private/a%20%FF.png\r\n"
        );
        for paths in [
            FileDragPaths::default(),
            FileDragPaths::new([(PathBuf::from("relative"), false)]),
            FileDragPaths::new([
                (PathBuf::from("/private/valid"), false),
                (PathBuf::from("relative"), false),
            ]),
            FileDragPaths::new((0..33).map(|_| (PathBuf::from("/private/path"), false))),
            FileDragPaths::new([(PathBuf::from("/private/nul\0"), false)]),
        ] {
            assert!(file_uri_list(&paths).is_none());
        }
    }
}
