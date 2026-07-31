# FileFerry design

## 1. Purpose

FileFerry brokers file transfers between a local filesystem and an
S3-compatible object store, over a small HTTP surface. It is a
Rust re-implementation of the JVM/NestJS
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).

## 2. Why re-implement

Two motivations:

1. **Uniform Buerostack Rust toolchain**. Sibling components
   (Ruuter-on-Rust, XTR-on-Rust) run as small hardened Rust
   containers. Keeping FileFerry in the same shape (same CI, same
   Dockerfile pattern, same supply-chain checks) reduces per-
   component operator overhead.
2. **Smaller attack surface**. NestJS + AWS SDK v3 pulls in ~500
   npm packages transitively; the FileFerry crate graph is under
   200 crates and every one is subject to `cargo audit` and
   `cargo deny check`. No admin JS bundle at runtime. Non-root
   uid 1000 in a distroless-adjacent Debian slim.

Non-motivations:

- New feature parity with any advanced-feature list in RESEARCH.md
  (auth, virus scanning, versioning, deduplication, etc.) — that
  scope is explicitly deferred to future major releases.

## 3. Domain surface (what FileFerry does)

| Endpoint            | Behaviour                                                                                                    |
|---------------------|--------------------------------------------------------------------------------------------------------------|
| `GET /`             | Returns `{"data": "FileFerry"}`. Kept for S3-Ferry parity; a health-check gateway checking for it still works. |
| `GET /health`       | Returns `{"status": "ok"}` if the process is up. Wired into the container's `HEALTHCHECK`.                    |
| `GET /api`          | Static OpenAPI 3.1 summary. `documentation_enabled: false` hides it (404).                                    |
| `GET /v1/files?type={FS,S3}` | Lists root-level files in the selected backend. Nested entries filtered out (S3-Ferry parity).      |
| `POST /v1/files/copy` | Stream-copies one file from `sourceStorageType`/`sourceFilePath` to `destinationStorageType`/`destinationFilePath`. Enforces source ≠ destination (400). |

The copy request body shape is byte-identical to S3-Ferry's, so
existing clients migrate with a single URL swap.

## 4. Architecture

```
                             +------------------+
                             |   axum router    |
                             |  src/router.rs   |
                             +--------+---------+
                                      |
                +---------------------+---------------------+
                |                                           |
       list / open_read / write_all               list / open_read / write_all
                |                                           |
      +---------v---------+                       +---------v---------+
      |    FsBackend      |                       |    S3Backend      |
      | src/backend/fs.rs |                       | src/backend/s3.rs |
      +---------+---------+                       +---------+---------+
                |                                           |
       tokio::fs                                    aws-sdk-s3 client
     (local filesystem)                             (S3 / MinIO / R2)
```

Every backend implements the `Backend` trait (`src/backend/mod.rs`).
`stream_copy` reads from a source backend into a `LimitedReader`
wrapper (caps total bytes at `limits.max_response_bytes`) and
writes to the destination backend. Adding a third backend
(Azure, GCS, WebDAV) is one file + a `Backends::pick` arm.

## 5. What FileFerry PRESERVES from S3-Ferry

- Endpoint paths + verbs + response envelopes
- `{"data": [...], "meta": {"count": N}}` list-response shape
- Path validation regex `^[0-9a-zA-Z-._/]+$` + null-byte rejection
  + traversal rejection
- `sourceStorageType == destinationStorageType` → error (prevents
  no-op copies)
- Root-level-only `list` (nested keys filtered out)
- `force_path_style(true)` on the S3 client
- S3 `bucket_path` acts as a key prefix
- Environment-variable-based credentials
- No auth built into the process — operator terminates upstream

## 6. What FileFerry CHANGES from S3-Ferry

Behaviour changes are enumerated so downstream operators know
exactly what to expect on the wire.

