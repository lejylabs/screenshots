use std::{
    fs::File,
    io::{ErrorKind, Write},
    os::fd::{AsRawFd, BorrowedFd, OwnedFd},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use calloop::{LoopHandle, PostAction};
use filedescriptor::Pipe;
use strum::IntoEnumIterator;
use wayland_client::{Connection, Proxy, protocol::wl_data_offer::WlDataOffer};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;

use crate::linux::platform::{PIPE_READ_TIMEOUT, read_fd_bounded};
use gpui::{ClipboardEntry, ClipboardItem, ClipboardReadError, Image, ImageFormat, hash};
#[path = "clipboard_image_read.rs"]
mod image_read;

/// Text mime types that we'll offer to other programs.
pub(crate) const TEXT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];
pub(crate) const FILE_LIST_MIME_TYPE: &str = "text/uri-list";

/// Text mime types that we'll accept from other programs.
pub(crate) const ALLOWED_TEXT_MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "UTF8_STRING"];

pub(crate) struct Clipboard {
    connection: Connection,
    loop_handle: LoopHandle<'static, ()>,
    self_mime: String,

    // Internal clipboard
    contents: Option<Arc<ClipboardItem>>,
    primary_contents: Option<Arc<ClipboardItem>>,

    // External clipboard
    cached_read: Option<ClipboardItem>,
    current_offer: Option<DataOffer<WlDataOffer>>,
    cached_primary_read: Option<ClipboardItem>,
    current_primary_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
    image_pending: Option<image_read::Pending>,
    image_active: Arc<AtomicBool>,
    image_id: u64,
}

pub(crate) trait ReceiveData {
    fn alive(&self) -> bool {
        true
    }
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>);
}

impl ReceiveData for WlDataOffer {
    fn alive(&self) -> bool {
        self.is_alive()
    }
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl ReceiveData for ZwpPrimarySelectionOfferV1 {
    fn alive(&self) -> bool {
        self.is_alive()
    }
    fn receive_data(&self, mime_type: String, fd: BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

#[derive(Clone, Debug)]
/// Wrapper for `WlDataOffer` and `ZwpPrimarySelectionOfferV1`, used to help track mime types.
pub(crate) struct DataOffer<T: ReceiveData> {
    pub inner: T,
    mime_types: Vec<String>,
    overflow: bool,
}

impl<T: ReceiveData> DataOffer<T> {
    pub fn new(offer: T) -> Self {
        Self {
            inner: offer,
            mime_types: Vec::new(),
            overflow: false,
        }
    }

    pub fn add_mime_type(&mut self, mime_type: String) {
        if self.mime_types.len() >= 256 || mime_type.is_empty() || mime_type.len() > 128 {
            self.overflow = true;
            return;
        }
        if !self.mime_types.contains(&mime_type) {
            self.mime_types.push(mime_type);
        }
    }

    pub(crate) fn dnd_has_mime(&self, mime: &str) -> bool {
        !self.overflow && self.has_mime_type(mime)
    }
    pub(crate) fn dnd_image_format(&self) -> Option<ImageFormat> {
        if self.overflow {
            return None;
        }
        [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Webp,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
        ]
        .into_iter()
        .find(|format| self.has_mime_type(format.mime_type()))
    }
    fn has_mime_type(&self, mime_type: &str) -> bool {
        self.mime_types.iter().any(|t| t == mime_type)
    }

    fn read_bytes(&self, connection: &Connection, mime_type: &str) -> Option<Vec<u8>> {
        if self.overflow || !self.inner.alive() {
            return None;
        }
        let pipe = Pipe::new().ok()?;
        self.inner.receive_data(mime_type.to_string(), unsafe {
            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
        });
        let fd = pipe.read;
        drop(pipe.write);

        connection.flush().ok()?;
        let maximum = if mime_type.starts_with("image/") {
            image_read::IMAGE_BYTES
        } else {
            64 * 1024
        };
        match read_fd_bounded(fd, PIPE_READ_TIMEOUT, maximum, None) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                log::error!("error reading clipboard pipe: {err:?}");
                None
            }
        }
    }

    fn read_text(&self, connection: &Connection) -> Option<ClipboardItem> {
        let mime_type = self.mime_types.iter().find(|&mime_type| {
            ALLOWED_TEXT_MIME_TYPES
                .iter()
                .any(|&allowed| allowed == mime_type)
        })?;
        let bytes = self.read_bytes(connection, mime_type)?;
        let text_content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(e) => {
                log::error!("Failed to convert clipboard content to UTF-8: {}", e);
                return None;
            }
        };

        // Normalize the text to unix line endings, otherwise
        // copying from eg: firefox inserts a lot of blank
        // lines, and that is super annoying.
        let result = text_content.replace("\r\n", "\n");
        Some(ClipboardItem::new_string(result))
    }

