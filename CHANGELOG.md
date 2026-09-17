# Changelog

All notable changes to FileFerry will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **T-22 — `fileferry doctor` subcommand.** New synchronous
  `fileferry doctor [--config <path>]` invocation. Loads the
  same config the runtime would load, runs the preflight WARN
  block (T-21), prints a green/amber report to stdout with a
  config summary + every warning, exits `0` (green), `1`
  (warnings surfaced), or `2` (config invalid). Bearer tokens
  and other secrets are shown as `SET (masked)` — the raw
  value never lands in the report. Handled BEFORE tokio starts
  so it stays fast and stderr-quiet (doctor output is
  stdout-only for shell composition, e.g.
  `fileferry doctor >/dev/null || alert`). New module
  `src/doctor.rs`; end-to-end test spawns the binary via
  `CARGO_BIN_EXE_fileferry` and asserts both exit codes.

- **T-21 — numbered preflight WARN block (fleet stronghold §11,
  TIM pattern).** `warn_if_unauth_non_loopback` grew into six
  checks with stable `W-<n>` ids. `W-1` is the previous
  unauth-non-loopback check; `W-2..W-6` cover
  `trust_network=true` without a token, `FILEFERRY_ADMIN_ENABLED`
  on a non-loopback bind, `documentation_enabled: true` on a
  non-loopback bind (heads-up for the pre-admin state),
  `copy_inactivity_secs` past the 5 minute ceiling (F3 slow-drip
  defence weakened), and `max_request_bytes` past 100 MiB
  (usually a misconception that the request body carries the
  file payload). Each check has a legitimate override so a
  hard fail would break existing deployments; the id makes the
  check log-alertable without matching against the wire
  message. New module `src/boot_warnings.rs` — adding a check
  is one `fn check_<n>` plus one entry in `preflight`.

### Security

- **Bump `rustls` to 0.23.45** (was 0.23.44) to pick up the fix
  for RUSTSEC-2026-0285: TLS 1.3 handshake messages were
  incorrectly accepted across encryption-level boundaries in
  0.23.44 (severity 5.3 medium). Reached transitively via
  `aws-sdk-s3 → aws-smithy-http-client → rustls`, so the client
  side of every S3 request was affected. Applied via
  `cargo update -p rustls`; no code changes.

