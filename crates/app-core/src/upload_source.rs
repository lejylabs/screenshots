//! Operation-owned upload source proof and bounded body consumption.
use super::UploadCancellation;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

#[cfg(test)]
thread_local! { static PAUSE_NEXT_BODY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(super) struct BodyPauseTestGuard;
#[cfg(test)]
impl Drop for BodyPauseTestGuard {
    fn drop(&mut self) {
        PAUSE_NEXT_BODY.with(|pause| pause.set(false));
    }
}
#[cfg(test)]
pub(super) fn pause_next_body_for_test() -> BodyPauseTestGuard {
    PAUSE_NEXT_BODY.with(|pause| pause.set(true));
    BodyPauseTestGuard
}

pub(super) struct UploadSourceGuard {
    // A duplicate pins the same native object. Never read or seek this handle;
    // duplicated handles can share the stream cursor.
    file: File,
    identity: crate::FileIdentity,
    length: u64,
}
impl UploadSourceGuard {
    pub(super) fn new(file: &File, identity: crate::FileIdentity, length: u64) -> Result<Self> {
        identity.verify_file(file)?;
        ensure!(
            file.metadata()?.len() == length,
            "Upload source length changed before admission"
        );
        Ok(Self {
            file: file.try_clone()?,
            identity,
            length,
        })
    }
    fn verify(self) -> Result<()> {
        self.identity.verify_file(&self.file).context(
            "Upload source changed during transfer; the server may already have received bytes",
        )?;
        ensure!(
            self.file.metadata()?.len() == self.length,
            "Upload source length changed; the server may already have received bytes"
        );
        Ok(())
    }
}

pub(super) struct TrackedUploadReader<R> {
    reader: tokio::io::Take<R>,
    consumed: Arc<AtomicU64>,
    expected: u64,
    cancel: UploadCancellation,
    #[cfg(test)]
    paused: bool,
}
impl<R: AsyncRead + Unpin> TrackedUploadReader<R> {
    pub(super) fn new(
        reader: R,
        expected: u64,
        consumed: Arc<AtomicU64>,
        cancel: UploadCancellation,
    ) -> Self {
        Self {
            reader: reader.take(expected),
            consumed,
            expected,
            cancel,
            #[cfg(test)]
            paused: PAUSE_NEXT_BODY.with(|pause| pause.take()),
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for TrackedUploadReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let this = self.get_mut();
        if let Err(error) = this.cancel.check() {
            return Poll::Ready(Err(std::io::Error::other(error.to_string())));
        }
        #[cfg(test)]
        if this.paused {
            return Poll::Pending;
        }
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buffer.filled().len();
        let result = {
            // Tokio's Take preserves ReadBuf initialization and enforces both quotas.
            let mut chunk = (&mut this.reader).take(64 * 1024);
            std::pin::Pin::new(&mut chunk).poll_read(cx, buffer)
        };
        if !matches!(result, Poll::Ready(Ok(()))) {
            return result;
        }
        if let Err(error) = this.cancel.check() {
            return Poll::Ready(Err(std::io::Error::other(error.to_string())));
        }
        let count = (buffer.filled().len() - before) as u64;
        if count == 0 && this.reader.limit() != 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Upload source ended before its admitted length; the server may already have received bytes",
            )));
        }
        let before = this.consumed.fetch_add(count, Ordering::Release);
        if before
            .checked_add(count)
            .is_none_or(|total| total > this.expected)
        {
            return Poll::Ready(Err(std::io::Error::other(
                "Upload body exceeded its admitted length",
            )));
        }
        Poll::Ready(Ok(()))
    }
}

/// Call before every successful outcome, including a resource-URL early return.
pub(super) async fn verify_transfer(
    guard: Option<UploadSourceGuard>,
    consumed: Arc<AtomicU64>,
    expected: u64,
    cancel: &UploadCancellation,
) -> Result<()> {
    cancel.check()?;
    ensure!(
        consumed.load(Ordering::Acquire) == expected,
        "Upload did not consume the complete admitted body; the server may already have received bytes"
    );
    if let Some(guard) = guard {
        let cancellation = cancel.clone();
        tokio::task::spawn_blocking(move || {
            cancellation.check()?;
            guard.verify()?;
            cancellation.check()
        })
        .await
        .context("Upload source verification worker failed")??;
    }
    cancel.check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracked_body_enforces_exact_length_short_reads_and_cancel() -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()?;
        runtime.block_on(async {
            let count = Arc::new(AtomicU64::new(0));
            let cancel = UploadCancellation::default();
            let mut reader = TrackedUploadReader::new(
                std::io::Cursor::new(vec![7; 100_000]),
                70_000,
                count.clone(),
                cancel.clone(),
            );
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            assert_eq!(bytes.len(), 70_000);
            verify_transfer(None, count, 70_000, &cancel).await?;
            let count = Arc::new(AtomicU64::new(0));
            let mut short = TrackedUploadReader::new(
                std::io::Cursor::new(vec![3; 7]),
                8,
                count.clone(),
                cancel.clone(),
            );
            assert!(short.read_to_end(&mut Vec::new()).await.is_err());
            assert!(verify_transfer(None, count, 8, &cancel).await.is_err());
            cancel.cancel();
            let mut stopped = TrackedUploadReader::new(
                std::io::Cursor::new(vec![1; 8]),
                8,
                Arc::new(AtomicU64::new(0)),
                cancel,
            );
            assert!(stopped.read_to_end(&mut Vec::new()).await.is_err());
            Ok::<_, anyhow::Error>(())
        })
    }
}
