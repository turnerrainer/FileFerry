# Changelog

All notable changes to FileFerry will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Compat corpus** (`compat/s3-ferry/config/*.env`) — verbatim
  copies of S3-Ferry's shipped env files, exercised by
  `tests/compat_corpus.rs` to assert every S3-Ferry field name
  is on a human-reviewed coverage list before it can pass CI.
- **Boot diagnostic pass** (`src/diagnose.rs`) — emits
  `tracing::warn!` at startup for every accepted-but-unwired
  config field set to a non-default value, naming the field and
  the intended behaviour. Currently detects `cors_origin`.

### Corrected

- Overclaim in the `[0.1.0-alpha.1]` intro: `POST /v1/files/copy`
  actually returns `204`, not the `201` S3-Ferry emits — this
  divergence was not previously documented. See the "Known
  coverage gaps" section of `[0.1.0-alpha.1]` below.

## [0.1.0-alpha.1] - 2026-07-31

First alpha release. Rust re-implementation of the JVM-based
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).
Preserves S3-Ferry's HTTP surface (same four endpoints, same
response envelopes on the happy path, same path-validation rules,
same "no nested directories" `list` semantics), with the
divergences listed in the "Known coverage gaps" subsection
below.

### Added — HTTP surface

- `GET /` — service banner
  (`{"data": "FileFerry"}`, was `{"data": "S3 Ferry"}`)
- `GET /health` — liveness probe
  (`{"status": "ok"}`) — **new** vs S3-Ferry; needed for
  the `HEALTHCHECK` clause in `docker-compose.yml`
- `GET /api` — static OpenAPI 3.1 summary — **new** vs S3-Ferry's
  interactive Swagger UI at `/documentation`. Static output is
  smaller, faster, and doesn't need an admin-UI JS bundle at runtime
- `GET /v1/files?type={FS,S3}` — list root-level files
- `POST /v1/files/copy` — stream-copy a file between backends

### Added — Storage abstraction

- `Backend` trait in `src/backend/mod.rs` with two impls:
  - `FsBackend` (`src/backend/fs.rs`) — local filesystem rooted
    at `fs.data_directory`
  - `S3Backend` (`src/backend/s3.rs`) — `aws-sdk-s3` 1.x with
    `force_path_style(true)` for MinIO/LocalStack compatibility
- `stream_copy()` orchestrates cross-backend transfers with a
  `LimitedReader` wrapper enforcing `limits.max_response_bytes`

### Added — Config

- YAML config at `./fileferry.yaml` (search order:
  `--config` > `FILEFERRY_CONFIG` env > `./fileferry.yaml` >
  built-in defaults)
- **Credentials never in the config file** — S3 access/secret
  keys come from named env vars (`access_key_id_env`,
  `secret_access_key_env`); startup fails hard if a referenced
  env var is unset
- `deny_unknown_fields` on every YAML shape — a config typo like
  `porrt: 9000` fails to parse rather than silently applying
  the default
- Resource ceilings: `max_request_bytes` (32 MiB),
  `max_response_bytes` (5 GiB), `request_timeout_secs` (300s)

### Added — Path validation

- Reject empty paths, null bytes, characters outside
  `[0-9 a-z A-Z - . _ /]`, and `..` segments
- Segment-wise `..` check catches `foo/..` and `.` corner cases
  that a substring match on `../` would miss

### Added — Container image

- Multi-stage `rust:1.94-slim-bookworm` → `debian:bookworm-slim`
- Non-root user (uid 1000), `tini` as PID 1, `libssl3` + `curl`
  + `ca-certificates` runtime deps only
- `HEALTHCHECK` hits `/health` every 30s; container marked
  unhealthy after 3 consecutive failures
- Ships `fileferry.yaml` + empty `/app/data` baked in — first
  boot works with no bind mounts

### Added — CI / supply chain

- `.github/workflows/tests.yml` — matrix on
  `ubuntu-latest` + `ubuntu-24.04-arm`, runs `cargo fmt --check`
  + `cargo clippy --all-targets -- -D warnings` + `cargo build
  --release` + `cargo test --release --no-fail-fast`, plus
  `mdbook build book` with linkcheck
- `.github/workflows/security.yml` — `cargo audit --deny
  warnings` + `cargo deny check all` on push, PR, and daily cron
