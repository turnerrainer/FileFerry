//! Optional inter-service bearer-token gate on the state-changing /
//! backend-touching endpoints (`/v1/files*`).
//!
//! Reference: h2ck.me/FileFerry/v1/BREAK-TESTS/PUBLIC-EXPOSURE-FINDINGS.md
//! F-FF-1 (unauth `/v1/files` list — S3 API cost + object-name recon),
//! F-FF-2 (unauth `/v1/files/copy` — cross-backend exfil + S3 billing
//! DoS), F-FF-3 (`/api` schema leak).
//!
//! The middleware is only active when
//! `security.inter_service_token_env` is set to a live env var in
//! `fileferry.yaml`. Absent → gated routes stay open, matching the
//! pre-auth deployment posture (safe when a reverse proxy / service
//! mesh already authenticates every request before it reaches
//! FileFerry — see SECURITY.md).

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::router::AppState;

/// Bearer-token guard. Expects `Authorization: Bearer <token>` when a
/// token is configured; rejects every other shape with a structured
/// `401 unauthorized`. Constant-time compares the presented token
/// against the configured value to close the timing-side-channel gap
/// (fleet stronghold §3.2).
pub async fn require_bearer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // No token configured → gate is a no-op (backwards-compatible).
    let Some(expected) = state.config.security.inter_service_token.as_deref() else {
        return next.run(req).await;
    };
    let auth = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let presented = auth.strip_prefix("Bearer ").unwrap_or("").trim();
    if presented.is_empty() {
        return unauthorized("missing bearer token").into_response();
    }
    if !ct_eq_str(presented, expected) {
        return unauthorized("invalid bearer token").into_response();
    }
    next.run(req).await
}

/// Structured 401 that mirrors the fleet-wide `{error, message}`
/// envelope. Kept `pub(crate)` so nothing outside this module builds a
/// bespoke 401 shape.
fn unauthorized(message: &'static str) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized", "message": message })),
    )
}

/// Constant-time string equality. Rejects mismatched-length inputs
/// upfront (leaking token length is intentional — the length isn't the
/// secret, the value is).
fn ct_eq_str(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_str_matches_only_exact() {
        assert!(ct_eq_str("abc", "abc"));
        assert!(!ct_eq_str("abc", "abd"));
        assert!(!ct_eq_str("abc", "abcd"));
        assert!(!ct_eq_str("", "x"));
        assert!(ct_eq_str("", ""));
    }
}
