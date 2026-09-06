# Security policy

## Reporting a vulnerability

Please **do not open a public GitHub issue** for security-sensitive
findings. Instead:

1. **Preferred**: use GitHub's private vulnerability reporting for
   this repo — Security tab → **Report a vulnerability**. That
   routes the report to maintainers via a private thread with
   tracking.
2. **Fallback**: email `rainer.turner@gmail.com` with `[FileFerry-security]`
   in the subject line.

Include, when you can:

- Affected version (image tag or git ref)
- Reproduction steps or PoC
- Impact assessment (what an attacker gains)
- Any suggested mitigation

## Response commitments

- **Acknowledgement**: within 3 business days of the report reaching
  a maintainer.
- **Triage decision** (accepted / needs-more-info / not-a-vuln):
  within 7 business days.
- **Fix + coordinated disclosure**: target 30 days for CRITICAL and
  HIGH severity, 90 days for MEDIUM. Extension is negotiable if a
  fix requires a coordinated upstream change.
- **Credit**: reporters are credited in the release notes unless
  they ask to remain anonymous.

## Supported versions

Only the latest published release receives security fixes.
FileFerry is pre-1.0 and follows SemVer — minor bumps are the
norm, patch releases are cut only for critical fixes on the
current line.

| Version   | Support status                             |
|-----------|--------------------------------------------|
| `0.1.x`   | ✅ Supported (current line)                |
| `< 0.1.0` | n/a                                        |

## What we do to reduce supply-chain risk

Every rule below is documented in
[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) §5 and mirrored
in this project's [`STANDARDS.md`](./STANDARDS.md).

- **`cargo audit --deny warnings`** — every push, every PR, daily at
  06:00 UTC. Advisory exceptions live in `.cargo/audit.toml` with a
  rationale and a review date; blind ignores are a code smell.
- **`cargo deny check all`** — enforces license allow-list
  (Apache-2.0 compatible only, no GPL/AGPL/SSPL), refuses git-URL
  deps and wildcard version specs, warns on duplicate crate
  versions. Config: [`deny.toml`](./deny.toml).
- **Trivy image scan** on every release-tag publish, gated on
  `HIGH` and `CRITICAL` fixed vulnerabilities. Blocks signing.
- **cosign keyless signatures** on every published image digest
  via Sigstore OIDC.
- **In-toto provenance + SPDX SBOM** attached to every multi-arch
  manifest.
- **Reproducible image layer timestamps** (`SOURCE_DATE_EPOCH` +
  `rewrite-timestamp=true`) so the same commit produces the same
  image digest.
- **Multi-arch smoke test** — every release image is booted under
  QEMU on both `linux/amd64` and `linux/arm64` and probed with
  `/health` before it's signed. A signed image is a working image.
- **Non-root container user** (uid 1000), `cap_drop: ALL`,
  `no-new-privileges: true` in the shipped `docker-compose.yml`.

## What is out of scope

FileFerry is a broker, not a policy enforcement point. The
operator is responsible for:

- Authentication / authorisation of callers (terminate at a
  reverse proxy or API gateway before FileFerry)
- Rate limiting (same)
- TLS termination
- Secret fetching (Vault / KMS / Docker secrets); FileFerry only
  reads secrets from named env vars — never from disk
- Persistent state / cross-replica coordination
- IAM policy on the S3 bucket
- File-content scanning (virus / DLP)

## Operational hardening notes

These are properties operators should understand about the current
release. Each was reviewed in the v1 audit (`h2ck.me/projects/FileFerry/v1/AUDIT.md`).

- **Filesystem symlinks are refused outright.** The FS backend
  rejects any path whose final component is a symlink and opens
  files with `O_NOFOLLOW`. Operators do not need to sanitise the
  data directory before mounting.
- **AWS credentials never appear in logs.** `S3Config` carries a
  hand-written `Debug` impl that redacts the access key and
  secret; any `tracing::error!(?state, ...)` prints `***REDACTED***`.
- **Slow-drip transfers time out.** `stream_copy` wraps the source
  reader in a per-poll timeout (`limits.copy_inactivity_secs`,
  default 30s). A stalled peer aborts within the budget instead of
  holding a request slot for the full `request_timeout_secs`.
- **List endpoints are paginated.** `GET /v1/files` returns at most
  `MAX_LIST_LIMIT` (10 000) entries per request; larger `?limit=`
  values return 413. Clients paginate via `?startAfter=<name>`
  taken from `meta.nextCursor`.
- **`bucket_path` is validated at boot.** Values containing `..`
  or a leading `/` are refused. S3 keys are flat, so this is
  defence-in-depth against future consumers that treat the prefix
  as a filesystem path.
- **CORS is not implemented.** Setting `config.cors_origin` is a
  hard boot failure to prevent silent-drop behaviour that pushed
  operators toward worse workarounds. Terminate CORS at your
  reverse proxy.
- **S3 uploads are buffered to `$TMPDIR` before transmission.** The
  temporary file is 0600 and named per-process by the `tempfile`
  crate, but on a shared multi-tenant host other UIDs can observe
  its existence. Run FileFerry in a container with a private
  `/tmp` (the shipped `docker-compose.yml` does this by default).
  A future release will switch to chunked `PutObject` to
  eliminate the tempfile entirely.
- **Response-body caps bound wire bytes.** FileFerry does not
  enable transparent decompression on any HTTP client, so a
  compressed source cannot amplify past the configured cap. If a
  future change enables client-side gzip decoding, revisit
  `LimitedReader` placement to keep the cap on decoded bytes.
