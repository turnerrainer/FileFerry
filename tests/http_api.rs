//! Integration tests for the FileFerry HTTP surface. Exercises the
//! axum router end-to-end via `tower::ServiceExt::oneshot` — no
//! network binding, no port juggling. The S3 backend is exercised
//! only insofar as the router rejects requests targeting it when no
//! S3 block is configured; live S3 wire tests live behind a separate
//! feature flag (see `tests/s3_live.rs` — feature-gated so CI runs
//! quickly without a LocalStack sidecar).

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use fileferry::backend::fs::FsBackend;
use fileferry::backend::{BackendRef, Backends};
use fileferry::config::{AppConfig, FsConfig, SecurityConfig};
use fileferry::router::{build_router, AppState};

fn app_state_with_fs_only(root: &std::path::Path) -> AppState {
    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap(),
    );
    AppState {
        backends: Backends { fs, s3: None },
        config: Arc::new(AppConfig::default()),
    }
}

/// F-FF-3: `/api` defaults to 404 (admin off). Tests that specifically
/// exercise `/api` need to opt into admin_enabled=true to reach the
/// document handler.
fn app_state_with_admin_enabled(root: &std::path::Path) -> AppState {
    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap(),
    );
    let mut cfg = AppConfig::default();
    cfg.security.admin_enabled = true;
    AppState {
        backends: Backends { fs, s3: None },
        config: Arc::new(cfg),
    }
}

async fn parse_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn root_returns_banner() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json, json!({"data": "FileFerry"}));
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    // Fleet stronghold §5.1: every response must have the five default
    // security headers. Sample the public routes (200) AND an error
    // response (400) to confirm the middleware wraps both branches.
    // F-FF-3: `/api` requires admin_enabled=true to reach the doc
    // handler; use the admin-enabled state so the header assertions
    // exercise the same code path a live admin-enabled deployment
    // would.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_admin_enabled(tmp.path()));
    let paths = ["/", "/health", "/api"];
    for path in paths {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        for expected in [
            "content-security-policy",
            "strict-transport-security",
            "x-frame-options",
            "x-content-type-options",
            "referrer-policy",
        ] {
            assert!(
                resp.headers().get(expected).is_some(),
                "{path} response missing header {expected}"
            );
        }
    }
    // Error path (400 → missing ?type=).
    let err_resp = app
        .oneshot(Request::get("/v1/files").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(err_resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        err_resp.headers().get("x-content-type-options").is_some(),
        "error responses must also carry security headers"
    );
}

#[tokio::test]
async fn every_response_carries_traceparent_and_x_trace_id() {
    // Fleet stronghold §1.6 / O1: every response echoes/generates a
    // W3C trace_id so log-shippers can correlate across services.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let traceparent = resp
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let x_trace_id = resp
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        traceparent.starts_with("00-") && traceparent.len() > 40,
        "expected W3C traceparent shape, got {traceparent:?}"
    );
    assert_eq!(
        x_trace_id.len(),
        32,
        "x-trace-id must be 32-char hex, got {x_trace_id:?}"
    );
    // The trace_id in `traceparent` must equal `x-trace-id`.
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!(parts.get(1).copied().unwrap_or(""), x_trace_id);
}

#[tokio::test]
async fn traceparent_inbound_id_is_echoed() {
    // W3C: when the caller sends a valid traceparent, the same trace_id
    // must come back — that's the whole cross-service correlation point.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/health")
                .header(
                    "traceparent",
                    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let x_trace_id = resp
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(x_trace_id, "0af7651916cd43dd8448eb211c80319c");
}

#[tokio::test]
async fn health_returns_ok() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json, json!({"status": "ok"}));
}

