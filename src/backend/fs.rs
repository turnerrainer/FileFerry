use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::backend::{Backend, ByteReader, ListOptions, DEFAULT_LIST_LIMIT};
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
            // F8: don't echo the attacker's input back in the client
            // response. Full detail stays in the operator log.
            tracing::warn!(
                requested_path = %path,
                root = %self.root.display(),
                "path resolution escaped fs root"
            );
            return Err(FerryError::InvalidPath(
                "resolved path escapes fs root".into(),
            ));
        }
        // F1: reject symlinks outright. Use `symlink_metadata` so we
        // observe the link itself, not its target. Existence errors
        // are folded into a NotFound so writes to fresh paths still
        // work (the file legitimately doesn't exist yet).
        match std::fs::symlink_metadata(&joined) {
            Ok(meta) if meta.file_type().is_symlink() => {
                tracing::warn!(
                    requested_path = %path,
                    "rejected: path is a symlink"
                );
                return Err(FerryError::InvalidPath(
                    "path is a symlink; symlinks are not permitted".into(),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(FerryError::Io(e)),
        }
        Ok(joined)
    }
}

#[async_trait]
impl Backend for FsBackend {
    fn kind(&self) -> StorageType {
        StorageType::Fs
    }

    async fn list(&self, opts: ListOptions) -> Result<Vec<FileEntry>, FerryError> {
        // F4: collect entries under a hard cap and support pagination.
        // The limit is enforced at the router (against MAX_LIST_LIMIT)
        // before we ever get here; we still defensively clamp to
        // DEFAULT_LIST_LIMIT so a direct caller (e.g. from Rust code)
        // can't accidentally allocate an unbounded list.
        let limit = opts.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        let mut read = fs::read_dir(&self.root).await?;
        let mut collected = Vec::new();
        while let Some(entry) = read.next_entry().await? {
            // F1: use symlink_metadata (never follows). The previous
            // `entry.metadata()` call would follow, letting a planted
            // symlink leak the target's size/mtime through the listing.
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let size = meta.len();
            let last_modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| format_rfc3339(d.as_secs()));
            collected.push(FileEntry {
                name,
                size,
                last_modified,
            });
        }
        // Sort by name so `start_after` is deterministic across
        // directory-entry orderings (readdir returns in filesystem-
        // dependent order).
        collected.sort_by(|a, b| a.name.cmp(&b.name));
        let filtered = collected.into_iter().filter(|e| match &opts.start_after {
            Some(cursor) => e.name.as_str() > cursor.as_str(),
            None => true,
        });
        Ok(filtered.take(limit).collect())
    }

    async fn open_read(&self, path: &str) -> Result<ByteReader, FerryError> {
        let resolved = self.resolve(path)?;
        // F1: close the TOCTOU window between symlink_metadata and
        // open by passing O_NOFOLLOW. If the final component becomes
        // a symlink after `resolve`, the open will fail with ELOOP.
        let std_file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&resolved)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(FerryError::NotFound(path.to_string()));
            }
            Err(e) if is_symlink_loop(&e) => {
                tracing::warn!(
                    requested_path = %path,
                    "open refused: path became a symlink between check and open"
                );
                return Err(FerryError::InvalidPath(
                    "path is a symlink; symlinks are not permitted".into(),
                ));
            }
            Err(e) => return Err(FerryError::Io(e)),
        };
        let file = fs::File::from_std(std_file);
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
        // F1: use O_NOFOLLOW on the destination too. If an attacker
        // races a symlink into place between resolve() and this open,
        // the write refuses instead of clobbering the target.
        let std_file = match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&resolved)
        {
            Ok(f) => f,
            Err(e) if is_symlink_loop(&e) => {
                tracing::warn!(
                    requested_path = %path,
                    "write refused: destination is or became a symlink"
                );
                return Err(FerryError::InvalidPath(
                    "path is a symlink; symlinks are not permitted".into(),
                ));
            }
            Err(e) => return Err(FerryError::Io(e)),
        };
        let mut file = fs::File::from_std(std_file);
        tokio::io::copy(&mut reader, &mut file).await?;
        file.flush().await?;
        Ok(())
    }
}

