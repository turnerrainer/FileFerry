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

    #[error("file not found: {0}")]
    NotFound(String),

    #[error("transfer exceeded configured size cap of {cap} bytes")]
    TransferTooLarge { cap: u64 },

    /// F4: caller asked for a list page larger than the server allows.
    #[error("requested list limit exceeds server cap of {cap}")]
    ListLimitTooLarge { cap: usize },

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
            FerryError::InvalidPath(_) | FerryError::SameStorageType => StatusCode::BAD_REQUEST,
            FerryError::BackendNotConfigured(_) => StatusCode::SERVICE_UNAVAILABLE,
            FerryError::NotFound(_) => StatusCode::NOT_FOUND,
            FerryError::TransferTooLarge { .. } | FerryError::ListLimitTooLarge { .. } => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
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
        // A 5xx warrants an operator log; 4xx is user error, DEBUG only.
        if status.is_server_error() {
            tracing::warn!(error.code = self.code(), error.message = %self, "request failed");
        } else {
            tracing::debug!(error.code = self.code(), error.message = %self, "request rejected");
        }
        (status, body).into_response()
    }
}