- `.github/workflows/publish.yml` — multi-arch (`linux/amd64` +
  `linux/arm64`) Docker Hub + GHCR publish on release tag or
  `workflow_dispatch`. Cosign keyless signing, SPDX SBOM,
  in-toto provenance, Trivy vulnerability scan gates signing,
  per-arch smoke test
- `.github/workflows/docs.yml` — mdBook + linkcheck build,
  GitHub Pages deploy on push to `main` or `dev`

### Added — Documentation

- 5 mdBook chapters: introduction, getting-started, configuration,
  failure-modes, reference/changelog. Zero duplication between
  chapters — every snippet appears in exactly one place
- `docs/DESIGN.md` — the domain design derived from a direct read
  of the original `buerokratt/S3-Ferry`; documents the JVM
  S3-Ferry's public surface, what FileFerry preserves, what it
  changes, and the reasoning
- `STANDARDS.md` — pointer at `../DEV-REQUIREMENTS.md` +
  FileFerry-specific extras (MSRV bump rationale, backend
  abstraction pattern, size-cap semantics)
- `SECURITY.md` — private disclosure recipe, response SLA,
  supported versions, CI supply-chain posture inventory

### Deviations from DEV-REQUIREMENTS

- **MSRV bumped from `1.88` to `1.94`.** Reason: `aws-sdk-s3`
  1.140 and its transitive `aws-*` deps require `rustc >= 1.94.1`.
  Cargo.toml, Dockerfile builder tag, and CI matrix all set
  to 1.94. STANDARDS.md §2 records this.

### Known coverage gaps

Pre-release; ships with the following known gaps against S3-Ferry
parity. Consumers running FileFerry against an S3-Ferry-derived
client should know what is and isn't reproduced:

- **`cors_origin` accepted but not enforced.** The field parses at
  boot but no `CorsLayer` is mounted; responses carry no CORS
  headers. Operators depending on CORS must front FileFerry with
  a CORS-aware reverse proxy until `v0.1.0-alpha.2`. The boot
  diagnostic warns when a non-default `cors_origin` is set.
- **`POST /v1/files/copy` returns `204`, not `201`.** Any client
  asserting `status === 201` must accept `204` instead.
- **Error response body shape changed.** In-handler errors emit
  `{"error": "<code>", "message": "<text>"}` — different from
  NestJS's `{"message": ..., "error": ..., "statusCode": ...}`.
  Framework-level rejections (JSON parse, query parse, `404`
  for unknown paths, `405` for wrong methods, `415` for wrong
  `Content-Type`) emit axum's default plain-text or empty
  bodies, not the FileFerry shape.
- **`lastModified` timestamp precision.** S3-Ferry emits
  `YYYY-MM-DDTHH:MM:SS.mmmZ` (milliseconds); FileFerry emits
  `YYYY-MM-DDTHH:MM:SSZ` (seconds).
- **Wrong HTTP method on a known path** returns
  `405 Method Not Allowed` with an `allow:` header, not
  S3-Ferry's `404 Cannot X /path`.
- **Log-string parity broken.** Structured `tracing` output
  replaces S3-Ferry's `Request: {…}` / `Response: {…}` /
  `Listing files failed: <stack>` / `Copying files failed:
  <stack>` literals. Log-grep patterns must migrate to
  field matching.
- **S3 listing uses `bucket_path` as prefix.** Diverges from
  S3-Ferry, which ignores `S3_DATA_BUCKET_PATH` on list. The
  FileFerry behaviour matches copy semantics (writes and lists
  scoped by the same prefix).
- **S3 upload buffers to a temp file.** Streaming regression:
  aws-sdk-s3 1.x rejects unsized bodies, so a `NamedTempFile`
  is used to supply a known `Content-Length`. Operators
  uploading multi-GiB files need `TMPDIR` on real disk.
  True streaming is planned for `v0.2.0`.
- **No cross-implementation reproduction fixtures.** Behavioural
  claims are verified by code-read and by the FileFerry test
  set only; no S3-Ferry-vs-FileFerry side-by-side fixture is
  run in CI. LocalStack-based comparison is planned for
  `v0.1.0-alpha.2`.
- **Unknown-field tolerance on JSON bodies.** Matches S3-Ferry's
  silent-accept behaviour for the alpha window; will tighten
  to `400` with a specific error code in `v0.2.0`.

[Unreleased]: https://github.com/turnerrainer/fileferry/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.0-alpha.1
