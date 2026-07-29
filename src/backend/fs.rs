use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::backend::{Backend, ByteReader};
use crate::config::FsConfig;
use crate::error::FerryError;
use crate::model::{FileEntry, StorageType};

/// Local-filesystem backend. Roots every operation at
/// `config.data_directory` — user-supplied paths are validated by the
/// axum handler layer before reaching here, but we still resolve
/// against the root and reject anything that escapes it (defence in
/// depth against a future validator regression).
pub struct FsBackend {
    root: PathBuf,
}

impl FsBackend {
    pub fn new(cfg: &FsConfig) -> std::io::Result<Self> {
        // Create the directory on boot if it doesn't exist. Operators
        // that mount a volume don't have to pre-create the mountpoint.
        std::fs::create_dir_all(&cfg.data_directory)?;
        Ok(Self {
            root: cfg.data_directory.clone(),
        })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, FerryError> {
        let joined = self.root.join(path);
        // Reject the assembled path if it escapes the root. This can
        // happen if a future validator change lets `..` through.
        let canonical_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        // We can't canonicalize `joined` when it doesn't exist (e.g.
        // on `write_all` for a new file). Fall back to a lexical check
        // relative to the root.
        let ok_prefix = joined
            .ancestors()
            .any(|a| a == canonical_root || a == self.root);
        if !ok_prefix {
            return Err(FerryError::InvalidPath(format!(
                "resolved path escapes fs root: {}",
                path
            )));
        }
        Ok(joined)
    }
}

#[async_trait]
impl Backend for FsBackend {
    fn kind(&self) -> StorageType {
        StorageType::Fs
    }

    async fn list(&self) -> Result<Vec<FileEntry>, FerryError> {
        let mut read = fs::read_dir(&self.root).await?;
        let mut out = Vec::new();
        while let Some(entry) = read.next_entry().await? {
            let meta = entry.metadata().await?;
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let size = meta.len();
            let last_modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| format_rfc3339(d.as_secs()));
            out.push(FileEntry {
                name,
                size,
                last_modified,
            });
        }
        Ok(out)
    }

    async fn open_read(&self, path: &str) -> Result<ByteReader, FerryError> {
        let resolved = self.resolve(path)?;
        let file = match fs::File::open(&resolved).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(FerryError::NotFound(path.to_string()));
            }
            Err(e) => return Err(FerryError::Io(e)),
        };
        Ok(Box::pin(file))
    }

    async fn write_all(
        &self,
        path: &str,
        mut reader: ByteReader,
        _size_hint: Option<u64>,
    ) -> Result<(), FerryError> {
        let resolved = self.resolve(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut file = fs::File::create(&resolved).await?;
        tokio::io::copy(&mut reader, &mut file).await?;
        file.flush().await?;
        Ok(())
    }
}

/// Format a UNIX epoch seconds value as an ISO-8601 UTC string
/// (`YYYY-MM-DDTHH:MM:SSZ`). Purposefully self-contained — no chrono
/// dependency for a five-line formatter.
fn format_rfc3339(secs: u64) -> String {
    // Days since 1970-01-01 (proleptic Gregorian). Algorithm from
    // Howard Hinnant's "chrono-Compatible Low-Level Date Algorithms".
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60,
    );

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_out = if mp < 10 { mp + 3 } else { mp - 9 };
    let y_out = if m_out <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y_out, m_out, d, h, m, s
    )
}

impl AsRef<Path> for FsBackend {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn list_returns_only_files_in_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::write(root.join("a.txt"), b"hello")
            .await
            .unwrap();
        tokio::fs::write(root.join("b.txt"), b"world")
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("nested"))
            .await
            .unwrap();
        tokio::fs::write(root.join("nested/c.txt"), b"nope")
            .await
            .unwrap();

        let be = FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap();

        let mut names: Vec<String> = be
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        names.sort();
        // Only root-level files, matching S3-Ferry's semantics.
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }

    #[tokio::test]
    async fn open_read_missing_is_notfound() {
        let tmp = TempDir::new().unwrap();
        let be = FsBackend::new(&FsConfig {
            data_directory: tmp.path().to_path_buf(),
        })
        .unwrap();
        match be.open_read("nope.txt").await {
            Err(FerryError::NotFound(_)) => {}
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(_) => panic!("expected NotFound, got Ok"),
        }
    }

    #[tokio::test]
    async fn write_all_then_open_read_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let be = FsBackend::new(&FsConfig {
            data_directory: tmp.path().to_path_buf(),
        })
        .unwrap();
        let data = b"round-trip payload".to_vec();
        be.write_all(
            "sub/dir/file.bin",
            Box::pin(std::io::Cursor::new(data.clone())),
            None,
        )
        .await
        .unwrap();
        let mut reader = be.open_read("sub/dir/file.bin").await.unwrap();
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf)
            .await
            .unwrap();
        assert_eq!(buf, data);
    }

    #[test]
    fn rfc3339_known_value() {
        // 2020-01-01T00:00:00Z == 1577836800 epoch seconds.
        assert_eq!(format_rfc3339(1_577_836_800), "2020-01-01T00:00:00Z");
        // Also check a value inside a leap year on 29-Feb.
        assert_eq!(format_rfc3339(1_582_934_400), "2020-02-29T00:00:00Z");
        // Epoch itself.
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
    }
}
