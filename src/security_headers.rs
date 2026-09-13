//! Default security headers on every response.
//!
//! Reference: FLEET-STRONGHOLDS.md §5.1. FileFerry is a service-to-
//! service HTTP proxy — it does not talk to browsers directly — so the
//! browser-side controls (CSP, X-Frame-Options, Referrer-Policy) are
//! belt-and-braces. They cost nothing per response and defend against
//! the "dev/staging bypass of the reverse proxy that normally sets
//! them" class of misconfiguration.

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

/// Middleware: append the fleet's five default security headers to
/// every outbound response. Existing header values (e.g. set by a
/// downstream handler) are preserved.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    // The service never returns HTML; a strict CSP is safe and cheap.
    h.entry("content-security-policy")
        .or_insert(HeaderValue::from_static(
            "default-src 'none'; frame-ancestors 'none'",
        ));
    // HSTS: only meaningful for TLS deployments, but harmless for
    // plaintext (browsers ignore it). Two years + preload matches TIM.
    h.entry("strict-transport-security")
        .or_insert(HeaderValue::from_static(
            "max-age=63072000; includeSubDomains; preload",
        ));
    h.entry("x-frame-options")
        .or_insert(HeaderValue::from_static("DENY"));
    h.entry("x-content-type-options")
        .or_insert(HeaderValue::from_static("nosniff"));
    h.entry("referrer-policy")
        .or_insert(HeaderValue::from_static("no-referrer"));
    response
}
