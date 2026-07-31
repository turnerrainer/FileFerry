# 002 — True-streaming S3 upload (no temp file)

## Filed
2026-07-29 — deferred from the v0.1.0-alpha.1 MVP scope. See
[`../../docs/DESIGN.md`](../../docs/DESIGN.md) §7.

## Severity
Medium. The current impl works; it just does 2× local disk I/O on
the writer host. Becomes High only when operators start pushing
multi-GB objects on a regular basis.

## Motivation
`aws-sdk-s3`'s `PutObject` requires a length-known body. The
0.1.0-alpha.1 `S3Backend::write_all` buffers the reader to a
`NamedTempFile`, then hands the SDK a `ByteStream::read_from()`
against the temp file path. Correct but wasteful: every byte
touches local disk twice (once written by us, once read by
the SDK's mmap).

## Fix / Design
Replace the temp-file dance with a direct `SdkBody::from_body_1_x`
wrap:

```
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::byte_stream::ByteStream;
use http_body_util::StreamBody;
use hyper::body::Frame;
use tokio_util::io::ReaderStream;

let stream = ReaderStream::new(reader)
    .map(|res| res.map(Frame::data));
let body = StreamBody::new(stream);
let byte_stream = ByteStream::new(SdkBody::from_body_1_x(body));
```

Add dependencies: `http-body-util`, `hyper` (already transitive
via axum), `bytes` (already present).

For objects large enough to warrant multipart, defer to task 003.
This task is just the streaming replacement for the single-PUT
path.

## Acceptance
- [ ] `src/backend/s3.rs::write_all` no longer creates a temp
  file; passes reader → `SdkBody::from_body_1_x` → PutObject
  directly
- [ ] `tempfile` removed from `[dependencies]` (still used in
  `[dev-dependencies]` for tests)
- [ ] New integration test that copies a 10 MB payload
  FS → S3 (via LocalStack — depends on task 004) and asserts the
  destination bytes match
- [ ] Existing `copy_happy_path_transfers_bytes` still passes
- [ ] `cargo bench` (new) shows write throughput improvement on
  a 100 MB payload

## Estimated effort
1-2 days. Most time is in the `http_body::Body` trait dance and
the type-plumbing to match `SdkBody::from_body_1_x`.

## Dependencies
- Task 004 (LocalStack CI job) preferred so the perf claim can be
  verified in CI. If task 004 lags, ship this task with unit
  tests only and defer the perf assertion.

## Non-scope
- Multipart-upload (task 003)
- Retry / resume on partial upload failure (future)
- Progress reporting to the caller (future — needs SSE/WebSocket
  scope decision)

## Risks
- `SdkBody::from_body_1_x` may impose trait bounds
  (`Send + Sync + 'static`) that don't hold on our current
  `ByteReader` type alias. Mitigation: widen the type alias if
  needed; the change is source-compatible with all existing
  callers.
