use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// h2ck.me v1 AP-6 / T-11: cap on the size of any user-controlled
/// string that lands in a response body or a log line. Prevents an
/// attacker from crafting arbitrary-size error output by sending huge
/// inputs (a 4 MiB `sourceFilePath` used to surface as a 4 MiB error
/// message). 256 chars is enough to keep a real filename readable and
/// short enough that the response body stays predictable.
pub const MAX_USER_MESSAGE_LEN: usize = 256;

/// Truncate a user-controlled string to `MAX_USER_MESSAGE_LEN`
/// characters at a UTF-8 character boundary. If the input is longer,
/// the output ends in `...` so a downstream reader knows the value
/// was clipped. The result is guaranteed to be at most
/// `MAX_USER_MESSAGE_LEN` chars long.
///
/// This is a *response*-side defence. Full input still reaches the
/// operator log via a dedicated INFO/WARN field (also clipped, same
/// cap) so debugging works. Never lets an attacker choose the size
/// of an emitted string.
pub fn clip_user_message(s: &str) -> String {
    let count = s.chars().count();
    if count <= MAX_USER_MESSAGE_LEN {
        return s.to_string();
    }
    // Leave room for the trailing `...` marker inside the cap.
    let keep = MAX_USER_MESSAGE_LEN.saturating_sub(3);
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

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

    /// T-17: caller took too long to finish sending the JSON body.
    /// Defends the accept queue against slow-drip peers that would
    /// otherwise hold a connection slot for up to
    /// `limits.request_timeout_secs`. Surfaces as HTTP 408 with the
    /// structured `body_read_timeout` code.
    #[error("timed out waiting for request body")]
    BodyReadTimeout,

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
            FerryError::BodyReadTimeout => StatusCode::REQUEST_TIMEOUT,
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
            FerryError::BodyReadTimeout => "body_read_timeout",
            FerryError::Upstream(_) => "upstream_error",
            FerryError::Io(_) => "io_error",
            FerryError::Internal(_) => "internal_error",
        }
    }
}

impl IntoResponse for FerryError {
    fn into_response(self) -> Response {
        let status = self.status();
        // AP-6 / T-11: clip the Display output before it lands in the
        // JSON body. `BadQuery`, `BadBody`, `Upstream`, and `Io`
        // variants can carry user-controlled substrings (the axum
        // rejection text embeds the offending value verbatim, and
        // `io::Error` Display embeds the OS path). Cap at 256 chars so
        // a caller can't inflate the response body by sending a huge
        // input.
        let message = clip_user_message(&self.to_string());
        let body = Json(json!({
            "error": self.code(),
            "message": message,
        }));
        // FN6: for NotFound, the response `message` is a fixed string
        // (`file not found`) so the caller doesn't get its own probe
        // input echoed back. The full user-supplied path is preserved
        // in the operator log at WARN — also clipped at 256 chars per
        // AP-6 so an attacker can't flood the log ingester with a
        // multi-MB path.
        if let FerryError::NotFound(path) = &self {
            let logged_path = clip_user_message(path);
            tracing::warn!(
                error.code = self.code(),
                requested_path = %logged_path,
                "request rejected: file not found"
            );
        } else if status.is_server_error() {
            // A 5xx warrants an operator log.
            tracing::warn!(
                error.code = self.code(),
                error.message = %message,
                "request failed"
            );
        } else {
            // 4xx is user error, DEBUG only.
            tracing::debug!(
                error.code = self.code(),
                error.message = %message,
                "request rejected"
            );
        }
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_user_message_leaves_short_strings_unchanged() {
        assert_eq!(clip_user_message(""), "");
        assert_eq!(clip_user_message("abc"), "abc");
        // Right at the cap: no truncation, no marker.
        let at_cap: String = "x".repeat(MAX_USER_MESSAGE_LEN);
        assert_eq!(clip_user_message(&at_cap), at_cap);
    }

    #[test]
    fn clip_user_message_truncates_and_marks_long_strings() {
        // Just over the cap.
        let over: String = "x".repeat(MAX_USER_MESSAGE_LEN + 1);
        let clipped = clip_user_message(&over);
        // Result MUST fit inside the cap (marker included).
        assert_eq!(
            clipped.chars().count(),
            MAX_USER_MESSAGE_LEN,
            "output exceeded cap: {} chars",
            clipped.chars().count()
        );
        assert!(clipped.ends_with("..."), "marker missing: {clipped}");
    }

    #[test]
    fn clip_user_message_handles_multibyte_boundaries() {
        // `char_indices`-safe truncation: an input of only multi-byte
        // characters must not split a codepoint. Uses `€` (3 bytes)
        // repeated to overflow the cap.
        let over: String = "€".repeat(MAX_USER_MESSAGE_LEN + 10);
        let clipped = clip_user_message(&over);
        assert!(
            clipped.chars().count() <= MAX_USER_MESSAGE_LEN,
            "byte-truncation broke a codepoint or exceeded cap"
        );
        // Guarantee: the string is still valid UTF-8 (Rust invariant on
        // `String`, but assert to lock in the intent).
        assert!(std::str::from_utf8(clipped.as_bytes()).is_ok());
    }

    #[test]
    fn clip_user_message_amplification_ratio_stays_flat() {
        // Break-the-fix probe: growing the input MUST NOT grow the
        // output past the cap. Sample four input sizes across four
        // orders of magnitude — the emitted length stays bounded.
        for input_len in [512usize, 4_096, 65_536, 1_048_576] {
            let input: String = "A".repeat(input_len);
            let out = clip_user_message(&input);
            assert!(
                out.chars().count() <= MAX_USER_MESSAGE_LEN,
                "clip failed for input len {input_len}: got {} chars",
                out.chars().count()
            );
        }
    }
}
