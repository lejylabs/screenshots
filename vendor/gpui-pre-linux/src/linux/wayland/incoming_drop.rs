//! Operation-owned native drop transfers. Raster bytes are requested only after Drop.
use super::*;
use anyhow::Result;
use gpui::ExternalImageDrop;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
struct Lease(Arc<AtomicBool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct NoOffer;
    impl FileDragDataOffer for NoOffer {
        fn finish(&self) {}
        fn destroy(&self) {}
    }
    #[test]
    fn completed_bytes_keep_the_single_slot_while_foreground_delivery_is_withheld() {
        use std::io::Write as _;
        let mut drag = DragState::<NoOffer, ()> {
            data_offer: None,
            window: None,
            position: Point::default(),
            uri_read_generation: 0,
            drop_pending: false,
            data_ready: false,
            copy_allowed: false,
            image_format: None,
            read_cancel: None,
            read_active: Arc::new(AtomicBool::new(false)),
        };
        let lease = lease(&drag.read_active).unwrap();
        let pipe = Pipe::new().unwrap();
        let (cancel_read, _cancel_write) = UnixStream::pair().unwrap();
        let background = std::thread::spawn(move || {
            read_with_lease(pipe.read, cancel_read, 32 * 1024 * 1024, lease)
        });
        let mut writer = pipe.write;
        writer.write_all(b"private completed raster bytes").unwrap();
        drop(writer);
        let (result, completed_lease) = background.join().unwrap();
        assert_eq!(result.as_ref().unwrap(), b"private completed raster bytes");
        for _ in 0..256 {
            drag.begin_uri_read();
            drag.cancel_from_application();
            assert!(super::lease(&drag.read_active).is_none());
        }
        drop(result);
        assert!(super::lease(&drag.read_active).is_none());
        drop(completed_lease);
        assert!(super::lease(&drag.read_active).is_some());
    }
}
fn lease(active: &Arc<AtomicBool>) -> Option<Lease> {
    active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| Lease(active.clone()))
}
fn read_with_lease(
    fd: filedescriptor::FileDescriptor,
    cancel: UnixStream,
    maximum: usize,
    lease: Lease,
) -> (Result<Vec<u8>>, Lease) {
    let result =
        crate::linux::platform::read_fd_bounded(fd, PIPE_READ_TIMEOUT, maximum, Some(cancel));
    // Completed bytes still occupy the one-reader budget until foreground delivery.
    (result, lease)
}
fn transfer(
    state: &mut DragState,
    offer: &WlDataOffer,
    mime: &str,
    connection: &Connection,
) -> Result<(filedescriptor::FileDescriptor, UnixStream)> {
    let pipe = Pipe::new()?;
    let (read, write) = UnixStream::pair()?;
    write.set_nonblocking(true)?;
    if !offer.is_alive() {
        anyhow::bail!("Native drop offer ended");
    }
    offer.receive(mime.to_owned(), unsafe {
        BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
    });
    drop(pipe.write);
    if let Err(error) = connection.flush() {
        if !matches!(error,wayland_client::backend::WaylandError::Io(ref error) if error.kind()==std::io::ErrorKind::WouldBlock)
        {
            return Err(error.into());
        }
    }
    state.read_cancel = Some(write);
    Ok((pipe.read, read))
}
impl WaylandClientStatePtr {
    pub(super) fn begin_incoming_drop(
        &self,
        state: &mut WaylandClientState,
        metadata: DataOffer<WlDataOffer>,
        window: WaylandWindowStatePtr,
        position: Point<Pixels>,
        serial: u32,
        connection: &Connection,
    ) -> Vec<(WaylandWindowStatePtr, PlatformInput)> {
        let mut events = Vec::with_capacity(2);
        state.drag.drop_pending = false;
        if let Some(old) = state.drag.handle_leave() {
            events.push(old);
        }
        let generation = state.drag.begin_uri_read();
        let format = metadata.dnd_image_format();
        let mime = if let Some(format) = format {
            format.mime_type()
        } else if metadata.dnd_has_mime(FILE_LIST_MIME_TYPE) {
            FILE_LIST_MIME_TYPE
        } else if metadata.dnd_has_mime("text/plain") {
            "text/plain"
        } else {
            metadata.inner.accept(serial, None);
            FileDragDataOffer::destroy(&metadata.inner);
            return events;
        };
        let offer = metadata.inner;
        state.drag.copy_allowed = offer.version() < 3;
        if offer.version() >= 3 {
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        }
        offer.accept(serial, Some(mime.to_owned()));
        state.drag.data_offer = Some(offer.clone());
        state.drag.window = Some(window.clone());
        state.drag.position = position;
        state.drag.data_ready = false;
        state.drag.image_format = format;
        if let Some(format) = format {
            events.push((
                window,
                PlatformInput::FileDrop(FileDropEvent::ImageEntered {
                    position,
                    image: ExternalImageDrop {
                        format,
                        bytes: None,
                    },
                }),
            ));
            return events;
        }
        let Some(lease) = lease(&state.drag.read_active) else {
            state.drag.drop_pending = false;
            state.drag.handle_leave();
            return events;
        };
        let (fd, cancel) = match transfer(&mut state.drag, &offer, mime, connection) {
            Ok(value) => value,
            Err(_) => {
                state.drag.handle_leave();
                return events;
            }
        };
        let read = state.common.borrow().background_executor.spawn(async move {
            let (result, lease) = read_with_lease(fd, cancel, 64 * 1024, lease);
            (
                result.and_then(|bytes| super::drop_metadata::parse(&bytes)),
                lease,
            )
        });
        let this = self.clone();
        state
            .common
            .borrow()
            .foreground_executor
            .clone()
            .spawn(async move {
                let (value, _lease) = read.await;
                let Some(client) = this.0.upgrade() else {
                    FileDragDataOffer::destroy(&offer);
                    return;
                };
                let mut state = client.borrow_mut();
                if !state.drag.is_uri_read_current(generation) {
                    FileDragDataOffer::destroy(&offer);
                    return;
                }
                let input = match value {
                    Ok(super::drop_metadata::Metadata::Files(paths)) => {
                        PlatformInput::FileDrop(FileDropEvent::Entered { position, paths })
                    }
                    Ok(super::drop_metadata::Metadata::Url(url)) => {
                        PlatformInput::FileDrop(FileDropEvent::UrlEntered { position, url })
                    }
                    Err(_) => {
                        state.drag.drop_pending = false;
                        let exit = state.drag.handle_leave();
                        drop(state);
                        if let Some((window, input)) = exit {
                            window.handle_input(input);
                        }
                        return;
                    }
                };
                let entered = state
                    .drag
                    .complete_data_read(generation, offer, window, position, input);
                let submitted = if state.drag.drop_pending {
                    state.drag.handle_drop()
                } else {
                    None
                };
                drop(state);
                if let Some((window, input)) = entered {
                    window.handle_input(input);
                }
                if let Some((window, input)) = submitted {
                    window.handle_input(input);
                }
            })
            .detach();
        events
    }
    pub(super) fn receive_incoming_image(
        &self,
        state: &mut WaylandClientState,
        connection: &Connection,
    ) -> Option<(WaylandWindowStatePtr, PlatformInput)> {
        if state.drag.drop_pending {
            return None;
        }
        state.drag.drop_pending = true;
        let Some(format) = state.drag.image_format else {
            return None;
        };
        let Some(offer) = state.drag.data_offer.clone() else {
            return None;
        };
        let Some(window) = state.drag.window.clone() else {
            return None;
        };
        let position = state.drag.position;
        let generation = state.drag.uri_read_generation;
        let Some(lease) = lease(&state.drag.read_active) else {
            state.drag.drop_pending = false;
            return state.drag.handle_leave();
        };
        let (fd, cancel) = match transfer(&mut state.drag, &offer, format.mime_type(), connection) {
            Ok(value) => value,
            Err(_) => {
                state.drag.drop_pending = false;
                return state.drag.handle_leave();
            }
        };
        let read = state
            .common
            .borrow()
            .background_executor
            .spawn(async move { read_with_lease(fd, cancel, 32 * 1024 * 1024, lease) });
        let this = self.clone();
        state
            .common
            .borrow()
            .foreground_executor
            .clone()
            .spawn(async move {
                let (value, _lease) = read.await;
                let Some(client) = this.0.upgrade() else {
                    FileDragDataOffer::destroy(&offer);
                    return;
                };
                let mut state = client.borrow_mut();
                if !state.drag.is_uri_read_current(generation) {
                    FileDragDataOffer::destroy(&offer);
                    return;
                }
                let bytes = match value {
                    Ok(bytes) if !bytes.is_empty() => bytes,
                    _ => {
                        state.drag.drop_pending = false;
                        let exit = state.drag.handle_leave();
                        drop(state);
                        if let Some((window, input)) = exit {
                            window.handle_input(input);
                        }
                        return;
                    }
                };
                let input = PlatformInput::FileDrop(FileDropEvent::ImageEntered {
                    position,
                    image: ExternalImageDrop {
                        format,
                        bytes: Some(Arc::new(bytes)),
                    },
                });
                let entered = state
                    .drag
                    .complete_data_read(generation, offer, window, position, input);
                let submitted = state.drag.handle_drop();
                drop(state);
                if let Some((window, input)) = entered {
                    window.handle_input(input);
                }
                if let Some((window, input)) = submitted {
                    window.handle_input(input);
                }
            })
            .detach();
        None
    }
}