#[tokio::test]
async fn openapi_lists_expected_paths() {
    // F-FF-3: `/api` requires FILEFERRY_ADMIN_ENABLED=1 (or, in tests,
    // security.admin_enabled=true) to reach the doc handler.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_admin_enabled(tmp.path()));
    let resp = app
        .oneshot(Request::get("/api").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    let paths = json.get("paths").unwrap().as_object().unwrap();
    for p in ["/", "/health", "/v1/files", "/v1/files/copy"] {
        assert!(paths.contains_key(p), "openapi missing {p}");
    }
}

#[tokio::test]
async fn openapi_returns_404_when_admin_disabled_by_default() {
    // F-FF-3 regression: `/api` is a recon endpoint (leaks the route
    // table + version) and defaults to 404 unless the operator sets
    // FILEFERRY_ADMIN_ENABLED. `AppConfig::default()` leaves
    // admin_enabled=false, so `/api` on a default deployment must
    // return 404 with no body content that leaks the version.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(Request::get("/api").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // The 404 must not leak the CARGO_PKG_VERSION nor the route
    // table — those are exactly the recon signals AP-2 targets.
    assert!(
        !text.contains(env!("CARGO_PKG_VERSION")),
        "404 body leaked version: {text:?}"
    );
    assert!(
        !text.contains("/v1/files"),
        "404 body leaked route table: {text:?}"
    );
}

#[tokio::test]
async fn openapi_returns_404_when_admin_enabled_but_docs_disabled() {
    // F-FF-3 dual-gate: `documentation_enabled=false` still 404s the
    // response even when admin_enabled=true. Lets operators keep the
    // env-gate on for tooling while silencing the doc endpoint via
    // YAML.
    let tmp = TempDir::new().unwrap();
    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: tmp.path().to_path_buf(),
        })
        .unwrap(),
    );
    let mut cfg = AppConfig::default();
    cfg.security.admin_enabled = true;
    cfg.documentation_enabled = false;
    let state = AppState {
        backends: Backends { fs, s3: None },
        config: Arc::new(cfg),
    };
    let app = build_router(state);
    let resp = app
        .oneshot(Request::get("/api").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_fs_files_reports_root_only() {
    let tmp = TempDir::new().unwrap();
    tokio::fs::write(tmp.path().join("hello.txt"), b"abc")
        .await
        .unwrap();
    tokio::fs::write(tmp.path().join("world.bin"), b"defgh")
        .await
        .unwrap();
    tokio::fs::create_dir_all(tmp.path().join("nested"))
        .await
        .unwrap();
    tokio::fs::write(tmp.path().join("nested/inner.txt"), b"skip me")
        .await
        .unwrap();

    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    let count = json
        .get("meta")
        .unwrap()
        .get("count")
        .unwrap()
        .as_u64()
        .unwrap();
    assert_eq!(count, 2, "should report only root-level files, got {json}");
    let names: std::collections::HashSet<String> = json
        .get("data")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        std::collections::HashSet::from(["hello.txt".to_string(), "world.bin".to_string()])
    );
}

#[tokio::test]
async fn list_s3_returns_503_when_not_configured() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=S3")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "backend_not_configured");
}

#[tokio::test]
async fn list_missing_type_query_is_422() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(Request::get("/v1/files").body(Body::empty()).unwrap())
        .await
        .unwrap();
    // axum's Query rejection surfaces as 400 (BAD_REQUEST) for
    // deserialization failures. Locking that expectation in so a
    // future axum bump that changes it fails visibly.
    assert!(
        matches!(resp.status(), StatusCode::BAD_REQUEST),
        "expected 400 for missing ?type=, got {}",
        resp.status()
    );
    // FN2 regression: response is structured JSON, not bare text.
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected application/json, got {content_type}"
    );
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "bad_query");
    assert!(json.get("message").unwrap().is_string());
}

#[tokio::test]
async fn list_bogus_type_is_400() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=NAS")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // FN2 regression: structured JSON body, not bare text.
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "bad_query");
}

#[tokio::test]
async fn list_bad_limit_returns_structured_bad_query() {
    // FN2 regression: previously axum returned a bare-text 400 for
    // `limit=abc`. Now it must be structured JSON `bad_query`.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS&limit=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected application/json, got {content_type}"
    );
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "bad_query");
}

#[tokio::test]
async fn copy_malformed_json_body_returns_structured_bad_body() {
    // FN2 regression: previously axum returned bare-text 400/415 for a
    // malformed JSON body. Now it must be structured JSON `bad_body`.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from("{not-json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected application/json, got {content_type}"
    );
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "bad_body");
}