    fn read_image(&self, connection: &Connection) -> Option<ClipboardItem> {
        for format in ImageFormat::iter() {
            let mime_type = format.mime_type();
            if !self.has_mime_type(mime_type) {
                continue;
            }

            if let Some(bytes) = self.read_bytes(connection, mime_type) {
                let id = hash(&bytes);
                return Some(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(Image { format, bytes, id })],
                });
            }
        }
        None
    }
}

impl Clipboard {
    pub fn new(connection: Connection, loop_handle: LoopHandle<'static, ()>) -> Self {
        Self {
            connection,
            loop_handle,
            self_mime: format!("pid/{}", std::process::id()),

            contents: None,
            primary_contents: None,

            cached_read: None,
            current_offer: None,
            cached_primary_read: None,
            current_primary_offer: None,
            image_pending: None,
            image_active: Arc::new(AtomicBool::new(false)),
            image_id: 0,
        }
    }

    pub fn set(&mut self, item: ClipboardItem) {
        self.contents = Some(Arc::new(item));
    }

    pub fn set_primary(&mut self, item: ClipboardItem) {
        self.primary_contents = Some(Arc::new(item));
    }

    pub fn set_offer(&mut self, data_offer: Option<DataOffer<WlDataOffer>>) {
        self.cancel_image_read();
        self.cached_read = None;
        if let Some(retired) = std::mem::replace(&mut self.current_offer, data_offer) {
            if self
                .current_offer
                .as_ref()
                .is_none_or(|current| current.inner.id() != retired.inner.id())
                && retired.inner.is_alive()
            {
                retired.inner.destroy();
            }
        }
    }

    pub fn set_primary_offer(&mut self, data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>) {
        self.cached_primary_read = None;
        self.current_primary_offer = data_offer;
    }

    pub fn self_mime(&self) -> String {
        self.self_mime.clone()
    }

