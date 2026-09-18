# CLAUDE.md — orientation for LLM contributors

Read this file before you touch anything in `turnerrainer/fileferry`. It
is the agent-facing brief: what to run, what to preserve, and how to
spot configs that break under the current release.

- **Current release**: `0.2.2-alpha`. Post-audit hardening pass
  that closes the last nine items on the h2ck.me v1 backlog
  (T-6 /api env-gate, T-11 clip user strings, T-17 body-read
  timeout, T-18 405-vs-404 pinning, T-21 boot WARN block,
  T-22 `fileferry doctor`, T-23 SIGTERM graceful shutdown,
  T-24 README security section) plus RUSTSEC-2026-0285 in
  rustls. See `CHANGELOG.md` `[0.2.2-alpha]` for the full
  narrative and §3 for the seams to check.
  Prior release `0.2.1-alpha` (Debian base-image CVE patch)
  superseded `0.2.0-alpha` before that ever shipped an image
  — Trivy blocked the `0.2.0-alpha` publish on 12 Debian
  base-image CVEs. All the `[0.2.0-alpha]` semantics still
  apply — see `CHANGELOG.md` `[0.2.0-alpha]` for the feature
  narrative. The MINOR bump vs `0.1.3-alpha` reflects a new
  `security:` config axis, a new `FILEFERRY_OFFLINE` runtime
  axis, breaking wire changes to the audit-log line, and
  breaking response-body-shape changes for extractor
  rejections — see §3 below. **Agents never bump the
  top-level version, never tag, never dispatch the publish
  workflow, never merge a release PR.** Releases are cut by the
  maintainer as a dedicated `chore(release)` commit merged from
  a `release/*` branch.
- **Branches**: `dev` is the default branch AND the release
  branch. Feature work goes on `feat/*`, `fix/*`,
  `hardening/*`, etc. — PR into `dev`. Release cuts are a
  `release/*` PR that bumps `Cargo.toml` and lands on `dev`.
  **Merging a version-bump PR into `dev` runs the release to
  completion unmanned** — `publish.yml`'s
  `tag-on-version-bump` job reads the version from
  `Cargo.toml`, short-circuits when the matching `v<version>`
  tag AND a GitHub Release both exist (feature merges cost
  nothing beyond a quick `ls-remote` + `gh release view`),
  otherwise tags the merge commit (if needed) and dispatches
  `publish`. `publish` builds multi-arch images, runs Trivy +
  cosign, creates the GitHub Release entry, and verifies it
  landed — no Actions-UI approval step. The single human
  touchpoint is the PR review + merge; the safety gates are
  the PR review itself plus the workflow-internal ones (Trivy
  blocks on HIGH/CRITICAL, `/health` smoke test must pass,
  cosign signing must succeed, Release-verify step confirms
  the entry landed). Agents never bump the version, never
  tag, never merge release PRs.
  (There is no `main` branch on origin; the earlier convention
  "release cuts merge `dev → main`" was never implemented and
  has been retired in favour of the model above.)
- **Companion files**: `README.md` (user-facing quickstart),
  `SECURITY.md` (disclosure + posture), `STANDARDS.md` (build/test/publish
  rules), `CHANGELOG.md` (per-release breaking-change narrative), and
  `book/src/` (mdBook — long-form user docs).

