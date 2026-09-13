use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FerryError {
    #[error("path is not valid: {0}")]
    InvalidPath(String),

    #[error("source and destination storage types must differ")]
    SameStorageType,

    #[error("storage backend {0:?} is not configured")]
    BackendNotConfigured(crate::model::StorageType),

    /// FN6 (h2ck.me v1 runtime): the `Display` of this variant is a
    /// fixed string that does NOT echo the user-supplied path. The
    /// inner `String` is kept for the operator-side `tracing::warn!`
    /// emitted from `IntoResponse` so debugging isn't blinded.
    #[error("file not found")]
    NotFound(String),

    #[error("transfer exceeded configured size cap of {cap} bytes")]
    TransferTooLarge { cap: u64 },

    /// F4: caller asked for a list page larger than the server allows.
    #[error("requested list limit exceeds server cap of {cap}")]
    ListLimitTooLarge { cap: usize },

    /// FN2: axum's typed `Query<T>` rejection wrapped into the fleet's
    /// structured error envelope. Kept separate from `InvalidPath` so
    /// clients can machine-distinguish "your path failed the validator"
    /// (`invalid_path`) from "your query string didn't deserialize"
    /// (`bad_query`).
    #[error("invalid query string: {0}")]
    BadQuery(String),

    /// FN2: axum's typed `Json<T>` rejection wrapped into the fleet's
    /// structured error envelope. Covers content-type mismatch, JSON
    /// syntax errors, missing/unknown fields.
    #[error("invalid request body: {0}")]
    BadBody(String),

    /// FN2: request body exceeded `limits.max_request_bytes`. Emitted
    /// as structured JSON 413 instead of tower's bare-text default.
    #[error("request body exceeds server cap of {cap} bytes")]
    BodyTooLarge { cap: usize },

    #[error("upstream storage error: {0}")]
    Upstream(String),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl FerryError {
    pub fn status(&self) -> StatusCode {
        match self {
            FerryError::InvalidPath(_)
            | FerryError::SameStorageType
            | FerryError::BadQuery(_)
            | FerryError::BadBody(_) => StatusCode::BAD_REQUEST,
            FerryError::BackendNotConfigured(_) => StatusCode::SERVICE_UNAVAILABLE,
            FerryError::NotFound(_) => StatusCode::NOT_FOUND,
            FerryError::TransferTooLarge { .. }
            | FerryError::ListLimitTooLarge { .. }
            | FerryError::BodyTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            FerryError::Upstream(_) => StatusCode::BAD_GATEWAY,
            FerryError::Io(_) | FerryError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            FerryError::InvalidPath(_) => "invalid_path",
            FerryError::SameStorageType => "same_storage_type",
            FerryError::BackendNotConfigured(_) => "backend_not_configured",
            FerryError::NotFound(_) => "not_found",
            FerryError::TransferTooLarge { .. } => "transfer_too_large",
            FerryError::ListLimitTooLarge { .. } => "list_limit_too_large",
            FerryError::BadQuery(_) => "bad_query",
            FerryError::BadBody(_) => "bad_body",
            FerryError::BodyTooLarge { .. } => "body_too_large",
            FerryError::Upstream(_) => "upstream_error",
            FerryError::Io(_) => "io_error",
            FerryError::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for FerryError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({
            "error": self.code(),
            "message": self.to_string(),
        }));
        // FN6: for NotFound, the response `message` is a fixed string
        // (`file not found`) so the caller doesn't get its own probe
        // input echoed back. The full user-supplied path is preserved
        // in the operator log at WARN so debugging still works.
        if let FerryError::NotFound(path) = &self {
            tracing::warn!(
                error.code = self.code(),
                requested_path = %path,
                "request rejected: file not found"
            );
        } else if status.is_server_error() {
            // A 5xx warrants an operator log.
            tracing::warn!(error.code = self.code(), error.message = %self, "request failed");
        } else {
            // 4xx is user error, DEBUG only.
            tracing::debug!(error.code = self.code(), error.message = %self, "request rejected");
        }
        (status, body).into_response()
    }
}
