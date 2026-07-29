# Changelog

All notable changes to FileFerry will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-rc.1] - 2026-07-29

First release candidate. Rust re-implementation of the JVM-based
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).
Complete parity with S3-Ferry's HTTP surface — same four
endpoints, same response envelopes, same path-validation rules,
same "no nested directories" `list` semantics.

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
- `HANDOFF.md` — entry point for the next contributor
- `tasks/backlog/001-domain-deep-dive-s3-ferry.md` — first
  task, marked landed on this release

### Deviations from DEV-REQUIREMENTS

- **MSRV bumped from `1.88` to `1.94`.** Reason: `aws-sdk-s3`
  1.140 and its transitive `aws-*` deps require `rustc >= 1.94.1`.
  Cargo.toml, Dockerfile builder tag, and CI matrix all set
  to 1.94. STANDARDS.md §2 records this.

[Unreleased]: https://github.com/turnerrainer/FileFerry/compare/v0.1.0-rc.1...HEAD
[0.1.0-rc.1]: https://github.com/turnerrainer/FileFerry/releases/tag/v0.1.0-rc.1
