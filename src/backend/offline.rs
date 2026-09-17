//! Offline stub — returns a fixed 502 for every backend call. Wired in
//! only when `FILEFERRY_OFFLINE=1` (or `true`) at boot AND an S3 block
//! was configured. See fleet stronghold §9.1 (FLEET-STRONGHOLDS.md):
//! break-tests and pentest engagements must never accidentally contact
//! a live upstream. FS operations stay real because they never leave
//! the process's own filesystem — only S3 (or a future outbound
//! backend) is stubbed here.

use async_trait::async_trait;

use crate::backend::{Backend, ByteReader, ListOptions};
use crate::error::FerryError;
use crate::model::{FileEntry, StorageType};

pub struct OfflineBackend {
    /// Reported by `kind()` so `/v1/files?type=S3` still reflects the
    /// operator-configured type even when the outbound is stubbed.
    reports_as: StorageType,
}

impl OfflineBackend {
    pub fn new(reports_as: StorageType) -> Self {
        Self { reports_as }
    }
}

#[async_trait]
impl Backend for OfflineBackend {
    fn kind(&self) -> StorageType {
        self.reports_as
    }

    async fn list(&self, _opts: ListOptions) -> Result<Vec<FileEntry>, FerryError> {
        Err(FerryError::Upstream(
            "offline mode: outbound blocked by FILEFERRY_OFFLINE".into(),
        ))
    }

    async fn open_read(&self, _path: &str) -> Result<ByteReader, FerryError> {
        Err(FerryError::Upstream(
            "offline mode: outbound blocked by FILEFERRY_OFFLINE".into(),
        ))
    }

    async fn write_all(
        &self,
        _path: &str,
        _reader: ByteReader,
        _size_hint: Option<u64>,
    ) -> Result<(), FerryError> {
        Err(FerryError::Upstream(
            "offline mode: outbound blocked by FILEFERRY_OFFLINE".into(),
        ))
    }
}

/// True when `FILEFERRY_OFFLINE` is set to `1` or `true`
/// (case-insensitive). Any other value including empty = live outbound.
pub fn offline_from_env() -> bool {
    match std::env::var("FILEFERRY_OFFLINE") {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_env_parses_truthy_and_falsy_values() {
        // Env vars are process-wide shared state — splitting the
        // truthy / falsy / unset probes into three parallel
        // `#[test]`s races on the shared FILEFERRY_OFFLINE value.
        // Grouped into one serialised test. Same pattern as
        // `config::tests::admin_enabled_from_env_parses_*`.
        for val in ["1", "true", "TRUE", "True", "yes"] {
            std::env::set_var("FILEFERRY_OFFLINE", val);
            assert!(offline_from_env(), "{val:?} should mean offline");
        }
        for val in ["0", "false", "no", ""] {
            std::env::set_var("FILEFERRY_OFFLINE", val);
            assert!(!offline_from_env(), "{val:?} should mean online");
        }
        std::env::remove_var("FILEFERRY_OFFLINE");
        assert!(!offline_from_env(), "unset must default to online");
    }

    #[tokio::test]
    async fn offline_backend_lists_returns_upstream_error() {
        let be = OfflineBackend::new(StorageType::S3);
        match be.list(ListOptions::default()).await {
            Err(FerryError::Upstream(msg)) => assert!(msg.contains("offline")),
            other => panic!("expected Upstream(offline), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn offline_backend_open_read_returns_upstream_error() {
        let be = OfflineBackend::new(StorageType::S3);
        let result = be.open_read("anything.txt").await;
        assert!(
            matches!(result, Err(FerryError::Upstream(_))),
            "expected Upstream error from offline backend"
        );
    }
}