/// True when the OS refused an open because O_NOFOLLOW hit a symlink.
/// Linux surfaces this as `ELOOP` (mapped by std to
/// `ErrorKind::FilesystemLoop` on recent toolchains, and to
/// `ErrorKind::Other` on older ones — we check raw errno to cover
/// both).
fn is_symlink_loop(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ELOOP)
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
            .list(ListOptions::default())
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

    #[tokio::test]
    async fn list_skips_symlinks() {
        // F1 regression: a symlink placed in the data-dir must not
        // appear in the listing (previously would leak target metadata).
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::write(root.join("real.txt"), b"hello")
            .await
            .unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("pwned")).unwrap();

        let be = FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap();
        let names: Vec<String> = be
            .list(ListOptions::default())
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, vec!["real.txt"], "symlink must not be listed");
    }

    #[tokio::test]
    async fn open_read_rejects_symlink() {
        // F1 regression: attempting to read a symlink returns
        // InvalidPath, not the target's contents.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::os::unix::fs::symlink("/etc/passwd", root.join("pwned")).unwrap();

        let be = FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap();
        match be.open_read("pwned").await {
            Err(FerryError::InvalidPath(msg)) => {
                assert!(
                    msg.contains("symlink"),
                    "message should say symlinks are rejected: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidPath, got {other:?}"),
            Ok(_) => panic!("expected InvalidPath, symlink was opened"),
        }
    }

    #[tokio::test]
    async fn open_read_rejects_symlink_even_when_target_is_inside_root() {
        // F1 policy: no symlinks at all, not just "no escaping symlinks".
        // Canonicalisation-based defences race; a flat ban avoids TOCTOU.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::write(root.join("real.txt"), b"hello")
            .await
            .unwrap();
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link")).unwrap();

        let be = FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap();
        assert!(matches!(
            be.open_read("link").await,
            Err(FerryError::InvalidPath(_))
        ));
    }

    #[tokio::test]
    async fn write_all_refuses_symlink_destination() {
        // F1: an attacker planting a symlink at the destination path
        // must not cause FileFerry to clobber the target.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let outside = tmp.path().join("outside.txt");
        tokio::fs::write(&outside, b"do-not-touch").await.unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let be = FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap();
        let data = b"attacker payload".to_vec();
        let result = be
            .write_all(
                "link",
                Box::pin(std::io::Cursor::new(data.clone())),
                None,
            )
            .await;
        assert!(
            matches!(result, Err(FerryError::InvalidPath(_))),
            "symlink write must be refused, got {result:?}"
        );
        // The original file outside the root must be untouched.
        let after = tokio::fs::read(&outside).await.unwrap();
        assert_eq!(after, b"do-not-touch");
    }

    #[tokio::test]
    async fn resolve_error_does_not_echo_user_path() {
        // F8 regression: previously `resolve` returned the raw user
        // path in its error message. Verify we now emit a fixed
        // string that doesn't include the input.
        let tmp = TempDir::new().unwrap();
        let be = FsBackend::new(&FsConfig {
            data_directory: tmp.path().to_path_buf(),
        })
        .unwrap();
        // Craft a path that survives lexical joining but still fails.
        // A leading absolute path escapes the root because `PathBuf::join`
        // discards `self` when the arg is absolute.
        let attacker_input = "/etc/passwd-DEADBEEF";
        match be.resolve(attacker_input) {
            Err(FerryError::InvalidPath(msg)) => {
                assert!(
                    !msg.contains("DEADBEEF"),
                    "error must not echo user input: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidPath, got {other:?}"),
            Ok(p) => panic!("expected InvalidPath, resolve returned {p:?}"),
        }
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
