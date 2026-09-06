# CLAUDE.md — orientation for LLM contributors

Read this file before you touch anything in `turnerrainer/fileferry`. It
is the agent-facing brief: what to run, what to preserve, and how to
spot configs that break under the current release.

- **Current release**: `0.1.3-alpha`
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

Baseline: 50/50 tests pass; clippy, audit, and deny all clean. If your
change reduces the test count or introduces a warning, that is a
regression — fix it before opening the PR.

## 2. Do-not-break invariants (0.1.3-alpha)

These are audit-driven (`h2ck.me/FileFerry/v1/AUDIT.md`). Removing or
weakening any of them is a regression, and the accompanying regression
tests will fire — read the test first, then decide.

| Invariant | Enforced by | Regression test |
|---|---|---|
| FS backend refuses symlinks; opens files with `O_NOFOLLOW` | `src/backend/fs.rs` `resolve()`, `open_read`, `write_all` | `tests/fs_symlink_*` |
| `S3Config` masks `access_key_id` + `secret_access_key` in `Debug` (and via `AppConfig` composition) | `src/config.rs` hand-written `impl fmt::Debug for S3Config` | `debug_of_s3config_never_contains_secrets`, `debug_of_appconfig_containing_s3_masks_secrets` |
| `stream_copy` aborts on per-poll inactivity (`limits.copy_inactivity_secs`, default 30 s) | `src/backend/mod.rs` `stream_copy` + `TimeoutReader` | see `copy_inactivity_*` tests |
| `GET /v1/files` is paginated: default `DEFAULT_LIST_LIMIT=1000`, cap `MAX_LIST_LIMIT=10_000`, over-cap → 413, `meta.nextCursor` on full pages | `src/backend/mod.rs`, `src/router.rs` | see list-pagination tests |
| `S3Config::validate()` refuses `bucket_path` with `..` or leading `/` | `src/config.rs` | `bucket_path_traversal_rejected`, `bucket_path_leading_slash_rejected` |
| Non-empty `cors_origin` is a **hard boot failure** (no silent-drop) | `src/config.rs` `from_yaml()` bail | `cors_origin_set_causes_startup_failure` |
| `resolve()` error message does not echo the user-supplied path (fixed string, full input goes to `tracing::warn!` only) | `src/backend/fs.rs` | `resolve_error_does_not_echo_input` |

If a reviewer or audit asks you to "just relax" one of these, push
back — they are v1 audit fixes and the compensating tests were designed
to catch attempts to remove them.

## 3. Breaking changes vs `0.1.0-alpha.2`

If you are upgrading callers, custom backends, or CI configs from
`0.1.0-alpha.2`, these are the seams to check. Full narrative:
`CHANGELOG.md` `[0.1.3-alpha]`.

| # | Change | Grep to find affected sites | Fix |
|---|---|---|---|
| 1 | `Backend::list` now takes `ListOptions { limit, start_after }` | `grep -rn 'fn list' src/backend/ path/to/your/backend` | Update trait impl; pass `ListOptions::default()` for the old "list everything" call site (still capped at `DEFAULT_LIST_LIMIT`) |
| 2 | `cors_origin: "..."` in YAML → refuses to boot | `grep -n 'cors_origin' fileferry.yaml your-configs/` | Leave empty; terminate CORS at reverse proxy (see `SECURITY.md`) |
| 3 | `bucket_path` with `..` or leading `/` → refuses to boot | `grep -n 'bucket_path' fileferry.yaml your-configs/` | Rewrite as a plain `foo/bar/` prefix without leading slash and without traversal |
| 4 | Versioning scheme: alphas are bare `-alpha` on the PATCH digit (`0.1.1-alpha`, `0.1.2-alpha`, `0.1.3-alpha`) | `grep -Rn 'alpha\.[0-9]' .` | Do not use `-alpha.N`; increment PATCH and keep the suffix `-alpha` |
| 5 | Publish workflow gated on `production` GitHub Environment | GH → Settings → Environments → `production` | Add required reviewers; tag push alone no longer ships an image |
| 6 | New optional limit `copy_inactivity_secs` (default 30) | `grep -n 'copy_inactivity_secs' fileferry.yaml` | Only override if legitimate slow streams need > 30 s |
| 7 | List responses gain `meta.nextCursor` and accept `?limit=` / `?startAfter=` | Client code that calls `GET /v1/files` | Wire the cursor loop; expect 413 for `limit > 10_000` |