## 1. Verification set (run before + after any change)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo audit --deny warnings
cargo deny check all
```

Baseline on the `0.2.2-alpha` release commit: **109/109 tests
pass** (66 unit + 2 compat + 2 doctor E2E + 39 integration).
Clippy, audit, and deny are all clean. If your change reduces
the test count or introduces a warning, that is a regression —
fix it before opening the PR.

## 2. Do-not-break invariants

These are audit-driven (`h2ck.me/FileFerry/v1/AUDIT.md`,
`v1/BREAK-TESTS/*`). Removing or weakening any of them is a regression,
and the accompanying regression tests will fire — read the test first,
then decide.

### 2.1 v1 audit fixes (F-series)

| Invariant | Enforced by | Regression test |
|---|---|---|
| FS backend refuses symlinks; opens files with `O_NOFOLLOW` (F1) | `src/backend/fs.rs` `resolve()`, `open_read`, `write_all` | `list_skips_symlinks`, `open_read_rejects_symlink*`, `write_all_refuses_symlink_destination` |
| `S3Config` masks `access_key_id` + `secret_access_key` in `Debug` (and via `AppConfig` composition) — same posture now applies to `SecurityConfig::inter_service_token` (F2) | `src/config.rs` hand-written `impl fmt::Debug` on both | `debug_of_s3config_never_contains_secrets`, `debug_of_appconfig_containing_s3_masks_secrets` |
| `stream_copy` aborts on per-poll inactivity (F3, `limits.copy_inactivity_secs`, default 30 s) | `src/backend/mod.rs` `stream_copy` + `TimeoutReader` | `stream_copy_aborts_when_source_stalls` |
| `GET /v1/files` is paginated: default `DEFAULT_LIST_LIMIT=1000`, cap `MAX_LIST_LIMIT=10_000`, over-cap → 413, `meta.nextCursor` on full pages (F4) | `src/backend/mod.rs`, `src/router.rs` | `list_pagination_returns_next_cursor_when_full`, `list_limit_over_cap_returns_413`, `list_start_after_rejects_traversal` |
| `S3Config::validate()` refuses `bucket_path` with `..` or leading `/` (F5) | `src/config.rs` | `bucket_path_traversal_rejected`, `bucket_path_leading_slash_rejected` |
| Non-empty `cors_origin` is a **hard boot failure** (no silent-drop) (F6) | `src/config.rs` `from_yaml()` bail | `cors_origin_set_causes_startup_failure` |
| `resolve()` error message does not echo the user-supplied path (F8, fixed string; full input goes to `tracing::warn!` only) | `src/backend/fs.rs` | `resolve_error_does_not_echo_user_path` |

### 2.2 v1 break-test follow-ups (FN + FN-LOG series, landed 2026-09-13)

| Invariant | Enforced by | Regression test |
|---|---|---|
| Path validator rejects `.` and `..` as segment-exact (FN1) — bare `.` no longer bypasses the guard and leaks EISDIR as 500 | `src/validate.rs` `is_traversal` | `rejects_bare_dot_and_dot_segments`, `copy_rejects_bare_dot_source_path` |
| `Query<T>` / `Json<T>` / oversize-body rejections return **structured JSON** `{error, message}`, never bare `text/plain` (FN2) | `src/extract.rs` `TypedQuery` / `TypedJson`; `DefaultBodyLimit::max` in the router | `list_missing_type_query_is_422`, `list_bad_limit_returns_structured_bad_query`, `copy_malformed_json_body_returns_structured_bad_body`, `copy_oversize_body_returns_structured_413` |
| Container rootfs is `read_only: true` in shipped `docker-compose.yml`; writable data lives on named volume `fileferry_data` (FN4) | `docker-compose.yml` | manual — see PR #8 test plan |
| `FerryError::NotFound` Display is the fixed string `file not found` — inner path stays for `tracing::warn!` only (FN6) | `src/error.rs` `IntoResponse` for `NotFound` | `copy_not_found_message_does_not_echo_marker` |
| Log stream contains 0 ESC bytes when piped (FN-LOG-1, fleet stronghold §1.1) | `src/main.rs` `init_tracing` (`with_ansi(atty::is(...))`) | check via `docker logs <ct> \| LC_ALL=C tr -cd $'\x1b' \| wc -c` — expect 0 |
| Every request emits an INFO access-log line with `trace_id` inherited from inbound `traceparent` (FN-LOG-3 / §1.2) | `src/access_log.rs` middleware, wired in `src/router.rs` | manual — see PR #14 |
| `CopyFileRequest` and `ListFilesQuery` use `#[serde(deny_unknown_fields)]` (FN-LOG-2) | `src/model.rs` | `copy_rejects_unknown_field_in_body`, `list_rejects_unknown_query_field` |
| Copy-audit log line hashes source/destination paths (SHA-256 first 12 hex) — never emit raw path to logs (FN-LOG-3 / §S4) | `src/backend/mod.rs` `path_hash` inside `stream_copy` | `path_hash_is_deterministic_and_short`, `path_hash_does_not_contain_raw_path_substring` |
| User-controlled substrings in error responses and log lines are clipped to `MAX_USER_MESSAGE_LEN` (256 chars) with a trailing `...` marker (AP-6 / T-11). Applied twice: once at extractor construction (`TypedQuery` / `TypedJson`) and again in `FerryError::IntoResponse` — belt-and-braces so a refactor on either side keeps the bound. | `src/error.rs` `clip_user_message`, `src/extract.rs` | `clip_user_message_*` (unit), `error_message_body_bounded_regardless_of_query_size`, `malformed_json_body_error_message_is_clipped` |
| Known route + wrong method → **405 Method Not Allowed** (with `allow: <valid methods>` response header per RFC 7231 §6.5.5), NOT 404 (T-18). Unknown route → 404. Behavior provided by axum's routing layer; regression tests pin it so a future middleware layer or router refactor doesn't accidentally swallow it. | `src/router.rs` route table | `method_not_allowed_on_known_route_returns_405`, `unknown_route_still_returns_404` |
| Graceful shutdown on SIGTERM / SIGINT — axum stops accepting new connections when the signal fires and waits for in-flight requests to complete (T-23, OWASP-PROBES §34.4). Under `docker stop` / K8s rolling-restart, a `POST /v1/files/copy` finishes cleanly and emits its audit-log line instead of dying half-way. | `src/shutdown.rs` `shutdown_signal`, wired via `axum::serve(...).with_graceful_shutdown(...)` in `src/main.rs` | `shutdown_signal_is_pending_then_resolves_on_sigterm` |
| Request-body read has a 30s hard cap (`extract::BODY_READ_TIMEOUT`) that fires **independently** of `limits.request_timeout_secs` (T-17). Slow-drip peers used to hold a connection slot for up to the total-request timeout (5 min); the narrower body-read cap defends the accept queue. Surfaces as HTTP 408 with `error: body_read_timeout`. | `src/extract.rs` `TypedJson::from_request` (`tokio::time::timeout` wrapper), `src/error.rs` `FerryError::BodyReadTimeout` | `slow_body_times_out_at_body_read_cap` (uses `#[tokio::test(start_paused = true)]` + `advance` — no wall-clock 30 s wait) |

### 2.3 Public-exposure defenses (F-FF-series, landed 2026-09-13)

| Invariant | Enforced by | Regression test |
|---|---|---|
| Optional inter-service bearer token gates `/v1/files*` when `security.inter_service_token_env` names a live env var (F-FF-1, F-FF-2). Comparison is constant-time via `subtle::ConstantTimeEq` (§3.2). `/`, `/health` remain public unconditionally. | `src/auth.rs` `require_bearer`, `src/router.rs` gated sub-router | `auth_off_by_default_list_still_open`, `auth_required_list_rejects_missing_bearer`, `auth_required_list_rejects_wrong_bearer`, `auth_required_list_accepts_correct_bearer`, `auth_required_copy_rejects_missing_bearer`, `auth_required_health_and_root_still_open` |
| `/api` (recon endpoint) defaults to 404 unless `FILEFERRY_ADMIN_ENABLED` is truthy at boot (F-FF-3 / T-6, AP-2 fleet stronghold §3.3). Even when admin is enabled, `documentation_enabled: false` in YAML still 404s. Never reveals the admin gate's existence to unauth callers — always returns 404, never 401. | `src/config.rs` `admin_enabled_from_env`, `src/router.rs` `openapi` handler | `openapi_returns_404_when_admin_disabled_by_default`, `openapi_returns_404_when_admin_enabled_but_docs_disabled`, `openapi_lists_expected_paths`, `admin_enabled_from_env_parses_truthy_and_falsy_values`, `admin_enabled_defaults_to_false` |
| Boot emits a numbered preflight WARN block with stable `W-<n>` ids (T-21, fleet stronghold §11 TIM pattern): W-1 unauth non-loopback bind (was `warn_if_unauth_non_loopback`), W-2 `trust_network=true` without a token, W-3 admin recon endpoint on public bind, W-4 `documentation_enabled` still on public bind, W-5 `copy_inactivity_secs` past the 5 min ceiling, W-6 `max_request_bytes` past the 100 MiB ceiling. Each check has a legitimate override so a hard fail would break existing deployments; the id makes the check log-alertable. | `src/boot_warnings.rs` `preflight` + `log_preflight`, wired in `src/main.rs` | `default_loopback_is_silent`, `wildcard_bind_without_auth_fires_w1_and_w4`, `wildcard_with_token_only_fires_w4`, `trust_network_without_token_fires_w2`, `admin_enabled_on_wildcard_fires_w3`, `copy_inactivity_above_ceiling_fires_w5`, `copy_inactivity_at_ceiling_is_silent`, `max_request_bytes_above_ceiling_fires_w6`, `every_warning_id_is_unique` |

### 2.4 Fleet-stronghold adoptions (landed 2026-09-13)

| Invariant | Enforced by | Regression test |
|---|---|---|
| Five default security headers on every response (CSP, HSTS, X-Frame-Options, X-Content-Type-Options, Referrer-Policy) — §5.1 | `src/security_headers.rs` middleware | `every_response_carries_security_headers` |
| W3C `traceparent` + `x-trace-id` on every response; inbound `traceparent` trace_id is echoed, otherwise synthesised — §1.6 / O1 | `src/trace_headers.rs` middleware | `every_response_carries_traceparent_and_x_trace_id`, `traceparent_inbound_id_is_echoed`, plus unit tests in the module |
| `FILEFERRY_OFFLINE=1` (or `true`/`yes`) replaces the S3 backend with `OfflineBackend` that fails every call with `Upstream("offline mode: ...")`. FS backend unaffected — §9.1 | `src/backend/offline.rs`, `src/main.rs` wiring | `offline_env_recognises_common_truthy_values`, `offline_backend_lists_returns_upstream_error`, `offline_backend_open_read_returns_upstream_error` |

If a reviewer or audit asks you to "just relax" one of these, push
back — they were designed to catch attempts to remove them.

## 3. Breaking changes vs `0.1.3-alpha` (shipped in `0.2.0-alpha`)

If you are upgrading callers, custom backends, or CI configs from
`0.1.3-alpha`, these are the seams to check. Full narrative:
`CHANGELOG.md` `[0.2.0-alpha]`.

| # | Change | Grep to find affected sites | Fix |
|---|---|---|---|
| 1 | Bare `.` or explicit `./`, `/.`, `a/./b`, `foo/.` in `sourceFilePath` / `destinationFilePath` / `startAfter` → 400 `invalid_path` (was 500 `os error 21` for `.`) | Client code that uses path components equal to `.` | Never send a bare `.` or a `.` segment; use a real filename |
| 2 | `Query<T>` / `Json<T>` failures + oversize body return **`application/json` `{error, message}`** (was `text/plain`) | Client code that parses bare-text 4xx bodies | Switch to JSON parsing on 4xx / 413. Codes: `bad_query`, `bad_body`, `body_too_large`, `invalid_path`, `unauthorized`, `not_found`, `same_storage_type`, `backend_not_configured`, `list_limit_too_large`, `transfer_too_large`, `upstream_error`, `io_error`, `internal_error` |
| 3 | `CopyFileRequest` / `ListFilesQuery` now `deny_unknown_fields` | Client code that sends extra fields "just in case" | Remove the extra fields; the server rejects with 4xx |
| 4 | `NotFound` response body no longer echoes the requested path | Client code that greps the message for the file name | Read `error == "not_found"` from the JSON body instead |
| 5 | Optional bearer gate: when `security.inter_service_token_env` is set, `/v1/files*` require `Authorization: Bearer <token>`; missing/wrong = 401 `unauthorized`. `/`, `/health` stay public unconditionally. | `grep -n 'security:' fileferry.yaml your-configs/` | Wire the token via env var; document `trust_network=true` only if a reverse proxy authenticates first |
| 6 | Response now carries `traceparent` + `x-trace-id` + 5 security headers | Client tests that assert exact response-header set | Update assertions; treat these as always-present |
| 7 | Copy-audit log line renamed `copy complete` → `file_transfer_completed`; `source_path` / `destination_path` fields replaced with `source_path_hash` / `destination_path_hash` (SHA-256 first 12 hex); new `duration_ms`, `outcome` fields | Log-shipping pipelines / SIEM rules that grep for `copy complete` or the raw path | Update parsers to the new event name + hashed fields |
| 8 | `docker-compose.yml` service is now `read_only: true`; data path is a named volume `fileferry_data` at `/app/data` | Compose overrides that assumed a writable rootfs | Use the named volume (default) or replace the `volumes:` entry with a `- ./data:/app/data:rw` bind mount |
| 9 | `FILEFERRY_OFFLINE=1` disables S3 outbound — every S3 call returns `Upstream("offline mode: ...")` | Pentest / break-test runners that expect FileFerry to reach real S3 | Explicitly unset the env var; or leave set on purpose to prevent live-S3 traffic |
| 10 | `/api` (OpenAPI recon endpoint) now defaults to 404. Serve it by setting `FILEFERRY_ADMIN_ENABLED=1` at boot (F-FF-3 / T-6, AP-2). `/`, `/health` still public. Returns 404 not 401 when disabled — never leaks the gate's existence. | Client / tooling that scrapes `/api` for the route table | Set `FILEFERRY_ADMIN_ENABLED=1` when the tooling needs `/api`; otherwise treat it as gone |

Also carried forward from the `0.1.3-alpha` breaking-change list:
`Backend::list(ListOptions{...})` signature, `cors_origin` refuses on
non-empty, `bucket_path` refuses on `..` or leading `/`, versioning
scheme (bare `-alpha` on PATCH), publish gated on `production`
Environment, `copy_inactivity_secs` default, and list responses gain
`meta.nextCursor`. See `CHANGELOG.md` `[0.1.3-alpha]` for full text.

## 4. Config search order & best-practice values

Config resolves in this order (first hit wins):

1. `--config <path>` CLI arg
2. `FILEFERRY_CONFIG` env var
3. `./fileferry.yaml`
4. Built-in defaults (FS backend at `./data`, S3 disabled, security block absent)

Recommended production config:

```yaml
port: 8080
documentation_enabled: true      # /api summary; safe to leave on
cors_origin: ""                  # MUST stay empty — CORS at reverse proxy
fs:
  data_directory: /app/data      # named volume in shipped compose
s3:                              # omit the whole block if S3 unused
  region: eu-west-1
  endpoint_url: ""               # empty = default AWS endpoint
  bucket: your-bucket
  bucket_path: prefix/           # no leading '/', no '..'
  access_key_id_env: FILEFERRY_S3_ACCESS_KEY_ID
  secret_access_key_env: FILEFERRY_S3_SECRET_ACCESS_KEY
limits:
  max_request_bytes:   33554432       # 32 MiB
  max_response_bytes:  5368709120     # 5 GiB
  request_timeout_secs: 300
  copy_inactivity_secs: 30
security:                        # optional; comment the whole block for open access
  inter_service_token_env: FILEFERRY_INTER_SERVICE_TOKEN
  trust_network: false           # true = reverse proxy is trusted to authenticate
```

Secrets **must** come from env vars named by the config; never inline
them in YAML. `deny_unknown_fields` is set on every YAML shape and
every request DTO — typos like `porrt: 9000` or `sourceFilepath`
fail to parse rather than silently apply the default.

Container posture (shipped `docker-compose.yml` sets all of this — do
not weaken it): `read_only: true` rootfs, `no-new-privileges: true`,
`cap_drop: ALL`, uid 1000 non-root, private `tmpfs /tmp`,
`fileferry_data` named volume mounted at `/app/data`.

### 4.1 Runtime env vars

| Env var | Effect |
|---|---|
| `RUST_LOG` | Standard `tracing-subscriber` filter; e.g. `info,fileferry=debug` |
| `LOG_ANSI` | `1`/`true` forces ANSI colour on stderr; `0`/`false` forces off. Absent = auto-detect via `atty` (off under Docker/systemd) |
| `FILEFERRY_CONFIG` | Path to YAML config (higher precedence than `./fileferry.yaml`) |
| `FILEFERRY_OFFLINE` | `1`/`true`/`yes` → S3 backend is stubbed; every S3 call returns `Upstream("offline mode")`. FS backend unaffected. For pentest / break-tests |
| `FILEFERRY_ADMIN_ENABLED` | `1`/`true`/`yes` → serve `/api` (OpenAPI recon endpoint). Absent / anything else → `/api` returns 404. F-FF-3 / T-6 (AP-2 fleet stronghold §3.3): recon endpoints are opt-in |

### 4.2 CLI subcommands

- `fileferry` (no subcommand) — run the HTTP server (default).
- `fileferry doctor [--config <path>]` — config + posture
  health check (T-22). Loads the config, runs the preflight
  WARN block, prints a green/amber report to stdout, exits
  `0` (clean), `1` (WARNs), or `2` (config invalid). Handled
  before tokio starts; safe to run in CI. Secrets are masked.
| Env var named by `security.inter_service_token_env` | Value is the bearer token clients must present as `Authorization: Bearer <value>` on `/v1/files*` |
| Env vars named by `s3.access_key_id_env` / `s3.secret_access_key_env` | S3 credentials, resolved at boot; missing = hard boot failure |

## 5. Repo landmarks

- `src/config.rs` — YAML → `AppConfig`; boot-time policy gates (F5
  bucket_path, F6 CORS). `SecurityConfig` with hand-written masked
  `Debug`. Every rejection has an explicit regression test.
- `src/error.rs` — `FerryError` enum + `IntoResponse`. **All 4xx/5xx
  responses go through here and emit structured `{error, message}`
  JSON.** New variants MUST be mapped in both `status()` and `code()`.
- `src/extract.rs` — `TypedQuery<T>` / `TypedJson<T>` wrappers that
  translate axum's `Query` / `Json` rejections into `FerryError`.
  Use these, not `axum::extract::{Query, Json}` directly.
- `src/backend/mod.rs` — `Backend` trait, `ListOptions`,
  `stream_copy` (with per-request `TimeoutReader`, `LimitedReader`,
  and the hashed audit-log emission), plus the shared `path_hash`.
- `src/backend/fs.rs` — FS backend + symlink policy (F1) + `O_NOFOLLOW`.
- `src/backend/s3.rs` — S3 backend + `start_after_key` derivation.
- `src/backend/offline.rs` — offline stub returned when
  `FILEFERRY_OFFLINE` is truthy AND an S3 block is configured.
- `src/shutdown.rs` — graceful-shutdown future awaited by
  `axum::serve(...).with_graceful_shutdown(...)`. Fires on
  SIGTERM (container) or SIGINT (dev Ctrl-C); wired in
  `src/main.rs`. In-flight `POST /v1/files/copy` requests
  complete cleanly under `docker stop` / K8s rolling-restart
  instead of dying half-way with no audit-log line.
- `src/auth.rs` — `require_bearer` middleware. Constant-time compare
  via `subtle`. Applied only when `security.inter_service_token` is
  `Some`.
- `src/access_log.rs` — per-request INFO access-log middleware with
  trace-id extraction from inbound `traceparent`.
- `src/trace_headers.rs` — response-side `traceparent` + `x-trace-id`.
- `src/security_headers.rs` — the five default response headers.
- `src/router.rs` — HTTP surface (`/`, `/health`, `/api`, `/v1/files`,
  `/v1/files/copy`) and middleware stacking. **When adding a route
  that touches a backend, put it inside the `gated` sub-router so it
  respects the bearer gate. Public liveness / discovery goes on the
  outer router.**
- `src/boot_warnings.rs` — numbered preflight WARN block (T-21).
  Each check has a stable `W-<n>` id so log-alert pipelines can
  key on it without matching the wire message. Adding a check:
  add a `fn check_<n>` returning `Option<BootWarning>` and
  reference it from `preflight`. Never reuse an existing id.
- `src/doctor.rs` — `fileferry doctor` subcommand (T-22).
  Loads the runtime config, runs `boot_warnings::preflight`,
  prints a green/amber report to stdout, exits `0`/`1`/`2`.
  Handled in `main()` BEFORE tokio starts so no runtime is
  spun up for a config check. Secrets render as `SET (masked)`
  — the raw bearer token never lands in the report. See
  `tests/doctor_cli.rs` for the E2E subprocess test.
- `src/main.rs` — boot sequence: config load → diagnose → backend
  init (offline check here) → preflight WARN block
  (`boot_warnings::log_preflight`) → serve. Version bumps and
  release tags NEVER land in a feature PR.
- `src/validate.rs` — path whitelist + `.` / `..` segment ban.
- `src/model.rs` — DTOs, all with `#[serde(deny_unknown_fields)]`.
- `tests/` — integration + regression tests grouped by finding ID.
- `.github/workflows/publish.yml` — release pipeline; **gated on the
  `production` GitHub Environment**. Read the top-of-file comment
  before editing.
- `book/src/` — user-facing docs; `configuration.md` is the long-form
  reference. Keep in sync with `fileferry.yaml`.

## 6. Working rules for agents

- **Sequential tool use.** Do not parallelise dependent operations.
- **Prefer editing existing files** (`CHANGELOG.md`, `book/src/*.md`,
  the modules above) over creating new ones.
- **Every new fix on its own branch, PR into `dev`.** Meaningful,
  stand-alone changes get their own `fix/*` / `hardening/*` /
  `feat/*` / `security/*` branch. Never batch unrelated fixes.
- **When you change one of N parallel sites** (one `Backend` impl,
  one router handler, one DTO), grep for the pattern and verify
  every site. Bugs at seams passed the previous audit round because
  a caller was missed.
- **Write break-the-fix tests, not confirm-the-fix tests.** If you
  fix the FS backend to reject `..`, add a test that tries `foo/..`,
  `..`, `.`, and encoded variants — not just one that asserts the
  happy path.
- **New 4xx/5xx paths MUST use the structured JSON envelope.** Never
  return bare text. Route through `FerryError` (add a variant if the
  existing set doesn't fit).
- **Middleware ordering matters.** Under the current stack the
  outer-to-inner order is: `TraceLayer` → `security_headers` →
  `traceparent` → `DefaultBodyLimit` → `TimeoutLayer` → routes.
  The `access_log` middleware wraps the whole thing. If you insert
  a new layer, name the intended position in the commit message.
- **Never bump the top-level project version, never tag, never
  dispatch the publish workflow, never merge a release PR.**
  Releases are cut by the maintainer as a dedicated
  `chore(release)` commit on a `release/*` branch. When the
  maintainer merges the PR into `dev`, `publish.yml` runs the
  full pipeline unmanned (Trivy + cosign + Release entry +
  verify) — no separate Actions-UI approval. The workflow-
  internal gates (Trivy blocks HIGH/CRITICAL, `/health` smoke
  test, cosign, Release-verify) are the deploy safety net.
  **Agents don't participate in any of this on their own.**
- **When adding a config field**: (a) update the runtime struct in
  `src/config.rs`, (b) mirror in the `*Yaml` shape with
  `deny_unknown_fields`, (c) resolve in `from_yaml`, (d) add a
  masked `Debug` line if the field holds a secret, (e) add a sample
  entry to `fileferry.yaml`, (f) document in `book/src/`, and
  (g) add regression tests.

## 7. Where to find external context

- **h2ck.me security audits** — private GitHub org
  `github.com/h2ckme/FileFerry/v1/{AUDIT.md,FIX-KIT.md,PR-REVIEWS/,BREAK-TESTS/}`.
  Access via org membership; the fleet-wide index is
  `h2ckme/security-fleet/REVIEW-INDEX.md`.
- **Fleet strongholds** — `h2ck.me/FLEET-STRONGHOLDS.md` (fleet-wide
  cross-project hardening patterns; adopted items are pinned in §2.4).
- **Original S3-Ferry** — <https://github.com/buerokratt/S3-Ferry>.
- **Book (rendered)** — <https://turnerrainer.github.io/fileferry/>.
- **Images** — `docker.io/turnerrainer/fileferry:alpha` (moving
  alpha tag) and `ghcr.io/turnerrainer/fileferry:alpha`. Latest
  immutable version tag: `:0.2.2-alpha` (prior: `:0.2.1-alpha`,
  `:0.1.3-alpha`, `:0.1.0-alpha.2`, `:0.1.0-alpha.1` —
  `:0.2.0-alpha` was never built successfully; superseded by
  `:0.2.1-alpha` before any image shipped). Every tag is signed
  via cosign keyless (Sigstore OIDC) and carries in-toto
  provenance + SPDX SBOM attestations.
- **Releases** — <https://github.com/turnerrainer/FileFerry/releases>.
  The `publish.yml` workflow creates a GitHub Release entry as
  part of every tag push, populated from the matching
  `CHANGELOG.md` section.
- **Pre-1.0 "Latest" policy.** While the shipping version is
  0.x, every release is written with `prerelease: false` +
  `make_latest: true` so it populates the repo main-page
  Releases sidebar and the `/releases/latest` API. GitHub
  otherwise refuses "Latest" for anything with `prerelease:
  true`, and the project only ships alphas — the sidebar would
  be permanently empty. The semver tag suffix (`-alpha`)
  continues to communicate maturity to any consumer who reads
  the tag. **When the first `v1.0.0` ships**, flip the
  `Create GitHub Release` step in `publish.yml` back to
  `prerelease: ${{ steps.meta.outputs.prerelease }}` +
  `make_latest: ${{ steps.meta.outputs.prerelease == 'false' }}`
  so future betas / RCs / hotfix-alphas don't displace stable
  as Latest. There's a block comment on the step describing the
  flip.
- **README badges.** Top of the README carries shields.io
  badges for latest release + release date + license + image
  surface. These render on the repo main page independently of
  the Releases sidebar, so consumers see the current version
  even if a future policy change ever leaves the sidebar blank.
