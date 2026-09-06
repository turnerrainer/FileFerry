# FileFerry

A small HTTP file-transfer proxy. Rust re-implementation of
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).

**Version:** 0.1.3-alpha · **License:** Apache-2.0
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

## Documentation

- **Book** — [turnerrainer.github.io/fileferry](https://turnerrainer.github.io/fileferry/)
  (getting started, config, failure modes)
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what FileFerry does and why
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md)
- **Original S3-Ferry** — <https://github.com/buerokratt/S3-Ferry>