- **AP-6 / T-11 — clip user-controlled substrings in error
  responses and log lines to 256 chars.** Before this change,
  an attacker sending a huge query string or malformed body
  surfaced as a proportionally huge error message: the axum
  `Query<T>` rejection text embeds the offending value
  verbatim (`unknown variant `<4 KB of A's>`...`), and
  `serde_json` parse errors quote a chunk of the payload.
  That's a free amplifier — 1 KB request → 4 KB response and
  a 4 KB log line per hit. Now `FerryError::IntoResponse`
  runs the message through `clip_user_message` (safely
  truncated to `MAX_USER_MESSAGE_LEN = 256` chars at a UTF-8
  boundary, `...` marker appended when clipped) before it
  lands in the JSON body OR the tracing field. `TypedQuery`
  and `TypedJson` apply the same clip at rejection
  construction — belt-and-braces so a refactor on either
  side keeps the bound. Verified via a break-the-fix probe
  that sends 4 KB / 64 KB / 1 MB inputs and asserts the
  emitted message stays flat.

### Changed (breaking)

- **`GET /api` (OpenAPI recon endpoint) now defaults to 404
  (F-FF-3 / T-6).** Serve it by setting `FILEFERRY_ADMIN_ENABLED`
  to `1`, `true`, or `yes` at boot (case-insensitive). When the
  env-gate is disabled, `/api` returns a **bodyless 404** — not
  401 — so the response never reveals the gate's existence to an
  unauth caller. The `documentation_enabled` YAML flag still
  applies on top of the env-gate: when admin is enabled but
  `documentation_enabled: false`, `/api` still 404s (dual gate
  lets operators keep the env var on for tooling while silencing
  the doc endpoint per-config). `/`, `/health` remain public
  unconditionally. Rationale: `/api` leaked the FileFerry
  version, the route table, and the DTO shapes to any unauth
  caller — exactly the reconnaissance signal AP-2 (fleet
  stronghold §3.3) targets. Clients or CI tooling that scraped
  `/api` from the default endpoint must set the env var
  explicitly.

### Chore

- **Drop the stale `RUSTSEC-2026-0253` (lru unsound) ignore from
  `.cargo/audit.toml` and `deny.toml`.** `aws-sdk-s3 1.146.1`
  now pulls `lru 0.18.4` transitively — verified via
  `cargo tree -i lru` on 2026-09-18 — so the advisory no longer
  applies to this build. Keeping the ignore around triggered a
  `cargo deny check` warning
  (`warning[advisory-not-detected]`).

## [0.2.1-alpha] - 2026-09-14

Base-image security patch. Supersedes `0.2.0-alpha`, which never
shipped an image: Trivy blocked the multi-arch build on 12
Debian base-image CVEs (3 CRITICAL + 9 HIGH) in `perl-base`,
`libsqlite3-0`, `libpcre2-8-0`, and `gzip`. All had fixes
available in the Debian security archive but were not present in
the `debian:13.6-slim` base tag pinned by the Dockerfile.

Feature narrative identical to `[0.2.0-alpha]` below — no code
changes, no test changes. Only the Dockerfile and the release
metadata (Cargo.toml, VERSION, docker-compose.yml, README,
CLAUDE.md) move.

### Security

- **Rebuild against latest Debian 13-slim + `apt upgrade`.**
  Dockerfile base tag changed from `debian:13.6-slim` (frozen
  minor pin) to `debian:13-slim` (major-tracked floating tag);
  runtime-stage build now runs `apt-get upgrade -y` before
  installing the pinned runtime packages, so every rebuild
  starts from the latest Debian security patches. Closes the
  12 base-image CVEs Trivy caught on the `0.2.0-alpha` build:
  - CVE-2026-13221 (perl-base, CRITICAL)
  - CVE-2026-42496 (perl-Archive-Tar, CRITICAL)
  - CVE-2026-8376 (perl, CRITICAL)
  - CVE-2026-41992 (gzip, HIGH)
  - CVE-2026-86145, CVE-2026-89161 (libpcre2-8-0, HIGH)
  - CVE-2026-11822, CVE-2026-11824 (libsqlite3-0, HIGH)
  - CVE-2026-42497, CVE-2026-48962, CVE-2026-57432,
    CVE-2026-57433 (perl-family, HIGH)

### Note on `v0.2.0-alpha`

The `v0.2.0-alpha` git tag exists on origin but there is
**no signed image and no GitHub Release** for it. The tag is
retained only as a historical marker for the commit at which the
`[0.2.0-alpha]` feature set was frozen; the shipping artifact for
that feature set is `0.2.1-alpha`. Consumers should treat
`v0.2.0-alpha` as if it never existed and pull `0.2.1-alpha`
instead. Nothing was published under the earlier version tag —
Trivy's block is the reason.

## [0.2.0-alpha] - 2026-09-13

Fourth alpha. Post-release hardening pass driven by the h2ck.me v1
break-tests (`v1/BREAK-TESTS/*.md`), the h2ck.me v1
public-exposure analysis, and the fleet-wide
`FLEET-STRONGHOLDS.md`. Version MINOR-bumps to `0.2.0-alpha`
because the release introduces a new `security:` config axis, a
new `FILEFERRY_OFFLINE` runtime axis, breaking wire changes to the
audit-log line, breaking response-body-shape changes for
extractor rejections, and a breaking compose posture flip
(`read_only: true` on the rootfs).

Every new invariant has a regression test; the suite grew 53 →
82 passing tests (48 unit + 2 compat + 32 integration).

### Added

- **Optional inter-service bearer token — F-FF-1, F-FF-2, F-FF-3
  (CRITICAL public-exposure finding closed).** New `security:`
  config block gates `/v1/files` and `/v1/files/copy` behind an
  `Authorization: Bearer <token>` when configured. `/`, `/health`,
  `/api` remain public. Token is env-referenced
  (`inter_service_token_env`) — never inlined in YAML. Comparison
  is constant-time via `subtle`; `SecurityConfig::Debug` masks the
  value as `***REDACTED***`. Boot logs a WARN when the listener is
  non-loopback AND no token is configured AND
  `security.trust_network=false`. Absent block ↔ pre-auth
  behaviour (backwards-compatible).
- **Offline mode — `FILEFERRY_OFFLINE=1`** (also `true`, `yes`,
  case-insensitive) replaces the S3 backend with a stub that
  fails every S3 call with
  `Upstream("offline mode: outbound blocked by FILEFERRY_OFFLINE")`.
  FS backend is unaffected. Prevents accidental live-S3 traffic
  during pentest / break-tests. Fleet stronghold §9.1.
- **Five default response security headers** (CSP, HSTS,
  X-Frame-Options, X-Content-Type-Options, Referrer-Policy) on
  every response, belt-and-braces against reverse-proxy
  misconfiguration. Fleet stronghold §5.1.
- **W3C `traceparent` + `x-trace-id` on every response.** When the
  caller sends a valid inbound `traceparent`, the same `trace_id`
  is echoed back so log-shippers can correlate a Ruuter-fronted
  request across services. Otherwise a fresh 128-bit trace_id is
  synthesised. Fleet stronghold §1.6 / O1.
- **Access-log middleware** emits one INFO
  `http_request_completed` line per request with method, route,
  status, duration, and trace_id inherited from inbound
  `traceparent`. Fleet stronghold §1.2 (FN-LOG-3).
- **`LOG_ANSI` env var** — `1`/`true` forces ANSI colour on stderr,
  `0`/`false` forces off. Absent = auto-detect via `atty` (off
  under Docker / systemd). Fleet stronghold §1.1 (FN-LOG-1).
- **CLAUDE.md** — new agent-facing brief. Codifies the verification
  set, invariant tables (F, FN, F-FF, fleet-stronghold groups),
  breaking-change grep cheat-sheets, config search order,
  runtime-env-var table, repo landmarks, and the never-bump-
  version / never-tag / never-dispatch-publish rules for LLM
  contributors.

### Changed (breaking)

- **Query and body extractor rejections now return structured JSON
  (FN2).** `Query<T>` failures land as
  `{"error":"bad_query","message":"..."}` (400); malformed JSON
  bodies land as `{"error":"bad_body","message":"..."}` (400);
  oversize bodies land as
  `{"error":"body_too_large","message":"..."}` (413). No bare
  `text/plain` errors remain on the response surface. Clients that
  parse bare-text 4xx bodies must switch to JSON parsing.
- **Copy-audit log line renamed and reshaped (FN-LOG-3).** The
  `copy complete` INFO line is now
  `file_transfer_completed`. The `source_path` /
  `destination_path` fields are gone; they are replaced by
  `source_path_hash` / `destination_path_hash` (SHA-256, first 12
  hex chars). Adds `duration_ms` and `outcome=success`. Log
  shippers / SIEM rules that grep the old event name or raw paths
  must update. Rationale: tenant identifiers in paths (e.g.
  `/opt/tenant-A-billing/2026-Q3.csv`) no longer leak into
  downstream log storage.
- **`FerryError::NotFound` no longer echoes the requested path in
  the response body (FN6).** Fixed message string; full path stays
  in the operator WARN log for debugging. Clients that grep the
  message for the file name must read `error == "not_found"` from
  the JSON body instead.
- **`CopyFileRequest` and `ListFilesQuery` DTOs now use
  `#[serde(deny_unknown_fields)]` (FN-LOG-2).** Junk fields in a
  request body or query string are refused instead of silently
  dropped. Clients that pack "just-in-case" extra fields must
  remove them.
- **Shipped `docker-compose.yml` is now `read_only: true` (FN4).**
  Writable data lives on a named volume
  (`fileferry_data:/app/data:rw`). Overrides that assumed a
  writable rootfs need to switch to the named volume — or replace
  it with a `- ./data:/app/data:rw` bind mount. Rationale: a
  shell-level RCE inside the container can no longer overwrite
  `/app/fileferry` or drop a shared library.

### Fixed

- **FN1 — Bare `.` path bypassed the validator.**
  `sourceFilePath: "."` previously resolved to the data directory
  itself and leaked `500 io_error: Is a directory (os error 21)`.
  The validator now rejects `.` and `..` as segment-exact so
  `.hidden` filenames still work but `foo/.`, `./foo`, and `.`
  are refused. Returns the structured
  `{"error":"invalid_path"}` 400 like every other rejection.

### Security (posture reinforcement)

- **Non-loopback boot WARN.** When the listener is
  non-loopback AND `security.inter_service_token_env` is unset
  AND `security.trust_network=false`, boot emits a diagnostic
  WARN naming the concrete impact (S3 API cost, cross-backend
  exfil) and how to fix it. Backwards-compatible — existing
  deployments keep booting; the WARN is advisory. Future
  release may promote to a boot refusal per fleet stronghold
  §3.1.
- **Constant-time bearer compare** via `subtle::ConstantTimeEq`
  closes the timing side-channel on the auth path.

### Docs

- `book/src/configuration.md` — new `security:` block reference,
  `copy_inactivity_secs`, expanded env-var table.
- `book/src/failure-modes.md` — new matrix rows for the
  extractor-error codes and `unauthorized`; new sections on
  default response headers, boot WARNs, and the audit-log line.
- `book/src/getting-started.md` — hardened `docker run` recipe
  (`--read-only`, `--tmpfs`, `--cap-drop`, `--security-opt`),
  bearer-gate quickstart, and offline-mode note.
- `SECURITY.md` — new operational-hardening summary for the
  additive items above.
- `README.md` — refreshed version line and "Upgrading from"
  section that brackets both `0.1.0-alpha.2` and `0.1.3-alpha`.
- `CLAUDE.md` — see "Added" above.

### Internal

- Dependencies added: `subtle = "2.6"` (auth compare),
  `sha2 = "0.10"` (audit-log path hash; already transitively
  present via `aws-sdk-s3`).
- Middleware ordering (outer → inner):
  `TraceLayer → security_headers → traceparent → DefaultBodyLimit
  → TimeoutLayer → routes`, all wrapped by the `access_log`
  middleware.
- New modules: `src/access_log.rs`, `src/auth.rs`,
  `src/backend/offline.rs`, `src/extract.rs`,
  `src/security_headers.rs`, `src/trace_headers.rs`.
- `RUSTSEC-2026-0253` ignore in `deny.toml` still reports
  "advisory-not-detected" (`aws-sdk-s3` moved past the affected
  `lru`); leaving the ignore in place one more cycle to observe
  post-release rather than combining a `deny.toml` cleanup with
  the release commit.

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

[Unreleased]: https://github.com/turnerrainer/fileferry/compare/v0.2.1-alpha...HEAD
[0.2.1-alpha]: https://github.com/turnerrainer/fileferry/releases/tag/v0.2.1-alpha
[0.2.0-alpha]: https://github.com/turnerrainer/fileferry/releases/tag/v0.2.0-alpha
[0.1.3-alpha]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.3-alpha
[0.1.0-alpha.2]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/fileferry/releases/tag/v0.1.0-alpha.1
