//! Per-request INFO access-log middleware.
//!
//! Audit LOG-v1 FN-LOG-3 — before this, FileFerry produced ZERO log lines
//! for 45+ probes at RUST_LOG=info. A file-transfer service with zero
//! record of who copied what is a SOC2/HIPAA/GDPR compliance blocker.
//!
//! Emits exactly one INFO line per completed request:
//!   INFO http_request_completed method=POST route=/v1/files/copy
//!        status=200 duration_us=1234 trace_id=<hex>
//!
//! **trace_id inheritance** — Buerostack topology: Ruuter is the fleet's
//! reverse proxy. Every request FileFerry sees carries a W3C `traceparent`
//! header set by Ruuter. Extract the 32-char trace-id so Ruuter's log and
//! FileFerry's log can be correlated. Deps-free fallback (monotonic time)
//! when the header is absent.
//!
//! Deliberate omissions: no headers, no body, no file paths logged here
//! (path hashing for audit trail happens inside the copy handler with
//! explicit tenant-safety wording).

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub async fn access_log_middleware(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "<unmatched>".to_string());
    let trace_id = extract_trace_id(&req);

    let response = next.run(req).await;

    let status = response.status().as_u16();
    let duration_us = start.elapsed().as_micros();

    tracing::info!(
        method = %method,
        route = %route,
        status,
        duration_us,
        trace_id = %trace_id,
        "http_request_completed"
    );
    response
}

fn extract_trace_id(req: &Request) -> String {
    if let Some(tp) = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
    {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() == 4
            && parts[1].len() == 32
            && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return parts[1].to_string();
        }
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:016x}", nanos.wrapping_add(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest};

    #[test]
    fn inherits_trace_id_from_valid_traceparent() {
        let req = HttpRequest::builder()
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_trace_id(&req), "0af7651916cd43dd8448eb211c80319c");
    }

    #[test]
    fn generates_fresh_id_when_traceparent_absent() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        let tp = extract_trace_id(&req);
        assert_eq!(tp.len(), 16);
        assert!(tp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fresh_ids_are_distinct_across_calls() {
        let req1 = HttpRequest::builder().body(Body::empty()).unwrap();
        let req2 = HttpRequest::builder().body(Body::empty()).unwrap();
        assert_ne!(extract_trace_id(&req1), extract_trace_id(&req2));
    }
}
