# HANDOFF

**Written**: 2026-07-29
**Last verified green**: 2026-07-29 — cargo test 30/0/0
(17 unit + 13 integration); fmt + clippy -D warnings clean;
release binary builds; container image built locally and
`/health` responds on both amd64 and (untested here) arm64;
`mdbook build` produces the site (linkcheck run only in CI —
local mdbook is 0.5.x, incompatible with linkcheck 0.7.7; CI
pins mdbook 0.4.40 which does work).
**Branch**: `dev` — first commit staged locally.
**Release**: `v0.1.0-alpha.1` — **not yet tagged, not yet pushed**.
The GitHub repo has not been created either. See "Publishing"
below.

Next contributor (human or Claude) must:

1. Read [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)
   front-to-back before touching anything.
2. Read this file for FileFerry-specific state.
3. Run the verification set (below) — every command exits 0.

## What this repo IS today

Rust re-implementation of
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry) with
byte-identical HTTP surface + hardening on top.

- `GET /` — service banner
- `GET /health` — liveness probe (new vs S3-Ferry — needed for the
  container `HEALTHCHECK`)
- `GET /api` — static OpenAPI 3.1
- `GET /v1/files?type={FS,S3}` — root-level file listing
- `POST /v1/files/copy` — stream-copy across backends
- Two backends: `FsBackend` (local FS) + `S3Backend` (aws-sdk-s3)
- Trait-based `Backend` abstraction — adding a third backend is
  one new file + one arm in `Backends::pick`

## Verification set (all should exit 0)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release --bin fileferry
cargo test --no-fail-fast
cargo audit --deny warnings          # needs network, blocked in some sandboxes
mdbook build book                    # needs mdbook 0.4.40 + linkcheck 0.7.7
```

Live smoke:

```bash
docker build -t fileferry:local .
docker run -d --name fileferry -p 18080:8080 fileferry:local
curl -sf http://localhost:18080/health
curl -sf http://localhost:18080/
curl -sf 'http://localhost:18080/v1/files?type=FS'
docker rm -f fileferry
```

## Publishing (not done yet)

Everything up to publish is prepared. Actual push to Docker Hub +
GHCR is a **manual step** that requires:

1. **Create the GitHub repo** — `gh repo create turnerrainer/fileferry --public`
2. **Push the local branch** — `git push -u origin dev`
3. **Enable Actions permissions** — `gh api repos/turnerrainer/fileferry/actions/permissions/workflow -X PUT -F 'default_workflow_permissions=write' -F 'can_approve_pull_request_reviews=false'`
4. **Enable Pages** — `gh api repos/turnerrainer/fileferry/pages -X POST -f 'build_type=workflow'`
5. **Create the Docker Hub repo** — Hub UI → New → `turnerrainer/fileferry` → public
6. **Generate a scoped Docker Hub token** and set repo secrets:
   - `DOCKERHUB_USERNAME` = `turnerrainer`
   - `DOCKERHUB_TOKEN` = the token (via `gh secret set` from stdin)
7. **Cut the release tag** — `git tag -a v0.1.0-alpha.1 -m "FileFerry v0.1.0-alpha.1"` + `git push origin v0.1.0-alpha.1`
8. **Link the GHCR package** after the first publish succeeds
   (Package settings → Manage Actions access → link repo with
   Write role)

See [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) §9 for the
full recipe, and XTR-on-Rust's `HANDOFF.md` for a worked example
of the same procedure completed.

## Roadmap

Landed on this release:

- ✅ Task 001 — domain deep-dive on S3-Ferry (see `tasks/done/`)

Open backlog:

| Task | Location | Notes |
|---|---|---|
| 002 | `tasks/backlog/002-streaming-s3-upload.md` | Replace temp-file S3 write with `SdkBody::from_body_1_x` |
| 003 | `tasks/backlog/003-multipart-s3-upload.md` | Add multipart-upload for objects > 100 MiB |
| 004 | `tasks/backlog/004-localstack-live-s3-tests.md` | CI job with LocalStack sidecar exercising the S3 wire |
| 005 | `tasks/backlog/005-cors-enforcement.md` | Wire `cors_origin` into `tower-http::cors` |

## Known gaps vs XTR/Ruuter reference implementations

- **`cargo audit` not verified locally.** The RustSec advisory
  git remote is blocked in the environment this scaffold was
  authored in. CI runs it on every push — see
  `.github/workflows/security.yml`.
- **`mdbook build` linkcheck not verified locally.** Local
  mdbook is 0.5.4, which is incompatible with mdbook-linkcheck
  0.7.7 (the plugin targets mdbook 0.4.x). CI installs the
  pinned pair and runs the check — see
  `.github/workflows/{tests,docs}.yml`.
- **Multi-arch smoke test not run locally.** Publish workflow
  handles it; local docker build only exercises amd64.

## For the next Claude session refactoring another core component

Everything you need is in these three files:

1. **[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)** — the
   ruleset. Non-negotiable unless a deviation is justified in
   the commit message.
2. **[`./docs/DESIGN.md`](./docs/DESIGN.md)** — reference example
   of a domain-design doc that documents preserves + changes
   from an upstream project.
3. **This FileFerry repo** — reference implementation of the
   "small proxy" shape (200 crates, 4 endpoints, 30 tests).

Common questions answered by files in this repo:

| Question | See |
|---|---|
| How do I structure `Cargo.toml`? | `Cargo.toml` |
| How does the multi-stage Dockerfile work? | `Dockerfile` |
| What goes in `docker-compose.yml`? | `docker-compose.yml` |
| What does `.github/workflows/*` look like? | `.github/workflows/` |
| How do I structure a task file? | `tasks/done/001-domain-deep-dive-s3-ferry.md` |
| How do I structure the book? | `book/src/` |
| How is CHANGELOG formatted? | `CHANGELOG.md` |

## Where to look for more detail

| Topic | File |
|---|---|
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (FileFerry-specific) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |
