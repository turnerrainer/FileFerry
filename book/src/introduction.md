# FileFerry

**Version:** 0.1.0-rc.1 · **License:** Apache-2.0

FileFerry is a small HTTP service that brokers file transfers
between a local filesystem and an S3-compatible object store. It
is a Rust re-implementation of the JVM-based
[buerokratt/S3-Ferry](https://github.com/buerokratt/S3-Ferry).

## What it does

Four HTTP endpoints, no more:

| Endpoint            | Purpose                                                     |
|---------------------|-------------------------------------------------------------|
| `GET /`             | Service banner                                              |
| `GET /health`       | Liveness probe                                              |
| `GET /api`          | Static OpenAPI 3.1 summary of the surface                   |
| `GET /v1/files`     | List root-level files in a backend (`?type=FS` or `?type=S3`) |
| `POST /v1/files/copy` | Stream-copy a file between backends                       |

## One-command demo

```bash
docker run -d --name fileferry -p 8080:8080 turnerrainer/fileferry:rc
curl -s http://localhost:8080/health
```

Then upload a file into the mounted `./data` directory and list it:

```bash
echo "hello" > data/hello.txt   # mounted at /app/data inside the container
curl -s 'http://localhost:8080/v1/files?type=FS' | jq .
```

Copy it to S3 (once your S3 credentials are configured — see
[Configuration](./configuration.md)):

```bash
curl -sX POST http://localhost:8080/v1/files/copy \
  -H 'content-type: application/json' \
  -d '{
    "sourceStorageType": "FS",
    "sourceFilePath": "hello.txt",
    "destinationStorageType": "S3",
    "destinationFilePath": "hello.txt"
  }'
```

Response: `HTTP/1.1 204 No Content`.

## What it explicitly does NOT do

FileFerry brokers transfers. It does not:

- Authenticate or authorise callers (terminate at a reverse proxy)
- Provide rate limiting (same)
- Track file versions
- Do image/video/document processing
- Support nested directories in `list` (root-level only, matching
  S3-Ferry's behaviour)

For the exhaustive list see the [FailureModes](./failure-modes.md)
chapter and the roadmap in [`HANDOFF.md`](https://github.com/turnerrainer/FileFerry/blob/dev/HANDOFF.md).

## Where to next

- [Getting started](./getting-started.md) — install, run, verify
- [Configuration](./configuration.md) — every config field
- [Failure modes](./failure-modes.md) — status codes + error codes
