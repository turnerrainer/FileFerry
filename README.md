# FileFerry

A small HTTP file-transfer proxy. Rust re-implementation of
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).

**Version:** 0.2.1-alpha · **License:** Apache-2.0
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
> `0.2.1-alpha`; the sections below cover both the feature move
> AND the base-image patch.

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
