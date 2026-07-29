use std::path::Path;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::Object;
use aws_sdk_s3::Client;
use tokio::io::AsyncWriteExt;

use crate::backend::{Backend, ByteReader};
use crate::config::S3Config;
use crate::error::FerryError;
use crate::model::{FileEntry, StorageType};

/// S3 (or S3-compatible) backend. Wraps `aws-sdk-s3::Client`.
///
/// Every operation runs against `bucket` under an optional key
/// prefix `bucket_path` — the prefix is prepended verbatim, so
/// operators wanting a trailing slash must include it themselves.
pub struct S3Backend {
    client: Client,
    bucket: String,
    bucket_path: String,
}

impl S3Backend {
    pub async fn new(cfg: &S3Config) -> Result<Self, FerryError> {
        let creds = Credentials::new(
            cfg.access_key_id.clone(),
            cfg.secret_access_key.clone(),
            None,
            None,
            "fileferry-static",
        );
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(cfg.region.clone()))
            .credentials_provider(creds);
        if !cfg.endpoint_url.is_empty() {
            loader = loader.endpoint_url(cfg.endpoint_url.clone());
        }
        let shared_cfg = loader.load().await;
        // `force_path_style(true)` matches S3-Ferry — S3-compatible
        // backends (MinIO, LocalStack, older non-DNS-compliant bucket
        // names) require it. AWS S3 accepts it too.
        let s3_cfg = aws_sdk_s3::config::Builder::from(&shared_cfg)
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(s3_cfg),
            bucket: cfg.bucket.clone(),
            bucket_path: cfg.bucket_path.clone(),
        })
    }

    /// Full S3 key = `bucket_path` + user-supplied path. `bucket_path`
    /// is prepended verbatim — no automatic slash insertion, matches
    /// S3-Ferry's `path.join` semantics minus its accidental
    /// `\`-collapse quirks (which don't apply here because Rust's
    /// `Path::join` is not used).
    fn key(&self, path: &str) -> String {
        if self.bucket_path.is_empty() {
            path.to_string()
        } else if self.bucket_path.ends_with('/') {
            format!("{}{}", self.bucket_path, path)
        } else {
            format!("{}/{}", self.bucket_path, path)
        }
    }

    /// Strip the configured `bucket_path` prefix from an S3 key so
    /// `list()` can filter and report keys as user-facing paths.
    fn strip_prefix<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        if self.bucket_path.is_empty() {
            return Some(key);
        }
        let prefix = if self.bucket_path.ends_with('/') {
            self.bucket_path.clone()
        } else {
            format!("{}/", self.bucket_path)
        };
        key.strip_prefix(&prefix)
    }
}

#[async_trait]
impl Backend for S3Backend {
    fn kind(&self) -> StorageType {
        StorageType::S3
    }

    async fn list(&self) -> Result<Vec<FileEntry>, FerryError> {
        // S3-Ferry lists the whole bucket root and drops any key
        // containing `/`. We instead list under the configured prefix
        // and filter keys whose remainder still contains a `/` (i.e.
        // objects nested another level deeper). Same net effect as
        // S3-Ferry when `bucket_path` is empty, but consistent with
        // the copy path when a prefix is set.
        let prefix_arg = if self.bucket_path.is_empty() {
            None
        } else if self.bucket_path.ends_with('/') {
            Some(self.bucket_path.clone())
        } else {
            Some(format!("{}/", self.bucket_path))
        };

        let mut req = self.client.list_objects_v2().bucket(&self.bucket);
        if let Some(p) = &prefix_arg {
            req = req.prefix(p.clone());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| FerryError::Upstream(format!("s3 list: {e}")))?;
        let contents: Vec<Object> = resp.contents.unwrap_or_default();
        let mut out = Vec::new();
        for obj in contents {
            let Some(key) = obj.key() else { continue };
            let Some(name) = self.strip_prefix(key) else {
                continue;
            };
            if name.is_empty() || name.contains('/') {
                continue;
            }
            let size = obj.size().unwrap_or(0);
            let size_u64 = if size < 0 { 0 } else { size as u64 };
            let last_modified = obj.last_modified().map(|t| {
                // `DateTime::to_string` yields RFC-3339 with subsecond
                // precision; `.fmt(Format::DateTime)` gives the trimmed
                // seconds-precision form we want in list responses.
                t.fmt(aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_else(|_| String::new())
            });
            out.push(FileEntry {
                name: name.to_string(),
                size: size_u64,
                last_modified,
            });
        }
        Ok(out)
    }

    async fn open_read(&self, path: &str) -> Result<ByteReader, FerryError> {
        let key = self.key(path);
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                // Distinguish 404 (NoSuchKey) from other upstream
                // errors so the HTTP layer maps to 404 vs 502.
                let svc_err = e.into_service_error();
                if svc_err.is_no_such_key() {
                    FerryError::NotFound(path.to_string())
                } else {
                    FerryError::Upstream(format!("s3 get: {svc_err}"))
                }
            })?;
        Ok(Box::pin(resp.body.into_async_read()))
    }

    async fn write_all(
        &self,
        path: &str,
        mut reader: ByteReader,
        _size_hint: Option<u64>,
    ) -> Result<(), FerryError> {
        // Buffer to a temp file first so we can hand aws-sdk-s3 a
        // ByteStream with a known Content-Length — the SDK doesn't
        // support chunked-transfer PutObject and rejects unsized
        // bodies. Trade-off: 2x local disk I/O on the writer host.
        // For the first release this is acceptable; a future task
        // can switch to true streaming via `SdkBody::from_body_1_x`.
        let tmp = tempfile_named()?;
        {
            let path = tmp.path().to_path_buf();
            let mut f = tokio::fs::File::create(&path).await?;
            tokio::io::copy(&mut reader, &mut f).await?;
            f.flush().await?;
        }
        let key = self.key(path);
        let body = ByteStream::read_from()
            .path(tmp.path())
            .build()
            .await
            .map_err(|e| FerryError::Upstream(format!("preparing upload: {e}")))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .send()
            .await
            .map_err(|e| FerryError::Upstream(format!("s3 put: {e}")))?;
        // Explicitly close the temp file so it's unlinked before we
        // return. `NamedTempFile` also unlinks on Drop, but doing it
        // here surfaces any filesystem error.
        let _ = tmp.close();
        Ok(())
    }
}

/// Wrapper that returns a `NamedTempFile` via `tempfile` — kept in
/// one place so tests can spy on temp-file creation if needed.
fn tempfile_named() -> Result<tempfile::NamedTempFile, FerryError> {
    tempfile::NamedTempFile::new().map_err(FerryError::Io)
}

// Kept for callers that need to inspect the configured root without
// coupling to the struct's private fields.
impl S3Backend {
    pub fn bucket(&self) -> &str {
        &self.bucket
    }
    pub fn bucket_path(&self) -> &str {
        &self.bucket_path
    }
    pub fn as_ref_client(&self) -> &Client {
        &self.client
    }
}

// Suppress a "field never read" warning if the client is only used
// via trait dispatch in some build configurations.
#[allow(dead_code)]
fn _touch_paths(_p: &Path) {}