#[tokio::test(start_paused = true)]
async fn slow_body_times_out_at_body_read_cap() {
    // T-17: a peer that opens `POST /v1/files/copy` but never
    // finishes sending the body used to hold a connection slot for
    // up to `limits.request_timeout_secs` (5 min default). Now
    // `TypedJson::from_request` wraps the inner body read in a
    // 30 s hard cap; on expiry the caller gets `408 Request Timeout`
    // with a structured `body_read_timeout` code.
    //
    // Uses `start_paused = true` + `tokio::time::advance` so the
    // test runs in wall-clock ms, not 30 s. The stream feeding the
    // body is `futures::stream::pending()` — yields `Pending`
    // forever, so the ONLY way `Json::from_request` can complete
    // is via the timeout wrapper.
    use bytes::Bytes;
    use futures::stream;
    let stream = stream::pending::<Result<Bytes, std::io::Error>>();
    let body = Body::from_stream(stream);

    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let response_task = tokio::spawn(async move {
        app.oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
    });
    // Advance past the body-read cap (30 s per BODY_READ_TIMEOUT).
    // The `tokio::time::timeout` inside `TypedJson::from_request`
    // should fire, producing `FerryError::BodyReadTimeout`.
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let resp = response_task.await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::REQUEST_TIMEOUT,
        "slow body must map to 408, got {}",
        resp.status()
    );
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "body_read_timeout");
}

#[tokio::test]
async fn copy_oversize_body_returns_structured_413() {
    // FN2 regression: previously the tower `RequestBodyLimitLayer`
    // returned bare-text 413. Now the extractor-side `DefaultBodyLimit`
    // + `TypedJson` combo surfaces it as structured JSON `body_too_large`.
    let tmp = TempDir::new().unwrap();
    let mut cfg = AppConfig::default();
    // Shrink the cap so the test body is comfortably over it.
    cfg.limits.max_request_bytes = 1024;
    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: tmp.path().to_path_buf(),
        })
        .unwrap(),
    );
    let state = AppState {
        backends: Backends { fs, s3: None },
        config: Arc::new(cfg),
    };
    let app = build_router(state);
    // 16 KiB — well above the 1 KiB cap.
    let big = vec![b'A'; 16 * 1024];
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(big))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected application/json, got {content_type}"
    );
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "body_too_large");
}

#[tokio::test]
async fn copy_rejects_same_source_and_destination() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "a.txt",
        "destinationStorageType": "FS",
        "destinationFilePath": "b.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "same_storage_type");
}

#[tokio::test]
async fn copy_rejects_traversal_path() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "../etc/passwd",
        "destinationStorageType": "S3",
        "destinationFilePath": "leak.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "invalid_path");
}

#[tokio::test]
async fn copy_rejects_unknown_field_in_body() {
    // FN-LOG-2 regression (h2ck.me v1 LOG break-tests): before adding
    // `#[serde(deny_unknown_fields)]`, junk fields were silently
    // dropped by serde. Now an unknown field must fail deserialisation
    // so schema-shape probes leave a signal in the log.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "a.txt",
        "destinationStorageType": "S3",
        "destinationFilePath": "b.txt",
        "attacker_controlled_field": "pwned"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected 400/422, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn list_rejects_unknown_query_field() {
    // FN-LOG-2 regression: unknown query params must also be refused.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS&attacker_field=pwned")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected 400/422 for unknown query field, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn copy_rejects_null_byte_path() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "harmless\u{0000}injected",
        "destinationStorageType": "S3",
        "destinationFilePath": "x.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn copy_rejects_bare_dot_source_path() {
    // FN1 regression (h2ck.me v1 runtime break-tests): a bare `.` used
    // to bypass validate_path and hit the FS as the data directory
    // itself, returning 500 with a raw OS error 21 (EISDIR) message.
    // Post-fix the validator refuses `.` at the boundary → structured
    // 400 invalid_path.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": ".",
        "destinationStorageType": "S3",
        "destinationFilePath": "leak.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "invalid_path");
    let msg = json.get("message").unwrap().as_str().unwrap();
    assert!(
        !msg.contains("os error"),
        "response must not leak raw OS error: {msg}"
    );
}

#[tokio::test]
async fn copy_source_not_found_maps_to_404() {
    let tmp = TempDir::new().unwrap();
    // Enable a "fake S3" as destination via a second FsBackend so the
    // copy handler passes the sourceStorageType != destinationStorageType
    // check. This is a legitimate test of the seam (FerryError::NotFound
    // → 404) — the fact that the "S3" is actually another local dir is
    // irrelevant at the HTTP layer.
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    tokio::fs::create_dir_all(&src_root).await.unwrap();
    tokio::fs::create_dir_all(&dst_root).await.unwrap();

    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: src_root.clone(),
        })
        .unwrap(),
    );
    let fake_s3: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: dst_root.clone(),
        })
        .unwrap(),
    );
    let state = AppState {
        backends: Backends {
            fs,
            s3: Some(fake_s3),
        },
        config: Arc::new(AppConfig::default()),
    };
    let app = build_router(state);
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "does-not-exist.txt",
        "destinationStorageType": "S3",
        "destinationFilePath": "target.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "not_found");
    // FN6: response body must NOT echo the caller-supplied source
    // file name. Fixed message only; log carries the actual path.
    let msg = json.get("message").unwrap().as_str().unwrap();
    assert!(
        !msg.contains("does-not-exist.txt"),
        "not_found message must not echo probed name: {msg}"
    );
}