## 4. Config search order & best-practice values

Config resolves in this order (first hit wins):

1. `--config <path>` CLI arg
2. `FILEFERRY_CONFIG` env var
3. `./fileferry.yaml`
4. Built-in defaults (FS backend at `./data`, S3 disabled)

Recommended production config:

```yaml
port: 8080
documentation_enabled: true      # /api summary; safe to leave on
cors_origin: ""                  # MUST stay empty — CORS at reverse proxy
fs:
  data_directory: /app/data      # bind-mount, writable by uid 1000
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
```

Secrets **must** come from env vars named by the config; never inline
them in YAML. `deny_unknown_fields` is set on every YAML shape — typos
like `porrt: 9000` fail to parse rather than silently apply the default.

Container posture (shipped `docker-compose.yml` sets all of this — do
not weaken it): `no-new-privileges: true`, `cap_drop: ALL`, uid 1000
non-root, private `tmpfs /tmp` (needed because S3 upload buffers to
`$TMPDIR`).

## 5. Repo landmarks

- `src/config.rs` — YAML → `AppConfig`; also the boot-time policy
  gates (F5 bucket_path, F6 CORS). Every rejection has an explicit
  regression test.
- `src/backend/mod.rs` — `Backend` trait, `ListOptions`,
  `stream_copy`, `LimitedReader`, `TimeoutReader`.
- `src/backend/fs.rs` — FS backend + symlink policy (F1).
- `src/backend/s3.rs` — S3 backend + `start_after_key` derivation.
- `src/router.rs` — HTTP surface (`/`, `/health`, `/api`,
  `/v1/files`, `/v1/files/copy`).
- `src/validate.rs` — path whitelist regex + `..` segment ban.
- `tests/` — integration + regression tests grouped by finding ID.
- `.github/workflows/publish.yml` — release pipeline; **gated on the
  `production` GitHub Environment**. Read the top-of-file comment
  before editing.
- `book/src/` — user-facing docs; `configuration.md` is the long-form
  reference.

## 6. Working rules for agents

- Sequential tool use; do not parallelise dependent operations.
- Prefer editing existing files (`CHANGELOG.md`, `book/src/*.md`)
  over creating new ones.
- When you change one of N parallel sites (e.g. one `Backend` impl,
  one router handler), grep for the pattern and verify every site.
  Bugs at seams passed the previous audit round because a caller was
  missed.
- Write break-the-fix tests, not confirm-the-fix tests. If you fix
  the FS backend to reject `..`, add a test that tries `foo/..`,
  `..`, `.`, and encoded variants — not just one that asserts the
  happy path.
- Never bump the top-level project version or push a tag as part of
  a feature PR. Releases are cut by the maintainer as a dedicated
  `chore(release)` commit merged from a `release/*` branch.

## 7. Where to find external context

- **h2ck.me security audits** — private GitHub org
  `github.com/h2ckme/FileFerry/v1/{AUDIT.md,FIX-KIT.md,PR-REVIEWS/}`.
  Access via org membership; the fleet-wide index is
  `h2ckme/security-fleet/REVIEW-INDEX.md`.
- **Original S3-Ferry** — <https://github.com/buerokratt/S3-Ferry>.
- **Book (rendered)** — <https://turnerrainer.github.io/fileferry/>.
- **Images** — `docker.io/turnerrainer/fileferry:alpha` (moving
  alpha tag) and `ghcr.io/turnerrainer/fileferry:alpha`. Immutable
  digest tags: `:0.1.3-alpha`.
