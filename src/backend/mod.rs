pub mod fs;
pub mod s3;

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncRead;

use crate::error::FerryError;
use crate::model::{FileEntry, StorageType};

pub type BackendRef = Arc<dyn Backend>;
pub type ByteReader = Pin<Box<dyn AsyncRead + Send + Sync>>;

/// A pluggable storage backend. Every method must be safe to call
/// concurrently; implementations rely on backing clients (S3 client,
/// tokio fs handles) that are themselves `Send + Sync`.
#[async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> StorageType;

    /// List root-level files. Mirrors S3-Ferry semantics: entries whose
    /// key contains a `/` (i.e. lives in a nested "directory") are
    /// filtered out.
    async fn list(&self) -> Result<Vec<FileEntry>, FerryError>;

    /// Open a reader over the named object. Callers are expected to
    /// drain the reader promptly; both backends hold connection or
    /// handle state per call.
    async fn open_read(&self, path: &str) -> Result<ByteReader, FerryError>;

    /// Consume a reader and write it to the named path, replacing any
    /// prior object. `size_hint` is passed through when available so
    /// the S3 backend can send a Content-Length upfront.
    async fn write_all(
        &self,
        path: &str,
        reader: ByteReader,
        size_hint: Option<u64>,
    ) -> Result<(), FerryError>;
}

/// Bundle of enabled backends. `s3` is `None` when no S3 block is
/// configured — reaching for it yields `BackendNotConfigured`.
#[derive(Clone)]
pub struct Backends {
    pub fs: BackendRef,
    pub s3: Option<BackendRef>,
}

impl Backends {
    pub fn pick(&self, kind: StorageType) -> Result<BackendRef, FerryError> {
        match kind {
            StorageType::Fs => Ok(Arc::clone(&self.fs)),
            StorageType::S3 => self
                .s3
                .as_ref()
                .map(Arc::clone)
                .ok_or(FerryError::BackendNotConfigured(StorageType::S3)),
        }
    }
}

/// Stream-copy from source backend → destination backend, enforcing a
/// hard cap on total bytes read. The cap protects against runaway
/// large downloads (S3 → local disk fill) and runaway uploads
/// (local → S3 unbounded cost).
///
/// Returns the number of bytes transferred on success.
pub async fn stream_copy(
    src: BackendRef,
    src_path: &str,
    dst: BackendRef,
    dst_path: &str,
    max_bytes: u64,
) -> Result<u64, FerryError> {
    let reader = src.open_read(src_path).await?;
    let (limited, counter) = LimitedReader::new(reader, max_bytes);
    dst.write_all(dst_path, Box::pin(limited), None).await?;
    let total = counter.load();
    tracing::info!(
        source = src.kind().as_str(),
        source_path = src_path,
        destination = dst.kind().as_str(),
        destination_path = dst_path,
        bytes = total,
        "copy complete"
    );
    Ok(total)
}

// ---------------------------------------------------------------
// LimitedReader — caps the total bytes yielded by an AsyncRead. Once
// the cap is exceeded, subsequent `poll_read` calls return an error
// wrapping `FerryError::TransferTooLarge`.
// ---------------------------------------------------------------

use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

pub(crate) struct LimitedReader<R> {
    inner: R,
    max: u64,
    counter: Arc<AtomicU64>,
}

impl<R: AsyncRead + Unpin + Send> LimitedReader<R> {
    pub fn new(inner: R, max: u64) -> (Self, Arc<AtomicU64>) {
        let counter = Arc::new(AtomicU64::new(0));
        (
            Self {
                inner,
                max,
                counter: Arc::clone(&counter),
            },
            counter,
        )
    }
}

impl<R: AsyncRead + Unpin + Send> AsyncRead for LimitedReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Already exceeded on a prior call — refuse before reading
        // another byte. `>` (not `>=`) here so a cap == total ==
        // clean-EOF case doesn't error on the follow-up call.
        if AtomicU64::load(&self.counter, Ordering::Relaxed) > self.max {
            return Poll::Ready(Err(std::io::Error::other(format!(
                "transfer exceeded configured size cap of {} bytes",
                self.max
            ))));
        }
        let before = buf.filled().len();
        let inner = pin!(&mut self.inner);
        match inner.poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let added = (buf.filled().len() - before) as u64;
                let new_total = self.counter.fetch_add(added, Ordering::Relaxed) + added;
                if new_total > self.max {
                    // Roll the cursor back so tokio's read_to_end
                    // (which asserts the buffer is consistent after
                    // an error) doesn't see partial garbage.
                    buf.set_filled(before);
                    return Poll::Ready(Err(std::io::Error::other(format!(
                        "transfer exceeded configured size cap of {} bytes",
                        self.max
                    ))));
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

trait AtomicU64Load {
    fn load(&self) -> u64;
}

impl AtomicU64Load for Arc<AtomicU64> {
    fn load(&self) -> u64 {
        self.as_ref().load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn limited_reader_allows_up_to_cap() {
        let data = [7u8; 100];
        let (mut reader, counter) = LimitedReader::new(&data[..], 100);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out.len(), 100);
        assert_eq!(counter.load(), 100);
    }

    #[tokio::test]
    async fn limited_reader_errors_over_cap() {
        let data = [7u8; 100];
        let (mut reader, _c) = LimitedReader::new(&data[..], 50);
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).await.unwrap_err();
        assert!(err.to_string().contains("50 bytes"), "{err}");
    }

    #[tokio::test]
    async fn limited_reader_cap_of_zero_rejects_any_byte() {
        // Edge case: a 0-byte cap must reject the first byte. Without an
        // explicit case for this a naive `if total > max` would let the
        // very first byte through when `max = 0`. This test locks in the
        // strict interpretation ("0 means literally nothing").
        let data = [1u8; 1];
        let (mut reader, _c) = LimitedReader::new(&data[..], 0);
        let mut out = Vec::new();
        assert!(reader.read_to_end(&mut out).await.is_err());
    }
}
