# CLAUDE.md — orientation for LLM contributors

Read this file before you touch anything in `turnerrainer/fileferry`. It
is the agent-facing brief: what to run, what to preserve, and how to
spot configs that break under the current release.

- **Current release**: `0.1.3-alpha`. `dev` carries the post-h2ck.me-v1
  hardening pass (§2 + §3 below); the next release will bump the
  PATCH digit and cut a `chore(release)` commit — **the maintainer
  decides when; agents never bump the top-level version, never tag,
  never dispatch the publish workflow.**
- **Branches**: work on `dev`, PR into `dev`, release cuts merge `dev → main`.
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

Baseline on `dev` after the 2026-09-13 hardening pass:
**82/82 tests pass** (48 unit + 2 compat + 32 integration). Clippy,
audit, and deny are all clean. If your change reduces the test count
or introduces a warning, that is a regression — fix it before opening
the PR.

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

### 2.3 Public-exposure defenses (F-FF-series, landed 2026-09-13)

| Invariant | Enforced by | Regression test |
|---|---|---|
| Optional inter-service bearer token gates `/v1/files*` when `security.inter_service_token_env` names a live env var (F-FF-1, F-FF-2, F-FF-3). Comparison is constant-time via `subtle::ConstantTimeEq` (§3.2). `/`, `/health`, `/api` remain public. | `src/auth.rs` `require_bearer`, `src/router.rs` gated sub-router | `auth_off_by_default_list_still_open`, `auth_required_list_rejects_missing_bearer`, `auth_required_list_rejects_wrong_bearer`, `auth_required_list_accepts_correct_bearer`, `auth_required_copy_rejects_missing_bearer`, `auth_required_health_root_and_api_still_open` |
| Boot emits WARN when listener is non-loopback AND no bearer token is configured AND `security.trust_network=false` | `src/main.rs` `warn_if_unauth_non_loopback` | manual — inspect boot log |

### 2.4 Fleet-stronghold adoptions (landed 2026-09-13)

| Invariant | Enforced by | Regression test |
|---|---|---|
| Five default security headers on every response (CSP, HSTS, X-Frame-Options, X-Content-Type-Options, Referrer-Policy) — §5.1 | `src/security_headers.rs` middleware | `every_response_carries_security_headers` |
| W3C `traceparent` + `x-trace-id` on every response; inbound `traceparent` trace_id is echoed, otherwise synthesised — §1.6 / O1 | `src/trace_headers.rs` middleware | `every_response_carries_traceparent_and_x_trace_id`, `traceparent_inbound_id_is_echoed`, plus unit tests in the module |
| `FILEFERRY_OFFLINE=1` (or `true`/`yes`) replaces the S3 backend with `OfflineBackend` that fails every call with `Upstream("offline mode: ...")`. FS backend unaffected — §9.1 | `src/backend/offline.rs`, `src/main.rs` wiring | `offline_env_recognises_common_truthy_values`, `offline_backend_lists_returns_upstream_error`, `offline_backend_open_read_returns_upstream_error` |

If a reviewer or audit asks you to "just relax" one of these, push
back — they were designed to catch attempts to remove them.

## 3. Breaking changes vs `0.1.3-alpha` (in-progress `[Unreleased]`)

If you are upgrading callers, custom backends, or CI configs from
`0.1.3-alpha`, these are the seams to check. Full narrative:
`CHANGELOG.md` `[Unreleased]`.

| # | Change | Grep to find affected sites | Fix |
|---|---|---|---|
| 1 | Bare `.` or explicit `./`, `/.`, `a/./b`, `foo/.` in `sourceFilePath` / `destinationFilePath` / `startAfter` → 400 `invalid_path` (was 500 `os error 21` for `.`) | Client code that uses path components equal to `.` | Never send a bare `.` or a `.` segment; use a real filename |
| 2 | `Query<T>` / `Json<T>` failures + oversize body return **`application/json` `{error, message}`** (was `text/plain`) | Client code that parses bare-text 4xx bodies | Switch to JSON parsing on 4xx / 413. Codes: `bad_query`, `bad_body`, `body_too_large`, `invalid_path`, `unauthorized`, `not_found`, `same_storage_type`, `backend_not_configured`, `list_limit_too_large`, `transfer_too_large`, `upstream_error`, `io_error`, `internal_error` |
| 3 | `CopyFileRequest` / `ListFilesQuery` now `deny_unknown_fields` | Client code that sends extra fields "just in case" | Remove the extra fields; the server rejects with 4xx |
| 4 | `NotFound` response body no longer echoes the requested path | Client code that greps the message for the file name | Read `error == "not_found"` from the JSON body instead |
| 5 | Optional bearer gate: when `security.inter_service_token_env` is set, `/v1/files*` require `Authorization: Bearer <token>`; missing/wrong = 401 `unauthorized`. `/`, `/health`, `/api` stay public. | `grep -n 'security:' fileferry.yaml your-configs/` | Wire the token via env var; document `trust_network=true` only if a reverse proxy authenticates first |
| 6 | Response now carries `traceparent` + `x-trace-id` + 5 security headers | Client tests that assert exact response-header set | Update assertions; treat these as always-present |
| 7 | Copy-audit log line renamed `copy complete` → `file_transfer_completed`; `source_path` / `destination_path` fields replaced with `source_path_hash` / `destination_path_hash` (SHA-256 first 12 hex); new `duration_ms`, `outcome` fields | Log-shipping pipelines / SIEM rules that grep for `copy complete` or the raw path | Update parsers to the new event name + hashed fields |
| 8 | `docker-compose.yml` service is now `read_only: true`; data path is a named volume `fileferry_data` at `/app/data` | Compose overrides that assumed a writable rootfs | Use the named volume (default) or replace the `volumes:` entry with a `- ./data:/app/data:rw` bind mount |
| 9 | `FILEFERRY_OFFLINE=1` disables S3 outbound — every S3 call returns `Upstream("offline mode: ...")` | Pentest / break-test runners that expect FileFerry to reach real S3 | Explicitly unset the env var; or leave set on purpose to prevent live-S3 traffic |

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
- `src/main.rs` — boot sequence: config load → diagnose → backend
  init (offline check here) → boot WARNs → serve. Version bumps and
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
  dispatch the publish workflow.** Releases are cut by the
  maintainer as a dedicated `chore(release)` commit merged from a
  `release/*` branch, and the maintainer approves the deploy in the
  `production` GH Environment. **Agents do neither on their own.**
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
  alpha tag) and `ghcr.io/turnerrainer/fileferry:alpha`. Immutable
  digest tags: `:0.1.3-alpha`. The next release will add its own
  immutable digest; the maintainer will decide the version bump.
