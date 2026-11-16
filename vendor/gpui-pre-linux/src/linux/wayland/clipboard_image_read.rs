//! One bounded offer transfer. Native admission is foreground; only this owned input is Send.
use gpui::{ClipboardEntry, ClipboardItem, ClipboardReadError, Image, ImageFormat, hash};
use std::{
    io::Write,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(crate) const IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) struct Lease(pub Arc<AtomicBool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
pub(crate) struct Pending {
    pub id: u64,
    pub cancel: Option<UnixStream>,
}
impl Pending {
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.as_mut() {
            let _ = cancel.write(&[1]);
        }
    }
}
pub(crate) enum Input {
    Owned(Arc<ClipboardItem>),
    Pipe {
        fd: filedescriptor::FileDescriptor,
        cancel: UnixStream,
        format: ImageFormat,
    },
}
pub(crate) struct Read {
    pub id: u64,
    pub input: Input,
    pub lease: Lease,
}
impl Read {
    pub fn execute(self) -> (Result<Image, ClipboardReadError>, Lease) {
        (self.input.execute(), self.lease)
    }
}
impl Input {
    fn execute(self) -> Result<Image, ClipboardReadError> {
        match self {
            Input::Owned(item) => {
                let image = item
                    .entries
                    .iter()
                    .find_map(|entry| {
                        if let ClipboardEntry::Image(image) = entry {
                            Some(image)
                        } else {
                            None
                        }
                    })
                    .ok_or(ClipboardReadError::UnsupportedContent)?;
                if image.bytes.len() > IMAGE_BYTES {
                    return Err(ClipboardReadError::TooLarge);
                }
                Ok(image.clone())
            }
            Input::Pipe { fd, cancel, format } => {
                let bytes = crate::linux::platform::read_fd_bounded(
                    fd,
                    Duration::from_secs(4),
                    IMAGE_BYTES,
                    Some(cancel),
                )
                .map_err(|error| {
                    error
                        .downcast::<ClipboardReadError>()
                        .unwrap_or(ClipboardReadError::Unavailable)
                })?;
                if bytes.is_empty() {
                    return Err(ClipboardReadError::UnsupportedContent);
                }
                let id = hash(&bytes);
                Ok(Image { format, bytes, id })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_covers_refused_completion_until_foreground_discard() {
        let active = Arc::new(AtomicBool::new(true));
        let read = Read {
            id: 1,
            input: Input::Owned(Arc::new(ClipboardItem::new_string("private text".into()))),
            lease: Lease(active.clone()),
        };
        let (result, lease) = read.execute();
        assert!(matches!(
            result,
            Err(ClipboardReadError::UnsupportedContent)
        ));
        assert!(active.load(Ordering::Acquire));
        drop(lease);
        assert!(!active.load(Ordering::Acquire));
    }
    #[test]
    fn owned_image_transfer_keeps_only_the_existing_owner() {
        let image = Image {
            format: ImageFormat::Png,
            bytes: vec![1, 2, 3],
            id: 7,
        };
        let item = Arc::new(ClipboardItem::new_image(&image));
        let active = Arc::new(AtomicBool::new(true));
        let read = Read {
            id: 1,
            input: Input::Owned(item.clone()),
            lease: Lease(active.clone()),
        };
        let (result, lease) = read.execute();
        let result = result.unwrap();
        assert_eq!(result, image);
        assert_eq!(Arc::strong_count(&item), 1);
        assert!(active.load(Ordering::Acquire));
        drop(result);
        assert!(active.load(Ordering::Acquire));
        drop(lease);
        assert!(!active.load(Ordering::Acquire));
    }
}
