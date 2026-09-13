//! W3C Trace Context (`traceparent`) response emission.
//!
//! Reference: FLEET-STRONGHOLDS.md §1.6 / adoption checklist O1.
//! When callers send `traceparent: 00-<trace_id>-<parent_span>-<flags>`
//! we echo the same `trace_id` back (with our own span) so log-shippers
//! can correlate cross-service. When callers don't, we generate a
//! fresh trace_id so every response carries one.
//!
//! Also emits `x-trace-id: <trace_id>` for tooling that hasn't wired
//! the W3C format yet (matches Ruuter).

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

/// Middleware: extract or synthesise the trace-id, then attach both
/// `traceparent` and `x-trace-id` headers to the response.
pub async fn traceparent(req: Request, next: Next) -> Response {
    let trace_id = extract_or_generate_trace_id(&req);
    let span_id = generate_span_id();
    let traceparent_value = format!("00-{}-{}-01", trace_id, span_id);

    let mut response = next.run(req).await;
    if let Ok(v) = HeaderValue::from_str(&traceparent_value) {
        response.headers_mut().insert("traceparent", v);
    }
    if let Ok(v) = HeaderValue::from_str(&trace_id) {
        response.headers_mut().insert("x-trace-id", v);
    }
    response
}

/// Pull the 32-hex trace_id out of an inbound `traceparent` header, or
/// mint a fresh one if the caller didn't send one. Non-compliant input
/// (wrong shape, wrong hex length) is treated the same as "missing" —
/// safer than trusting attacker-controlled bytes verbatim.
pub fn extract_or_generate_trace_id(req: &Request) -> String {
    if let Some(hv) = req.headers().get("traceparent") {
        if let Ok(s) = hv.to_str() {
            // Expected shape: `00-<32 hex>-<16 hex>-<2 hex>`. Only the
            // trace_id matters for correlation.
            let parts: Vec<&str> = s.split('-').collect();
            if parts.len() == 4
                && parts[1].len() == 32
                && parts[1].chars().all(|c| c.is_ascii_hexdigit())
            {
                return parts[1].to_ascii_lowercase();
            }
        }
    }
    random_hex_128()
}

/// 16-char (64-bit) hex span id.
fn generate_span_id() -> String {
    let mut out = String::with_capacity(16);
    for byte in random_bytes::<8>() {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

/// 32-char (128-bit) hex trace id.
fn random_hex_128() -> String {
    let mut out = String::with_capacity(32);
    for byte in random_bytes::<16>() {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn random_bytes<const N: usize>() -> [u8; N] {
    // Small, dependency-free RNG. Uses process-time nanoseconds + a
    // per-process counter — collisions across restarts are irrelevant
    // (a fresh boot means a fresh trace tree). Not for crypto; that's
    // the `subtle` crate's job in src/auth.rs.
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = seed;
    let mut out = [0u8; N];
    for byte in out.iter_mut() {
        // xorshift64 — 64-bit period is fine for a 16-byte draw.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xff) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[test]
    fn generates_when_no_header() {
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let id = extract_or_generate_trace_id(&req);
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extracts_from_valid_traceparent() {
        let req = Request::builder()
            .uri("/")
            .header(
                "traceparent",
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            extract_or_generate_trace_id(&req),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    #[test]
    fn synthesises_when_header_malformed() {
        // Non-hex bytes in the trace_id slot must fall back to a fresh
        // id — never trust attacker-controlled bytes verbatim.
        let req = Request::builder()
            .uri("/")
            .header(
                "traceparent",
                "00-!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!-b7ad6b7169203331-01",
            )
            .body(Body::empty())
            .unwrap();
        let id = extract_or_generate_trace_id(&req);
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
