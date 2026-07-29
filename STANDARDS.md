# Standards for FileFerry

This document pins the authoritative ruleset for this project and
lists project-specific extras. **The authoritative source is
[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)** — every rule
below either quotes or points at that file. Where they conflict,
the parent document wins.

## 0. Product identity

| Variable          | Value                                    |
|-------------------|------------------------------------------|
| Product name      | `FileFerry`                              |
| Cargo crate name  | `fileferry`                              |
| Binary name       | `fileferry`                              |
| GitHub repo       | `github.com/turnerrainer/FileFerry`      |
| Docker Hub image  | `turnerrainer/fileferry`                 |
| GHCR image        | `ghcr.io/turnerrainer/fileferry`         |
| License           | Apache-2.0                               |
| Book title        | `FileFerry`                              |
| First stable      | `v1.0.0` on `main`                       |
| Author            | Rainer Türner                            |
| Namespace         | `Buerostack/FileFerry`                   |

## 1. Repository layout

Per DEV-REQUIREMENTS §1. This project's tree:

```
FileFerry/
├── Cargo.toml
├── Cargo.lock                  # tracked (binary crate)
├── VERSION
├── README.md
├── CHANGELOG.md
├── SECURITY.md
├── HANDOFF.md
├── LICENSE
├── NOTICE
├── STANDARDS.md                # this file
├── Dockerfile
├── docker-compose.yml
├── fileferry.yaml              # default runtime config
├── .gitignore
├── .dockerignore
├── deny.toml
├── .cargo/audit.toml
├── src/                        # Rust source (main + lib + modules)
├── tests/                      # integration tests (http_api.rs)
├── book/                       # mdBook — 5 required chapters per DEV-REQ §4.1
├── docs/DESIGN.md              # domain design
├── tasks/{backlog,done}/       # sequential task IDs
└── .github/workflows/          # tests.yml, security.yml, publish.yml, docs.yml
```

## 2. Rust conventions

Per DEV-REQUIREMENTS §2. **Deviation**: MSRV is `1.94` (not `1.88`)
because the transitive `aws-*` crates require `rustc >= 1.94.1`
starting with aws-sdk-s3 1.140. Documented on first release.

Beyond the parent doc:

- **HTTP framework**: `axum` 0.7+, `tower-http` layers for body-cap
  and timeout.
- **Storage abstraction**: `Backend` trait in `src/backend/mod.rs`
  with two concrete impls: `FsBackend`, `S3Backend`. Adding a new
  backend = one file + a `Backends::pick` arm.
- **AWS SDK**: `aws-sdk-s3` 1.x with `behavior-version-latest` and
  `rustls` (avoids double-linking OpenSSL alongside the rest of
  the stack).
- **Path validation**: whitelist regex + `..` segment ban. See
  `src/validate.rs`. Purposefully restrictive — this is a proxy,
  not a general filesystem shell.
- **Size caps**: `LimitedReader` wraps every stream copy. Total
  bytes moved is capped by `limits.max_response_bytes`; overrun
  aborts mid-stream (see the failure-modes chapter).

## 3. Testing tiers

Per DEV-REQUIREMENTS §3.

Baseline for the initial release (locked in HANDOFF.md, updated
per release):

- **Unit tests**: 17 (config, validate, backend/fs, backend/mod)
- **Integration tests**: 13 (tests/http_api.rs — router-level)
- **Total**: 30 pass / 0 fail / 0 ignored

**Explicit non-goals**:

- No live-S3 tests in the default suite. A future task adds a
  feature-gated `tests/s3_live.rs` that a CI job with a LocalStack
  sidecar can enable. Meantime the S3 code path is exercised
  via router integration tests that pass in a substitute
  `FsBackend` for the S3 slot — verifies the routing seam without
  needing an S3 wire.
- No line-coverage tracking. Per DEV-REQ §3, coverage is the wrong
  metric — path coverage via meaningful assertions is what we
  optimise for.

## 4. Documentation

Per DEV-REQUIREMENTS §4. This project's book is 5 chapters + 1
reference (min per §4.1):

```
book/src/
├── SUMMARY.md
├── introduction.md
├── getting-started.md
├── configuration.md
├── failure-modes.md
└── reference/
    └── changelog.md      # synced from root CHANGELOG.md in CI
```

CI pins `mdbook 0.4.40` + `mdbook-linkcheck 0.7.7` and installs
them fresh in the workflow. Newer mdbook releases (0.5.x) break
the linkcheck plugin — do not bump without simultaneously
verifying linkcheck compatibility.

## 5-9. Security, CI, containers, releases, registries

Delegated to DEV-REQUIREMENTS §5-9 without deviation.

## 10-15. Task tracking, git, secrets, audit discipline, chat style, memory

Delegated to DEV-REQUIREMENTS §10-15 without deviation.

## Change log for this document

- **2026-07-29** — Initial version. Extracted from XTR-on-Rust's
  STANDARDS.md, deltas for FileFerry: MSRV bumped to 1.94
  (AWS SDK constraint).
