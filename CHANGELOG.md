# Changelog

All notable changes to FileFerry will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3-alpha] - 2026-09-06

Third alpha. Security hardening pass driven by the h2ck.me v1
audit (`AUDIT.md`, `FIX-KIT.md`). Ships fixes for the two
CRITICAL, two HIGH and two MEDIUM findings, all LOW findings, and
documents the remaining operational posture. Versioning switches
to `MAJOR.MINOR.PATCH-alpha` (bare `-alpha` suffix); this release
supersedes `0.1.0-alpha.2`.

### Security

- **F1 — FS-backend symlink escape (CRITICAL).** `resolve()` now
  rejects any path whose final component is a symlink (flat
  policy, no in-root carve-out). `open_read` and `write_all` open
  files with `O_NOFOLLOW`, closing the TOCTOU window between the
  pre-check and the open. `list()` uses `symlink_metadata()` so
  planted symlinks no longer leak their target's size / mtime.
- **F2 — `S3Config` derived Debug leaked credentials (CRITICAL).**
  Replaced with a hand-written `impl fmt::Debug` that renders
  `access_key_id` and `secret_access_key` as `***REDACTED***`.
  `{:?}` on `AppConfig` inherits the masked inner impl.
- **F3 — Slow-drip transfer DoS (HIGH).** `stream_copy` wraps the
  source reader in a per-poll `TimeoutReader` gated by
  `limits.copy_inactivity_secs` (default 30). Stalled peers now
  abort within the budget instead of holding a request slot for
  the full `request_timeout_secs`.
- **F4 — Unpaginated list DoS (HIGH).** `GET /v1/files` accepts
  `?limit=` (default 1 000, cap 10 000, over-cap → 413) and
  `?startAfter=<name>`. Responses gain `meta.nextCursor` when a
  page is full. The FS backend sorts by name for stable
  pagination; the S3 backend uses server-side continuation
  tokens. `startAfter` is validated the same way as file paths.
- **F5 — `bucket_path` traversal (MEDIUM).** Values containing
  `..` or a leading `/` fail the boot validation with a named
  error before the server binds.
- **F6 — `cors_origin` was parsed but unimplemented (MEDIUM).**
  Setting a non-empty value is now a hard boot failure. Terminate
  CORS at a reverse proxy; full CORS wiring will land in a later
  release with a proper config surface.
- **F8 — Error echoed user-supplied path (LOW).** The escape
  branch of `resolve()` returns a fixed string
  (`resolved path escapes fs root`). The full input is still
  logged via `tracing::warn!` for operator debugging.
- **F7 / F9 / F10 — Documented posture.** F7 (response-cap /
  compressed bytes) is a preserved invariant — FileFerry does not
  enable transparent decompression on any HTTP client. F9 (S3
  tempfile in shared `$TMPDIR`) is mitigated by the shipped
  compose file's private `/tmp`; chunked `PutObject` is v0.2
  backlog. F10 (circular-symlink DoS) is mooted by F1.

### Changed

- **Versioning scheme.** Alphas now increment the PATCH digit
  with a bare `-alpha` suffix (`0.1.1-alpha`, `0.1.2-alpha`,
  `0.1.3-alpha`, ...) instead of the `-alpha.N` dot-counter form.
  `0.1.0-alpha.2` corresponds to what the new scheme would call
  `0.1.2-alpha`; the next release is `0.1.3-alpha`.
- **Publish workflow.** `publish.yml` now runs under the
  `production` GitHub Environment. A maintainer must approve the
  deploy in the Actions UI before images are pushed and signed.
  Tag push alone no longer ships an image.
- **`Backend::list` signature.** Takes a `ListOptions { limit,
  start_after }`. Custom `Backend` implementations outside this
  repo must update to match.

### Added

- `SECURITY.md` gained an "Operational hardening notes" section
  covering the symlink policy, credential redaction, slow-drip
  timeout, pagination caps, `bucket_path` validation, CORS
  posture, tempfile / `$TMPDIR` posture, and the wire-bytes-cap
  invariant.
- Regression tests for every finding above (see `tests/` and
  `src/backend/*.rs`).

## [0.1.0-alpha.2] - 2026-08-05

Second alpha. Additive-only over `0.1.0-alpha.1`; no runtime
behaviour change beyond the boot diagnostic WARN line described
below. Existing `0.1.0-alpha.1` images and configs remain valid.

### Added

- **Compat corpus** (`compat/s3-ferry/config/*.env`) — verbatim
  copies of S3-Ferry's shipped env files, exercised by
  `tests/compat_corpus.rs` to assert every S3-Ferry field name
  is on a human-reviewed coverage list before it can pass CI.
  A new upstream field will fail CI until a human reviews it
  against the FileFerry target contract.
- **Boot diagnostic pass** (`src/diagnose.rs`) — emits
  `tracing::warn!` at startup for every accepted-but-unwired
  config field set to a non-default value, naming the field
  and the planned release when it will become active. Currently
  detects `cors_origin`.

### Corrected

- Overclaim in the `[0.1.0-alpha.1]` intro: `POST /v1/files/copy`
  actually returns `204`, not the `201` S3-Ferry emits — this
  divergence was not previously documented. See the "Known
  coverage gaps" section of `[0.1.0-alpha.1]` below (all still
  apply to `0.1.0-alpha.2` — no gaps have been closed yet).

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
  a CORS-aware reverse proxy until enforcement lands in a later
  release. The boot diagnostic warns when a non-default
  `cors_origin` is set.
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
  run in CI. A LocalStack-based comparison job is planned for
  a later release.
- **Unknown-field tolerance on JSON bodies.** Matches S3-Ferry's
  silent-accept behaviour for the alpha window; will tighten
  to `400` with a specific error code in `v0.2.0`.

[Unreleased]: https://github.com/turnerrainer/fileferry/compare/v0.1.3-alpha...HEAD
[0.1.3-alpha]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.3-alpha
[0.1.0-alpha.2]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.0-alpha.1
