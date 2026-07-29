# Getting started

Install, run, verify. One page.

## Prerequisites

One of:

- Docker 20.10+ (recommended), OR
- Rust 1.94+ (`rustup` install) if you want to build from source

## 1. Pull and run the image

```bash
docker run -d --name fileferry -p 8080:8080 \
  -v "$PWD/data:/app/data:rw" \
  turnerrainer/fileferry:rc
```

Confirm it's up:

```bash
curl -sf http://localhost:8080/health
```

Expected:

```json
{"status":"ok"}
```

## 2. List files

Create a sample file in the mounted `./data` directory and list it:

```bash
echo "hello world" > data/greeting.txt
curl -s 'http://localhost:8080/v1/files?type=FS'
```

Expected:

```json
{
  "data": [
    {"name": "greeting.txt", "size": 12, "lastModified": "2026-07-29T12:00:00Z"}
  ],
  "meta": {"count": 1}
}
```

## 3. Configure S3 (optional)

To also enable the S3 backend, mount a `fileferry.yaml` with an
`s3:` block and supply the credentials via environment variables:

```bash
docker run -d --name fileferry -p 8080:8080 \
  -v "$PWD/data:/app/data:rw" \
  -v "$PWD/fileferry.yaml:/app/fileferry.yaml:ro" \
  -e FILEFERRY_S3_ACCESS_KEY_ID="$MY_ACCESS_KEY" \
  -e FILEFERRY_S3_SECRET_ACCESS_KEY="$MY_SECRET_KEY" \
  turnerrainer/fileferry:rc
```

See [Configuration](./configuration.md) for every field, and
[Failure modes](./failure-modes.md) for what happens when the S3
block references an env var that isn't set (spoiler: hard-fail on
boot, never a silent downgrade).

## 4. Copy a file

Once both backends are up, copy the sample file up to S3:

```bash
curl -sX POST http://localhost:8080/v1/files/copy \
  -H 'content-type: application/json' \
  -d '{
    "sourceStorageType": "FS",
    "sourceFilePath": "greeting.txt",
    "destinationStorageType": "S3",
    "destinationFilePath": "greeting.txt"
  }'
```

Expected:

```
HTTP/1.1 204 No Content
```

And back down again:

```bash
curl -sX POST http://localhost:8080/v1/files/copy \
  -H 'content-type: application/json' \
  -d '{
    "sourceStorageType": "S3",
    "sourceFilePath": "greeting.txt",
    "destinationStorageType": "FS",
    "destinationFilePath": "greeting-copy.txt"
  }'
```

## 5. Build from source (alternative)

```bash
git clone -b dev https://github.com/turnerrainer/FileFerry.git fileferry
cd fileferry
cargo build --release --bin fileferry
./target/release/fileferry
```

Or via docker compose:

```bash
docker compose up -d --build
```

## What to read next

- [Configuration](./configuration.md) — every YAML field
- [Failure modes](./failure-modes.md) — every HTTP status + error code
- [Changelog](./reference/changelog.md)
