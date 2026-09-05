use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde_json::json;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::backend::{stream_copy, Backends, ListOptions, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT};
use crate::config::AppConfig;
use crate::error::FerryError;
use crate::model::{CopyFileRequest, ListFilesMeta, ListFilesQuery, ListFilesResponse};
use crate::validate::validate_path;

#[derive(Clone)]
pub struct AppState {
    pub backends: Backends,
    pub config: Arc<AppConfig>,
}

pub fn build_router(state: AppState) -> Router {
    let max_body: usize = state
        .config
        .limits
        .max_request_bytes
        .try_into()
        .unwrap_or(usize::MAX);
    let timeout = Duration::from_secs(state.config.limits.request_timeout_secs);

    let api = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/api", get(openapi))
        .route("/v1/files", get(list_files))
        .route("/v1/files/copy", post(copy_file));

    api.layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            // Inbound body cap. Files themselves are transferred
            // backend↔backend inside the handler — the HTTP body
            // only carries request metadata (JSON) — so the cap
            // can be relatively small.
            .layer(RequestBodyLimitLayer::new(max_body))
            .layer(tower_http::timeout::TimeoutLayer::new(timeout)),
    )
    .with_state(state)
}

async fn root() -> impl IntoResponse {
    Json(json!({ "data": "FileFerry" }))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Static hand-maintained OpenAPI 3.1 summary. Small enough that a
/// dependency (`utoipa` / `okapi`) is overkill; large enough that
/// clients want *something* discoverable. Refresh in-lockstep with
/// the route table above.
async fn openapi(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.documentation_enabled {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error":"documentation_disabled"})),
        )
            .into_response();
    }
    let doc = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "FileFerry",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Generic file-transfer proxy. See docs at /introduction.md in the book."
        },
        "paths": {
            "/": { "get": { "summary": "Service banner", "responses": {"200":{"description":"ok"}} } },
            "/health": { "get": { "summary": "Liveness", "responses": {"200":{"description":"ok"}} } },
            "/v1/files": {
                "get": {
                    "summary": "List root-level files in a backend",
                    "parameters": [
                        {"name":"type","in":"query","required":true,
                         "schema":{"type":"string","enum":["FS","S3"]}},
                        {"name":"limit","in":"query","required":false,
                         "schema":{"type":"integer","minimum":1,"maximum":MAX_LIST_LIMIT}},
                        {"name":"startAfter","in":"query","required":false,
                         "schema":{"type":"string"}}
                    ],
                    "responses": {
                        "200":{"description":"file list (see meta.nextCursor for pagination)"},
                        "413":{"description":"limit exceeds server cap"}
                    }
                }
            },
            "/v1/files/copy": {
                "post": {
                    "summary": "Copy a file between backends",
                    "requestBody": {
                        "required": true,
                        "content": {"application/json": {"schema": {
                            "type": "object",
                            "required": ["sourceStorageType","sourceFilePath","destinationStorageType","destinationFilePath"],
                            "properties": {
                                "sourceStorageType": {"type":"string","enum":["FS","S3"]},
                                "sourceFilePath": {"type":"string"},
                                "destinationStorageType": {"type":"string","enum":["FS","S3"]},
                                "destinationFilePath": {"type":"string"}
                            }
                        }}}
                    },
                    "responses": {
                        "204": {"description":"copied"},
                        "400": {"description":"invalid path or same source/destination"},
                        "404": {"description":"source not found"},
                        "502": {"description":"upstream storage failure"}
                    }
                }
            }
        }
    });
    Json(doc).into_response()
}

async fn list_files(
    State(state): State<AppState>,
    Query(q): Query<ListFilesQuery>,
) -> Result<Json<ListFilesResponse>, FerryError> {
    let backend = state.backends.pick(q.storage_type)?;
    // F4: cap the client-requested `limit`. Values above MAX_LIST_LIMIT
    // are refused before we touch the backend so a caller can't ask us
    // to allocate an unbounded response. Zero is coerced to the default
    // to avoid a "silently return no results" footgun.
    let limit = match q.limit {
        Some(0) | None => DEFAULT_LIST_LIMIT,
        Some(n) if n > MAX_LIST_LIMIT => {
            return Err(FerryError::ListLimitTooLarge {
                cap: MAX_LIST_LIMIT,
            });
        }
        Some(n) => n,
    };
    // F4: sanity-check `startAfter` against the same validator we use
    // for copy paths — no null bytes, no traversal fragments, no
    // exotic charset. Prevents a caller from smuggling weird input
    // into the S3 list request.
    if let Some(cursor) = q.start_after.as_deref() {
        validate_path(cursor)?;
    }
    let files = backend
        .list(ListOptions {
            limit: Some(limit),
            start_after: q.start_after.clone(),
        })
        .await?;
    let count = files.len();
    // F4: emit a cursor only when the page filled up — a short page
    // signals the tail of the listing.
    let next_cursor = if count == limit {
        files.last().map(|e| e.name.clone())
    } else {
        None
    };
    Ok(Json(ListFilesResponse {
        data: files,
        meta: ListFilesMeta { count, next_cursor },
    }))
}

async fn copy_file(
    State(state): State<AppState>,
    Json(req): Json<CopyFileRequest>,
) -> Result<StatusCode, FerryError> {
    if req.source_storage_type == req.destination_storage_type {
        return Err(FerryError::SameStorageType);
    }
    validate_path(&req.source_file_path)?;
    validate_path(&req.destination_file_path)?;

    let src = state.backends.pick(req.source_storage_type)?;
    let dst = state.backends.pick(req.destination_storage_type)?;

    stream_copy(
        src,
        &req.source_file_path,
        dst,
        &req.destination_file_path,
        state.config.limits.max_response_bytes,
        Duration::from_secs(state.config.limits.copy_inactivity_secs),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
