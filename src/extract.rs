//! Typed extractors that translate axum's default rejection responses
//! (bare `text/plain`) into the fleet-wide structured JSON error shape
//! `{error, message}` via `FerryError`.
//!
//! Reference: h2ck.me/FileFerry/v1/BREAK-TESTS/RUNTIME-FINDINGS.md FN2.

use async_trait::async_trait;
use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequest, FromRequestParts, Json, Query, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::{clip_user_message, FerryError};

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
        match Json::<T>::from_request(req, state).await {
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
