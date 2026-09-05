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

/// F4: default cap on the number of entries a single `list()` call
/// returns. Callers can raise the cap via query params only up to
/// `MAX_LIST_LIMIT`; anything past that is refused with 413.
pub const DEFAULT_LIST_LIMIT: usize = 1_000;
pub const MAX_LIST_LIMIT: usize = 10_000;

/// F4: pagination options for `Backend::list`. Both backends must
/// return entries in name order so `start_after` is deterministic.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Return at most this many entries. `None` uses `DEFAULT_LIST_LIMIT`.
    /// Callers exceeding `MAX_LIST_LIMIT` are rejected at the router.
    pub limit: Option<usize>,
    /// Skip until the first entry whose name is strictly greater than
    /// this value. `None` starts from the first entry.
    pub start_after: Option<String>,
}

/// A pluggable storage backend. Every method must be safe to call
/// concurrently; implementations rely on backing clients (S3 client,
/// tokio fs handles) that are themselves `Send + Sync`.
#[async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> StorageType;

    /// List root-level files. Mirrors S3-Ferry semantics: entries whose
    /// key contains a `/` (i.e. lives in a nested "directory") are
    /// filtered out.
    ///
    /// F4: `opts.limit` caps entries returned (default
    /// `DEFAULT_LIST_LIMIT`); `opts.start_after` skips to a resume
    /// point. Entries are returned in name order.
    async fn list(&self, opts: ListOptions) -> Result<Vec<FileEntry>, FerryError>;

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
/// F3: `inactivity` bounds how long the source reader is allowed to
/// stall between successful reads. A slow-drip peer (1 byte/minute)
/// used to keep the request alive because each byte reset the total
/// timeout; this guard fires per poll.
///
/// Returns the number of bytes transferred on success.
pub async fn stream_copy(
    src: BackendRef,
    src_path: &str,
    dst: BackendRef,
    dst_path: &str,
    max_bytes: u64,
    inactivity: std::time::Duration,
) -> Result<u64, FerryError> {
    let reader = src.open_read(src_path).await?;
    // F3: wrap in a per-poll timeout BEFORE the byte cap so the source
    // side of the pipe is the one that gets aborted when it stalls.
    let mut timed = tokio_io_timeout::TimeoutReader::new(reader);
    timed.set_timeout(Some(inactivity));
    let (limited, counter) = LimitedReader::new(Box::pin(timed), max_bytes);
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
    async fn stream_copy_aborts_when_source_stalls() {
        // F3 regression: a source that never yields a byte must not
        // hold the transfer open indefinitely. Wire a StallBackend as
        // the source and a discarding sink as the destination; the copy
        // must error within ~2× the inactivity budget.
        use crate::model::{FileEntry, StorageType};
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct StallReader;
        impl tokio::io::AsyncRead for StallReader {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                _buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                // Never wakes — TimeoutReader must fire.
                Poll::Pending
            }
        }

        struct StallBackend;
        #[async_trait::async_trait]
        impl Backend for StallBackend {
            fn kind(&self) -> StorageType {
                StorageType::Fs
            }
            async fn list(
                &self,
                _opts: ListOptions,
            ) -> Result<Vec<FileEntry>, FerryError> {
                Ok(Vec::new())
            }
            async fn open_read(
                &self,
                _path: &str,
            ) -> Result<ByteReader, FerryError> {
                Ok(Box::pin(StallReader))
            }
            async fn write_all(
                &self,
                _path: &str,
                _reader: ByteReader,
                _size_hint: Option<u64>,
            ) -> Result<(), FerryError> {
                unreachable!("no bytes should ever reach the destination");
            }
        }

        struct SinkBackend;
        #[async_trait::async_trait]
        impl Backend for SinkBackend {
            fn kind(&self) -> StorageType {
                StorageType::S3
            }
            async fn list(
                &self,
                _opts: ListOptions,
            ) -> Result<Vec<FileEntry>, FerryError> {
                Ok(Vec::new())
            }
            async fn open_read(
                &self,
                _path: &str,
            ) -> Result<ByteReader, FerryError> {
                unreachable!();
            }
            async fn write_all(
                &self,
                _path: &str,
                mut reader: ByteReader,
                _size_hint: Option<u64>,
            ) -> Result<(), FerryError> {
                // Actually try to read — this is what triggers the
                // TimeoutReader's per-poll clock.
                let mut buf = [0u8; 64];
                use tokio::io::AsyncReadExt;
                let _ = reader.read(&mut buf).await?;
                Ok(())
            }
        }

        let src: BackendRef = Arc::new(StallBackend);
        let dst: BackendRef = Arc::new(SinkBackend);
        let inactivity = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream_copy(src, "in", dst, "out", 1024, inactivity),
        )
        .await;
        let elapsed = started.elapsed();
        let inner = result.expect("copy must complete (with error) inside outer timeout");
        assert!(inner.is_err(), "stalled source must error, got {inner:?}");
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "copy should abort ~near inactivity budget, took {elapsed:?}"
        );
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