    pub fn send(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self.contents.as_ref().and_then(|contents| contents.text()) {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn send_primary(&self, _mime_type: String, fd: OwnedFd) {
        if let Some(text) = self
            .primary_contents
            .as_ref()
            .and_then(|contents| contents.text())
        {
            self.send_bytes(fd, text.as_bytes().to_owned());
        }
    }

    pub fn read(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_offer.as_ref()?;
        if let Some(cached) = self.cached_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.contents.as_ref().map(|item| (**item).clone());
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        if item
            .entries
            .iter()
            .all(|entry| matches!(entry, ClipboardEntry::String(_)))
        {
            self.cached_read = Some(item.clone());
        }
        Some(item)
    }

    pub fn read_primary(&mut self) -> Option<ClipboardItem> {
        let offer = self.current_primary_offer.as_ref()?;
        if let Some(cached) = self.cached_primary_read.clone() {
            return Some(cached);
        }

        if offer.has_mime_type(&self.self_mime) {
            return self.primary_contents.as_ref().map(|item| (**item).clone());
        }

        let item = offer
            .read_text(&self.connection)
            .or_else(|| offer.read_image(&self.connection))?;

        if item
            .entries
            .iter()
            .all(|entry| matches!(entry, ClipboardEntry::String(_)))
        {
            self.cached_primary_read = Some(item.clone());
        }
        Some(item)
    }

    pub(super) fn cancel_image_read(&mut self) {
        if let Some(mut pending) = self.image_pending.take() {
            pending.cancel();
        }
    }
    pub(super) fn is_image_read_current(&self, id: u64) -> bool {
        self.current_offer
            .as_ref()
            .is_some_and(|offer| offer.inner.alive())
            && self
                .image_pending
                .as_ref()
                .is_some_and(|pending| pending.id == id)
    }
    pub(super) fn finish_image_read(&mut self, id: u64) {
        if self
            .image_pending
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            self.image_pending.take();
        }
    }
    pub(super) fn begin_image_read(&mut self) -> Result<image_read::Read, ClipboardReadError> {
        if self
            .image_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ClipboardReadError::Busy);
        }
        let lease = image_read::Lease(self.image_active.clone());
        let offer = self
            .current_offer
            .as_ref()
            .ok_or(ClipboardReadError::Unavailable)?;
        if offer.overflow {
            return Err(ClipboardReadError::TooLarge);
        }
        if !offer.inner.alive() {
            return Err(ClipboardReadError::Unavailable);
        }
        self.image_id = self
            .image_id
            .checked_add(1)
            .ok_or(ClipboardReadError::Unavailable)?;
        let id = self.image_id;
        if offer.has_mime_type(&self.self_mime) {
            let item = self
                .contents
                .clone()
                .ok_or(ClipboardReadError::UnsupportedContent)?;
            self.image_pending = Some(image_read::Pending { id, cancel: None });
            return Ok(image_read::Read {
                id,
                input: image_read::Input::Owned(item),
                lease,
            });
        }
        let format = [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Webp,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
        ]
        .into_iter()
        .find(|format| offer.has_mime_type(format.mime_type()))
        .ok_or(ClipboardReadError::UnsupportedContent)?;
        let pipe = Pipe::new().map_err(|_| ClipboardReadError::Unavailable)?;
        let (cancel_read, cancel_write) =
            UnixStream::pair().map_err(|_| ClipboardReadError::Unavailable)?;
        cancel_write
            .set_nonblocking(true)
            .map_err(|_| ClipboardReadError::Unavailable)?;
        offer
            .inner
            .receive_data(format.mime_type().to_string(), unsafe {
                BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
            });
        drop(pipe.write);
        // Connection flushing is nonblocking; pending output remains with the native event loop.
        if let Err(error) = self.connection.flush() {
            if !matches!(error,wayland_client::backend::WaylandError::Io(error) if error.kind()==ErrorKind::WouldBlock)
            {
                return Err(ClipboardReadError::Unavailable);
            }
        }
        self.image_pending = Some(image_read::Pending {
            id,
            cancel: Some(cancel_write),
        });
        Ok(image_read::Read {
            id,
            input: image_read::Input::Pipe {
                fd: pipe.read,
                cancel: cancel_read,
                format,
            },
            lease,
        })
    }

    pub fn send_bytes(&self, fd: OwnedFd, bytes: Vec<u8>) {
        let mut written = 0;
        self.loop_handle
            .insert_source(
                calloop::generic::Generic::new(
                    File::from(fd),
                    calloop::Interest::WRITE,
                    calloop::Mode::Level,
                ),
                move |_, file, _| {
                    let file = unsafe { file.get_mut() };
                    loop {
                        match file.write(&bytes[written..]) {
                            Ok(n) if written + n == bytes.len() => {
                                written += n;
                                break Ok(PostAction::Remove);
                            }
                            Ok(n) => written += n,
                            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                                break Ok(PostAction::Continue);
                            }
                            Err(_) => break Ok(PostAction::Remove),
                        }
                    }
                },
            )
            .unwrap();
    }
}

#[cfg(test)]
mod bounded_offer_tests {
    use super::*;
    #[derive(Clone, Debug)]
    struct NoRead;
    impl ReceiveData for NoRead {
        fn receive_data(&self, _: String, _: BorrowedFd<'_>) {
            panic!("Metadata-only admission requested clipboard bytes");
        }
    }
    #[test]
    fn offers_bound_count_and_label_before_retention() {
        let mut offer = DataOffer::new(NoRead);
        for index in 0..256 {
            offer.add_mime_type(format!("application/private-{index}"));
        }
        assert_eq!(offer.mime_types.len(), 256);
        assert!(!offer.overflow);
        offer.add_mime_type("one-too-many".into());
        assert!(offer.overflow);
        assert_eq!(offer.mime_types.len(), 256);
        let mut offer = DataOffer::new(NoRead);
        offer.add_mime_type("x".repeat(129));
        assert!(offer.overflow && offer.mime_types.is_empty());
    }
}
