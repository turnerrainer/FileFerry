//! Typed extractors that translate axum's default rejection responses
//! (bare `text/plain`) into the fleet-wide structured JSON error shape
//! `{error, message}` via `FerryError`.
//!
//! Reference: h2ck.me/FileFerry/v1/BREAK-TESTS/RUNTIME-FINDINGS.md FN2.
//!
//! T-17: `TypedJson::from_request` also wraps the inner Json extractor
//! in a `tokio::time::timeout` so a slow-drip HTTP body can't hold a
//! request open for up to `limits.request_timeout_secs` (5 min
//! default). The narrower cap defends the accept-queue: a hostile
//! peer that opens a POST and dribbles 1 byte every 60 s used to
//! consume a full connection slot for the entire request-timeout
//! window. See `BODY_READ_TIMEOUT`.

use std::time::Duration;

use async_trait::async_trait;
use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequest, FromRequestParts, Json, Query, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::{clip_user_message, FerryError};

/// T-17: hard cap on the time `TypedJson` will wait for the caller
/// to finish sending a JSON body. Kept as a const (not a config
/// field) because the JSON bodies FileFerry accepts are small
/// (`CopyFileRequest` — a handful of strings, well under 1 KiB in
/// practice); 30 s is orders of magnitude more than a healthy peer
/// ever needs. Independent of `limits.request_timeout_secs` (which
/// covers the whole request including the backend copy) so a slow
/// body can't hold a connection slot until the total-request cap
/// eventually fires.
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Wrapper around `axum::extract::Query<T>` whose rejection is a
/// `FerryError::BadQuery` — surfaces as structured 400 JSON.
///
/// AP-6 / T-11: the axum rejection text embeds the offending query
/// value verbatim (e.g. `unknown variant `UNKNOWN_LONG_VALUE...`,
/// expected `FS` or `S3``). Clip the inner message at 256 chars
/// before stashing it in `BadQuery` so the response body stays
/// bounded regardless of caller input size. The `IntoResponse` impl
/// re-applies the same clip; belt-and-braces so refactors on either
/// side don't lose the guarantee.
pub struct TypedQuery<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for TypedQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = FerryError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(v)) => Ok(TypedQuery(v)),
            Err(QueryRejection::FailedToDeserializeQueryString(err)) => {
                Err(FerryError::BadQuery(clip_user_message(&err.to_string())))
            }
            Err(other) => Err(FerryError::BadQuery(clip_user_message(&other.to_string()))),
        }
    }
}

/// Wrapper around `axum::extract::Json<T>` whose rejection is either
/// `FerryError::BadBody` (400) or `FerryError::BodyTooLarge` (413).
pub struct TypedJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for TypedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = FerryError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // T-17: hard cap on body-read time. `Json::from_request`
        // reads the entire body before deserialising; without this
        // guard a slow-drip peer could hold the connection open for
        // up to `limits.request_timeout_secs` (5 min default). The
        // narrower `BODY_READ_TIMEOUT` targets the accept-queue
        // exhaustion angle specifically.
        let inner = Json::<T>::from_request(req, state);
        let result = match tokio::time::timeout(BODY_READ_TIMEOUT, inner).await {
            Ok(res) => res,
            Err(_elapsed) => return Err(FerryError::BodyReadTimeout),
        };
        match result {
            Ok(Json(v)) => Ok(TypedJson(v)),
            Err(rej) => {
                // Body-limit misses surface with the composite
                // `IntoResponse` status PAYLOAD_TOO_LARGE (via
                // `axum_core::extract::rejection::LengthLimitError`
                // #[status = PAYLOAD_TOO_LARGE]). Everything else is a
                // client-side JSON shape/content-type error.
                let status = rej.status();
                let msg = rej.body_text();
                if status == StatusCode::PAYLOAD_TOO_LARGE {
                    Err(FerryError::BodyTooLarge { cap: 0 })
                } else {
                    Err(FerryError::BadBody(clip_user_message(&msg)))
                }
            }
        }
    }
}