#[tokio::test]
async fn copy_not_found_message_does_not_echo_marker() {
    // FN6 regression (h2ck.me v1 runtime FN6): use a distinctive
    // marker as the probed filename and assert it never appears in
    // the response body. This is a stronger check than the sibling
    // test — the marker is unique so a partial-match wouldn't collide
    // with an existing test fixture.
    let tmp = TempDir::new().unwrap();
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    tokio::fs::create_dir_all(&src_root).await.unwrap();
    tokio::fs::create_dir_all(&dst_root).await.unwrap();

    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: src_root.clone(),
        })
        .unwrap(),
    );
    let fake_s3: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: dst_root.clone(),
        })
        .unwrap(),
    );
    let state = AppState {
        backends: Backends {
            fs,
            s3: Some(fake_s3),
        },
        config: Arc::new(AppConfig::default()),
    };
    let app = build_router(state);
    let marker = "unique_marker_DEADBEEF";
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": marker,
        "destinationStorageType": "S3",
        "destinationFilePath": "target.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        !body_str.contains(marker),
        "response body must not echo probe marker: {body_str}"
    );
}

#[tokio::test]
async fn copy_happy_path_transfers_bytes() {
    let tmp = TempDir::new().unwrap();
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    tokio::fs::create_dir_all(&src_root).await.unwrap();
    tokio::fs::create_dir_all(&dst_root).await.unwrap();

    let payload: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    tokio::fs::write(src_root.join("input.bin"), &payload)
        .await
        .unwrap();

    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: src_root.clone(),
        })
        .unwrap(),
    );
    let fake_s3: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: dst_root.clone(),
        })
        .unwrap(),
    );
    let state = AppState {
        backends: Backends {
            fs,
            s3: Some(fake_s3),
        },
        config: Arc::new(AppConfig::default()),
    };
    let app = build_router(state);
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "input.bin",
        "destinationStorageType": "S3",
        "destinationFilePath": "output.bin"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let written = tokio::fs::read(dst_root.join("output.bin")).await.unwrap();
    assert_eq!(written, payload);
}

