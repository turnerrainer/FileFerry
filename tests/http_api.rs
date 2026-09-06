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
use fileferry::config::{AppConfig, FsConfig};
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
    let tmp = TempDir::new().unwrap();
    let app = build_router(app_state_with_fs_only(tmp.path()));
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
