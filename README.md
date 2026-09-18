# FileFerry

[![Latest release](https://img.shields.io/github/v/release/turnerrainer/FileFerry?include_prereleases&sort=semver&label=release&color=blue)](https://github.com/turnerrainer/FileFerry/releases)
[![Release date](https://img.shields.io/github/release-date-pre/turnerrainer/FileFerry?label=released&color=blue)](https://github.com/turnerrainer/FileFerry/releases)
[![License](https://img.shields.io/github/license/turnerrainer/FileFerry?color=blue)](./LICENSE)
[![Container images](https://img.shields.io/badge/images-ghcr%20%7C%20docker.io-blue)](https://github.com/turnerrainer/FileFerry/pkgs/container/fileferry)

A small HTTP file-transfer proxy. Rust re-implementation of
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).

**Version:** 0.2.2-alpha · **License:** Apache-2.0
· **Docs:** [turnerrainer.github.io/fileferry](https://turnerrainer.github.io/fileferry/)
· **Images:** `docker.io/turnerrainer/fileferry:alpha`, `ghcr.io/turnerrainer/fileferry:alpha`

Point FileFerry at a local directory + (optionally) an S3 bucket
and it exposes four HTTP endpoints for listing and copying files
between the two.

## One-command demo

```bash
docker run -d --name fileferry -p 8080:8080 turnerrainer/fileferry:alpha
curl -s http://localhost:8080/health
```

Response:

```json
{"status":"ok"}
```

## ⚠️ Security posture — read before exposing

**FileFerry ships with no built-in authentication on the file-transfer
endpoints.** The demo command above binds to `0.0.0.0:8080`; on any
network where that port is reachable, `GET /v1/files` enumerates the
FS root or the S3 bucket and `POST /v1/files/copy` triggers backend
I/O (and, if S3 is wired, real S3 API calls + egress bandwidth billed
per-request). Two supported deployment models:

1. **Behind a reverse proxy** (default assumption — Ruuter, nginx,
   Caddy). Terminate auth + rate-limiting there; leave FileFerry on
   `127.0.0.1`. Set `security.trust_network: true` in
   `fileferry.yaml` to silence the boot WARN once the proxy is in
   place.
2. **Inter-service bearer token** (opt-in). Set
   `security.inter_service_token_env: FILEFERRY_INTER_SERVICE_TOKEN`
   in `fileferry.yaml`, then supply the token via the named env var.
   `GET /v1/files` and `POST /v1/files/copy` require
   `Authorization: Bearer <token>` (constant-time compared);
   `/` and `/health` remain open for liveness probes.

Also:

- **`/api` (OpenAPI recon endpoint) is off by default.** Set
  `FILEFERRY_ADMIN_ENABLED=1` at boot to serve it. When disabled
  it returns 404 (not 401 — the gate's existence itself doesn't
  leak).
- **CORS is not implemented in-process.** A non-empty
  `cors_origin` in `fileferry.yaml` is a hard boot failure —
  terminate CORS at the reverse proxy.
- **The shipped `docker-compose.yml` runs with
  `read_only: true` rootfs**, uid 1000, `no-new-privileges`,
  `cap_drop: [ALL]`, and a private `tmpfs /tmp`. Overrides that
  weaken these need explicit justification.

Full posture including disclosure policy: [`SECURITY.md`](./SECURITY.md).
Every gate above is regression-tested; the invariant tables in
[`CLAUDE.md`](./CLAUDE.md) §2 name the tests.

## Build from source

```bash
git clone -b dev https://github.com/turnerrainer/fileferry.git fileferry
cd fileferry
docker compose up -d --build
```

Or with cargo directly:

```bash
cargo build --release --bin fileferry
./target/release/fileferry
```

## Upgrading

> **Note on `0.2.0-alpha`.** That version was tagged but never
> shipped an image — Trivy blocked the build on 12 Debian
> base-image CVEs. `0.2.1-alpha` cuts the same feature set on a
> patched Dockerfile. Consumers should skip straight to
> `0.2.2-alpha`; the sections below cover the feature move, the
> base-image patch, AND the `0.2.2-alpha` hardening pass.

### From `0.2.1-alpha` → `0.2.2-alpha`

Post-audit hardening pass; every residual on the h2ck.me v1
backlog is now closed. Read the [`0.2.2-alpha` entry in
`CHANGELOG.md`](./CHANGELOG.md) for the full narrative. New
seams to check:

- **`GET /api` now defaults to 404.** Set
  `FILEFERRY_ADMIN_ENABLED=1` at boot to serve it. When the
  env-gate is off the endpoint returns 404 (not 401) — never
  reveals the gate's existence.
- **New `fileferry doctor` subcommand.** `fileferry doctor
  [--config <path>]` prints a green/amber report and exits
  `0` (clean), `1` (WARNs surfaced), or `2` (config invalid).
  Bearer tokens render as `SET (masked)`; the raw value never
  lands in the report. Safe to run in CI.
- **Numbered boot-time WARN block (`W-1`..`W-6`).** New
  preflight checks with stable ids for log-alert pipelines:
  unauth non-loopback bind, `trust_network=true` without a
  token, admin recon endpoint on public bind,
  `documentation_enabled: true` on public bind,
  `copy_inactivity_secs > 300`, `max_request_bytes > 100 MiB`.
  Each check has a legitimate override — WARN only, not a
  hard fail.
- **HTTP 408 `body_read_timeout` is now a possible response
  code** on `POST /v1/files/copy` — a slow-drip peer that
  stalls past 30 s of body read gets a structured 408 instead
  of holding the connection until the total-request timeout.
- **Error-message bodies are capped at 256 characters.**
  Clients that used to see the offending value echoed back
  verbatim now see a truncated form ending in `...`.
- **Graceful shutdown on SIGTERM / SIGINT.** In-flight
  transfers under `docker stop` / K8s rolling-restart now
  complete cleanly (up to Docker's grace window) instead of
  dying half-way with no audit-log line.
- **Log-alert rules can key on the shutdown INFO line.** Boot
  emits `graceful shutdown initiated` naming the signal
  (SIGTERM / SIGINT).

### From `0.1.3-alpha` → `0.2.1-alpha`

`0.2.1-alpha` MINOR-bumps to signal a new `security:` config
axis, a new `FILEFERRY_OFFLINE` runtime axis, and breaking wire
changes to the audit-log line and to extractor-error response
bodies. Read the [`0.2.1-alpha` entry in
`CHANGELOG.md`](./CHANGELOG.md) before you bump;
[`CLAUDE.md`](./CLAUDE.md) §3 has a nine-item grep cheat sheet.

Common upgrade seams:

- **Log shippers / SIEM.** The copy-audit line is now
  `file_transfer_completed` (was `copy complete`) and carries
  `source_path_hash` / `destination_path_hash` (first 12 hex of
  SHA-256) instead of raw paths. Grep rules that key on the old
  event name or on tenant identifiers in paths must update.
- **Clients that parse 4xx bodies.** Malformed queries and
  malformed / oversize bodies now return
  `application/json` with structured `error` codes
  (`bad_query`, `bad_body`, `body_too_large`, `unauthorized`,
  `list_limit_too_large`). Bare-text 4xx / 413 responses are
  gone.
- **`NotFound` message no longer echoes the requested path.**
  Read `error == "not_found"` from the JSON body instead.
- **Junk fields in a copy request or list query now 4xx.**
  Remove any "just-in-case" extra fields; the DTOs use
  `#[serde(deny_unknown_fields)]`.
- **Compose rootfs is now `read_only: true`.** Data lives on a
  named volume `fileferry_data:/app/data:rw`. Overrides that
  assumed a writable rootfs must switch to the named volume or
  replace it with `- ./data:/app/data:rw`.
- **Boot may WARN on non-loopback bind without auth.** Configure
  `security.inter_service_token_env` or set
  `security.trust_network=true` to silence it. Backwards-
  compatible; deployments keep booting.

### From `0.1.0-alpha.2` → `0.2.1-alpha`

Combine the notes above with the `0.1.3-alpha` breaking-config
seams that also still apply (also documented in
[`CHANGELOG.md`](./CHANGELOG.md)):

- Container refuses to boot with `config.cors_origin is set but CORS
  is not implemented` → clear `cors_origin` and terminate CORS at your
  reverse proxy.
- Boot fails on `config.s3.bucket_path may not contain '..'` or
  `may not start with '/'` → rewrite the prefix (e.g. `prefix/`).
- `Backend::list` signature changed to take a `ListOptions { limit,
  start_after }` — out-of-tree implementations must match.
- Publish workflow now runs unmanned on merge of a version-bump PR
  into `dev` — no Actions-UI approval step. The PR review + merge
  is the deploy approval; workflow-internal gates (Trivy on
  HIGH/CRITICAL, `/health` smoke test, cosign signing,
  Release-verify) are the safety net.

## Documentation

- **Book** — [turnerrainer.github.io/fileferry](https://turnerrainer.github.io/fileferry/)
  (getting started, config, failure modes)
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what FileFerry does and why
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md)
- **LLM contributor brief** — [`CLAUDE.md`](./CLAUDE.md) — invariants,
  breaking-change locators, and best-practice config for automated agents
- **Original S3-Ferry** — <https://github.com/buerokratt/S3-Ferry>