#[tokio::test]
async fn list_pagination_returns_next_cursor_when_full() {
    // F4 regression: with limit=2 across 3 files, page 1 returns
    // meta.nextCursor pointing at the last name; page 2 (with
    // startAfter=cursor) returns the remainder without a cursor.
    let tmp = TempDir::new().unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        tokio::fs::write(tmp.path().join(n), b"x").await.unwrap();
    }
    let app = build_router(app_state_with_fs_only(tmp.path()));

    let resp = app
        .clone()
        .oneshot(
            Request::get("/v1/files?type=FS&limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    let names: Vec<String> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["a.txt", "b.txt"]);
    let cursor = json["meta"]["nextCursor"]
        .as_str()
        .expect("nextCursor present when page fills")
        .to_string();
    assert_eq!(cursor, "b.txt");

    let resp = app
        .oneshot(
            Request::get(format!("/v1/files?type=FS&limit=2&startAfter={cursor}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = parse_json(resp.into_body()).await;
    let names: Vec<String> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["c.txt"]);
    assert!(
        json["meta"].get("nextCursor").is_none(),
        "tail page must not carry a cursor"
    );
}

#[tokio::test]
async fn list_limit_over_cap_returns_413() {
    // F4 regression: a `limit` above MAX_LIST_LIMIT (10_000) is 413.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS&limit=10001")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json["error"], "list_limit_too_large");
}

#[tokio::test]
async fn list_start_after_rejects_traversal() {
    // F4 defence-in-depth: the pagination cursor is passed through the
    // same validator as user paths, so a malicious cursor can't smuggle
    // `..` into the S3 request.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS&startAfter=..%2Fetc%2Fpasswd")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json["error"], "invalid_path");
}

#[tokio::test]
async fn copy_enforces_max_response_bytes() {
    // Verify the transfer size cap actually fires at the HTTP layer.
    // Try to copy a 200 kB file with a 100 kB cap → 413.
    let tmp = TempDir::new().unwrap();
    let src_root = tmp.path().join("src");
    let dst_root = tmp.path().join("dst");
    tokio::fs::create_dir_all(&src_root).await.unwrap();
    tokio::fs::create_dir_all(&dst_root).await.unwrap();
    let payload = vec![7u8; 200_000];
    tokio::fs::write(src_root.join("big.bin"), &payload)
        .await
        .unwrap();

    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: src_root.clone(),
        })
        .unwrap(),
    );
    let fake_s3: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: dst_root.clone(),
        })
        .unwrap(),
    );
    let mut cfg = AppConfig::default();
    cfg.limits.max_response_bytes = 100_000;
    let state = AppState {
        backends: Backends {
            fs,
            s3: Some(fake_s3),
        },
        config: Arc::new(cfg),
    };
    let app = build_router(state);
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "big.bin",
        "destinationStorageType": "S3",
        "destinationFilePath": "big-out.bin"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    // The stream copy errors partway through. FerryError::Io wraps
    // the "size cap" io::Error emitted by LimitedReader — status is
    // 500 (Io) rather than 413 because the cap fires inside the
    // stream, not at the axum layer. Documented here so a future
    // change that promotes it to 413 is visible.
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "size cap should fail the copy"
    );
}

// ----- inter-service auth (F-FF-1 / F-FF-2 / F-FF-3) --------------

fn app_state_with_auth(root: &std::path::Path, token: &str) -> AppState {
    let fs: BackendRef = Arc::new(
        FsBackend::new(&FsConfig {
            data_directory: root.to_path_buf(),
        })
        .unwrap(),
    );
    let cfg = AppConfig {
        security: SecurityConfig {
            inter_service_token: Some(token.to_string()),
            trust_network: false,
            admin_enabled: false,
        },
        ..AppConfig::default()
    };
    AppState {
        backends: Backends { fs, s3: None },
        config: Arc::new(cfg),
    }
}

#[tokio::test]
async fn auth_off_by_default_list_still_open() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "no token configured → gate is off"
    );
}

#[tokio::test]
async fn auth_required_list_rejects_missing_bearer() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_auth(tmp.path(), "supersecret-token"));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let json = parse_json(resp.into_body()).await;
    assert_eq!(json.get("error").unwrap(), "unauthorized");
}

#[tokio::test]
async fn auth_required_list_rejects_wrong_bearer() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_auth(tmp.path(), "supersecret-token"));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_required_list_accepts_correct_bearer() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_auth(tmp.path(), "supersecret-token"));
    let resp = app
        .oneshot(
            Request::get("/v1/files?type=FS")
                .header("Authorization", "Bearer supersecret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_required_copy_rejects_missing_bearer() {
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_auth(tmp.path(), "supersecret-token"));
    let body = json!({
        "sourceStorageType": "FS",
        "sourceFilePath": "a.txt",
        "destinationStorageType": "S3",
        "destinationFilePath": "b.txt"
    });
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn method_not_allowed_on_known_route_returns_405() {
    // T-18: axum's default is 404 for a route defined with only `get()`
    // when accessed via POST. RFC 7231 §6.5.5 requires 405 when the
    // path is known but the method isn't. Probes each known route with
    // an inappropriate method and asserts 405 — plus an `allow`
    // response header naming the valid methods (RFC MUST).
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    // (path, wrong-method, expected-allow-substring)
    let cases: &[(&str, axum::http::Method, &str)] = &[
        ("/", axum::http::Method::POST, "GET"),
        ("/health", axum::http::Method::POST, "GET"),
        ("/health", axum::http::Method::DELETE, "GET"),
        ("/v1/files", axum::http::Method::POST, "GET"),
        ("/v1/files", axum::http::Method::DELETE, "GET"),
        ("/v1/files/copy", axum::http::Method::GET, "POST"),
        ("/v1/files/copy", axum::http::Method::DELETE, "POST"),
    ];
    for (path, method, allow_needle) in cases {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method.clone())
                    .uri(*path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "expected 405 for {method} {path}, got {}",
            resp.status()
        );
        let allow = resp
            .headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            allow.contains(allow_needle),
            "405 for {method} {path} missing `allow: {allow_needle}` header (got {allow:?})"
        );
    }
}

