//! One stack-owned work budget, shared across short rows and long pixel spans.
use crate::CaptureError;
use std::io::Write;

pub(crate) fn latch(cancelled: impl Fn() -> bool) -> impl Fn() -> bool {
    let observed = std::cell::Cell::new(false);
    move || {
        if !observed.get() {
            observed.set(cancelled());
        }
        observed.get()
    }
}

pub(crate) struct ImageWork<'a, C: Fn() -> bool> {
    cancelled: &'a C,
    remaining: usize,
}

pub(crate) struct CheckedImageWriter<'a, W: Write, C: Fn() -> bool> {
    inner: Option<std::io::BufWriter<W>>,
    work: ImageWork<'a, C>,
}
impl<'a, W: Write, C: Fn() -> bool> CheckedImageWriter<'a, W, C> {
    pub(crate) fn new(inner: W, cancelled: &'a C) -> Result<Self, CaptureError> {
        Ok(Self {
            inner: Some(std::io::BufWriter::with_capacity(64 * 1024, inner)),
            work: ImageWork::new(cancelled)?,
        })
    }
    fn check(&self) -> std::io::Result<()> {
        self.work.check().map_err(std::io::Error::other)
    }
}
impl<W: Write, C: Fn() -> bool> Write for CheckedImageWriter<'_, W, C> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.work.remaining == 0 {
            self.check()?;
            self.work.remaining = 64 * 1024;
        }
        let count = self
            .inner
            .as_mut()
            .expect("live image writer")
            .write(&bytes[..bytes.len().min(self.work.remaining)])?;
        self.work.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.inner.as_mut().expect("live image writer").flush()
    }
}

impl<W: Write, C: Fn() -> bool> Drop for CheckedImageWriter<'_, W, C> {
    fn drop(&mut self) {
        if let Some(writer) = self.inner.take() {
            // Every successful encoding explicitly flushes. On error, discard
            // buffered bytes without another potentially blocking file write.
            let _ = writer.into_parts();
        }
    }
}

impl<'a, C: Fn() -> bool> ImageWork<'a, C> {
    pub(crate) fn new(cancelled: &'a C) -> Result<Self, CaptureError> {
        let value = Self {
            cancelled,
            remaining: 0,
        };
        value.check()?;
        Ok(value)
    }

    pub(crate) fn check(&self) -> Result<(), CaptureError> {
        if (self.cancelled)() {
            return Err(CaptureError::Canceled);
        }
        Ok(())
    }

    pub(crate) fn pixel(&mut self) -> Result<(), CaptureError> {
        self.charge(4)
    }

    /// Charge bounded scalar traversal or one bounded kernel chunk before doing it.
    pub(crate) fn charge(&mut self, amount: usize) -> Result<(), CaptureError> {
        if amount > 64 * 1024 {
            return Err(CaptureError::Backend(
                "Image work chunk exceeds its cancellation budget".into(),
            ));
        }
        if self.remaining < amount {
            self.check()?;
            self.remaining = 64 * 1024;
        }
        self.remaining -= amount;
        Ok(())
    }

    pub(crate) fn copy(
        &mut self,
        destination: &mut [u8],
        source: &[u8],
    ) -> Result<(), CaptureError> {
        if destination.len() != source.len() {
            return Err(CaptureError::Backend("Image copy extents differ".into()));
        }
        let mut offset = 0;
        while offset < source.len() {
            if self.remaining == 0 {
                self.check()?;
                self.remaining = 64 * 1024;
            }
            let count = self.remaining.min(source.len() - offset);
            destination[offset..offset + count].copy_from_slice(&source[offset..offset + count]);
            offset += count;
            self.remaining -= count;
        }
        Ok(())
    }

    pub(crate) fn extend(
        &mut self,
        destination: &mut Vec<u8>,
        source: &[u8],
    ) -> Result<(), CaptureError> {
        let mut offset = 0;
        while offset < source.len() {
            if self.remaining == 0 {
                self.check()?;
                self.remaining = 64 * 1024;
            }
            let count = self.remaining.min(source.len() - offset);
            destination.extend_from_slice(&source[offset..offset + count]);
            offset += count;
            self.remaining -= count;
        }
        Ok(())
    }
}
