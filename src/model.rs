use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum StorageType {
    Fs,
    S3,
}

impl StorageType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            StorageType::Fs => "FS",
            StorageType::S3 => "S3",
        }
    }
}

/// Single file's metadata as reported by any backend's `list()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    /// Last-modified as an ISO-8601 UTC string. `None` when the backend
    /// does not report one (e.g. S3 object with no LastModified).
    #[serde(rename = "lastModified")]
    pub last_modified: Option<String>,
}

/// Response shape returned by `GET /v1/files`, mirroring S3-Ferry's
/// `{data, meta}` envelope.
#[derive(Debug, Serialize)]
pub struct ListFilesResponse {
    pub data: Vec<FileEntry>,
    pub meta: ListFilesMeta,
}

#[derive(Debug, Serialize)]
pub struct ListFilesMeta {
    pub count: usize,
    /// F4: when the page is full (`count == limit`), the client should
    /// request `?startAfter={nextCursor}` to fetch the next page. Absent
    /// when the response contains the tail of the listing.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListFilesQuery {
    #[serde(rename = "type")]
    pub storage_type: StorageType,
    /// F4: cap results per request. Absent = server default
    /// (`DEFAULT_LIST_LIMIT`); values above `MAX_LIST_LIMIT` are 413.
    #[serde(default)]
    pub limit: Option<usize>,
    /// F4: pagination cursor. Returns entries whose name is strictly
    /// greater than this string. Uses the S3 `start-after` convention.
    #[serde(default, rename = "startAfter")]
    pub start_after: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CopyFileRequest {
    #[serde(rename = "sourceStorageType")]
    pub source_storage_type: StorageType,
    #[serde(rename = "sourceFilePath")]
    pub source_file_path: String,
    #[serde(rename = "destinationStorageType")]
    pub destination_storage_type: StorageType,
    #[serde(rename = "destinationFilePath")]
    pub destination_file_path: String,
}