#[tokio::test]
async fn unknown_route_still_returns_404() {
    // T-18 sanity check: only KNOWN routes get 405. A path the router
    // never heard of must still 404 — otherwise `/anything` -> 405 would
    // itself be a recon signal (attacker learns "I found a real path").
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    for path in ["/nope", "/v1/files/there_is_no_such_thing", "/admin"] {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "unknown path {path} must 404, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn auth_required_health_and_root_still_open() {
    // F-FF-1/F-FF-2: `/`, `/health` MUST remain public regardless of
    // bearer-token configuration — liveness probes and load balancers
    // rely on them.
    // F-FF-3 (T-6): `/api` moved OUT of "always public" — it now
    // defaults to 404 (admin off). The separate
    // `openapi_returns_404_when_admin_disabled_by_default` test
    // covers that gate; this test asserts only the never-gated pair.
    let tmp = TempDir::new().unwrap();
    let app_state = app_state_with_auth(tmp.path(), "supersecret-token");
    let app = build_router(app_state);
    for path in ["/", "/health"] {
        let resp = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "public route {path} must not require bearer"
        );
    }
    // `/api` returns 404 by default (admin off) — NOT 401 (would leak
    // the admin gate's existence to an unauth caller).
    let resp = app
        .oneshot(Request::get("/api").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "`/api` must 404 when admin gate is off, not 401"
    );
}

#[tokio::test]
async fn error_message_body_bounded_regardless_of_query_size() {
    // AP-6 / T-11: an attacker sending a huge query value used to
    // surface as a proportionally huge error message
    // (`Failed to deserialize query string: unknown variant
    // <4KB of A's>`). Enforce that the response body's `message`
    // field is capped so amplification isn't a valid signal.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let huge: String = "A".repeat(4096);
    let uri = format!("/v1/files?type={huge}");
    let resp = app
        .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    // Status is a client error — either 400 (bad_query) or 422 (deserialize).
    assert!(
        resp.status().is_client_error(),
        "got status {}",
        resp.status()
    );
    let json = parse_json(resp.into_body()).await;
    let msg = json.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let msg_chars = msg.chars().count();
    // Cap is 256 chars (see error::MAX_USER_MESSAGE_LEN). Enforce
    // strictly — a leaky refactor here re-enables the amplifier.
    assert!(
        msg_chars <= 256,
        "response `message` exceeds 256-char cap: {} chars, body {:?}",
        msg_chars,
        msg
    );
    // The amplification signal is that the response body grows with the
    // input size. Directly assert: response length must not scale with
    // the 4KB input.
    assert!(
        msg.chars().filter(|c| *c == 'A').count() < 500,
        "clip failed: {} `A`s survived in message {:?}",
        msg.chars().filter(|c| *c == 'A').count(),
        msg
    );
}

#[tokio::test]
async fn malformed_json_body_error_message_is_clipped() {
    // AP-6 / T-11: serde_json's parse-error message can embed a chunk
    // of the offending payload verbatim. Send a 4 KB junk body with a
    // JSON content-type; the response message must stay bounded.
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
    let junk: String = "Z".repeat(4096);
    let resp = app
        .oneshot(
            Request::post("/v1/files/copy")
                .header("content-type", "application/json")
                .body(Body::from(junk))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "got status {}",
        resp.status()
    );
    let json = parse_json(resp.into_body()).await;
    let msg = json.get("message").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.chars().count() <= 256,
        "malformed-JSON message exceeds cap: {} chars, body {:?}",
        msg.chars().count(),
        msg
    );
    assert!(
        msg.chars().filter(|c| *c == 'Z').count() < 500,
        "clip failed on JSON parse error: {} Z's in {:?}",
        msg.chars().filter(|c| *c == 'Z').count(),
        msg
    );
}