| Area                       | S3-Ferry                                                | FileFerry                                                                     |
|----------------------------|---------------------------------------------------------|-------------------------------------------------------------------------------|
| Banner text                | `{"data": "S3 Ferry"}`                                  | `{"data": "FileFerry"}`                                                       |
| Health check               | *(none)*                                                | `GET /health` → `{"status": "ok"}`                                            |
| API discovery              | Interactive Swagger at `/documentation` (opt-in)         | Static OpenAPI 3.1 at `/api` (opt-out via `documentation_enabled: false`)     |
| List S3 with `bucket_path` | Lists whole bucket, filters keys with `/` — silently ignores `bucket_path` | Lists under `bucket_path`, filters remainders with `/`. Consistent with copy. |
| Credentials in config      | Plain-text env-var expansion in `.env` files            | YAML config names the env var; startup fails hard if env var is unset         |
| Path validator             | class-validator decorators, regex + null check + `path.normalize` for traversal | Segment-wise `..` check + null check + whitelist regex, no path normalisation |
| Size caps                  | No caps                                                 | `max_request_bytes` (inbound HTTP body), `max_response_bytes` (per-transfer)  |
| Timeouts                   | Default express/undici                                  | `request_timeout_secs` enforced by `tower-http`                               |
| S3 write path              | `fs.createReadStream(path).pipe(s3.putObject)`          | Buffer to `NamedTempFile`, then `ByteStream::read_from().path(...)` — needs Content-Length upfront. Trade-off: 2× local disk I/O on the writer. (Task 002 planned to switch to true streaming.) |

## 7. Trade-offs and known limitations

- **S3 upload buffering.** The S3 SDK's `PutObject` requires a
  length-known body. The 0.1.0-alpha.1 impl buffers uploads to a
  temp file. This is correct but incurs 2× local disk I/O on the
  writer. Task 002 in the backlog switches to
  `SdkBody::from_body_1_x` for true streaming.
- **No multipart upload.** A single PUT caps at 5 GiB per S3's
  own limits. Larger objects fail with an S3 error. Task 003 in
  the backlog adds multipart-upload for objects > 100 MiB.
- **No S3 live tests in the default CI matrix.** The trait seam
  is exercised by substituting `FsBackend` for `S3Backend` in
  integration tests; a wire-level LocalStack test lives behind a
  `--features localstack-tests` gate (task 004 wires it into CI
  as a separate job).
- **`cors_origin` is not enforced yet.** The field exists to
  mirror S3-Ferry's config shape but no `tower-http::cors` layer
  reads it. Task 005 adds it.

## 8. Roadmap to v1.0

- **v0.1.x** (current line) — S3-Ferry parity + hardening + docs.
- **v0.2.x** — Real streaming S3 upload (task 002) + multipart
  (task 003) + live-S3 CI job (task 004) + CORS enforcement
  (task 005).
- **v0.3.x** — Nested directory support in `list`, opt-in with a
  `?recursive=true` query. Backward compatible.
- **v0.4.x** — Additional backends behind cargo features
  (`gcs-backend`, `azure-backend`, `webdav-backend`). Pluggable
  via `Backends` registry.
- **v1.0.0** — Public-API-stable release on `main`. Cuts only
  after downstream integrations have exercised the alpha line for
  at least one release cycle.

## 9. Rejected alternatives

- **Reuse the S3-Ferry TypeScript code as-is.** Rejected —
  breaks the "one build/CI/container toolchain" property of
  the Buerostack Rust family.
- **Use `aws-sdk-s3::primitives::ByteStream::from_body_1_x`
  in 0.1.0-alpha.1 for streaming upload.** Rejected as scope
  overreach for the first alpha. Feature-parity + hardening first;
  performance optimisation next.
- **Hide the FS backend behind a feature flag.** Rejected — the
  FS backend is what makes FileFerry a *proxy*; without it there
  is no source-of-truth for `local` files, and S3-Ferry
  compatibility breaks.
