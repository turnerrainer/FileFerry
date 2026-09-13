# Configuration

FileFerry reads a single YAML file at boot. The search order is:

1. `--config <path>` command-line flag
2. `FILEFERRY_CONFIG` environment variable
3. `./fileferry.yaml` in the current working directory
4. Built-in defaults (see below)

If the config file is missing entirely, FileFerry starts with
default settings and only the FS backend enabled. If the config
file is present but references an env var that isn't set, FileFerry
**fails hard on boot** — never a silent downgrade.

## Complete reference

```yaml
# ------------------------------------------------------------------
# HTTP server
# ------------------------------------------------------------------
port: 8080                        # TCP port to bind
documentation_enabled: true       # serve GET /api (OpenAPI 3.1)
cors_origin: ""                   # must remain empty; a non-empty value is a hard boot failure (see SECURITY.md)

# ------------------------------------------------------------------
# Local filesystem backend (always enabled)
# ------------------------------------------------------------------
fs:
  data_directory: ./data          # every FS operation resolves here

# ------------------------------------------------------------------
# S3 (or S3-compatible) backend — OMIT this block to disable
# ------------------------------------------------------------------
s3:
  region: us-east-1
  endpoint_url: ""                # empty = default AWS endpoint; set for MinIO/LocalStack
  bucket: your-bucket
  bucket_path: ""                 # optional prefix inside the bucket
  access_key_id_env: FILEFERRY_S3_ACCESS_KEY_ID
  secret_access_key_env: FILEFERRY_S3_SECRET_ACCESS_KEY

# ------------------------------------------------------------------
# Resource ceilings
# ------------------------------------------------------------------
limits:
  max_request_bytes: 33554432     # 32 MiB — inbound HTTP body cap
  max_response_bytes: 5368709120  # 5 GiB  — per-transfer stream cap
  request_timeout_secs: 300       # 5 min  — per-request timeout
  copy_inactivity_secs: 30        # per-poll inactivity guard on the source reader (slow-drip DoS defence)

# ------------------------------------------------------------------
# Optional inter-service auth — OMIT the whole block to leave gated
# routes open (backwards-compatible with pre-auth deployments).
# ------------------------------------------------------------------
security:
  # Name of an env var whose value is the bearer token clients must
  # present as `Authorization: Bearer <token>` on `/v1/files*`.
  # `/`, `/health`, `/api` remain public regardless. Comparison is
  # constant-time; the token value is never logged.
  inter_service_token_env: FILEFERRY_INTER_SERVICE_TOKEN
  # true = suppress the boot WARN about an unauth non-loopback
  # listener. Set only when a reverse proxy / service mesh
  # authenticates every request before it reaches FileFerry.
  trust_network: false
```

## Field reference

### Top-level

| Field                  | Type    | Default | Notes                                                           |
|------------------------|---------|---------|-----------------------------------------------------------------|
| `port`                 | integer | `8080`  | TCP port for the HTTP server                                    |
| `documentation_enabled`| bool    | `true`  | If false, `GET /api` returns 404                                |
| `cors_origin`          | string  | `""`    | **Must remain empty.** A non-empty value is a hard boot failure (terminate CORS at your reverse proxy). |

### `fs`

| Field           | Type   | Default   | Notes                                              |
|-----------------|--------|-----------|----------------------------------------------------|
| `data_directory`| path   | `./data`  | Created on boot if missing. Must be writable by the runtime user. |

### `s3`

Omit the entire `s3:` block to disable the S3 backend.
Requests targeting `type=S3` will then return `503
Service Unavailable` with `error: "backend_not_configured"`.

| Field                    | Type   | Required | Notes                                                              |
|--------------------------|--------|----------|--------------------------------------------------------------------|
| `region`                 | string | yes      | AWS region name                                                    |
| `endpoint_url`           | string | no       | Override for MinIO / LocalStack / R2 / etc.                        |
| `bucket`                 | string | yes      | Target bucket                                                      |
| `bucket_path`            | string | no       | Prefix prepended to every key                                      |
| `access_key_id_env`      | string | yes      | Name of an env var containing the access key ID                    |
| `secret_access_key_env`  | string | yes      | Name of an env var containing the secret access key                |

Credentials **never** appear in the YAML file. FileFerry resolves
the env var by name at boot; if it is unset or empty, the process
exits with a clear error identifying the missing variable.

### `limits`

| Field                   | Type   | Default        | Notes                                                          |
|-------------------------|--------|----------------|----------------------------------------------------------------|
| `max_request_bytes`     | u64    | `33554432`     | 32 MiB. Caps inbound HTTP body via `DefaultBodyLimit`; oversize → `413 body_too_large` (structured JSON). |
| `max_response_bytes`    | u64    | `5368709120`   | 5 GiB. Caps the total bytes moved through `stream_copy`. Exceeding it aborts the transfer with `500 io_error` mid-stream. |
| `request_timeout_secs`  | u64    | `300`          | 5 min. Per-request timeout enforced by `tower-http`.           |
| `copy_inactivity_secs`  | u64    | `30`           | Per-poll inactivity guard on the source reader inside `stream_copy`. Aborts a slow-drip peer (e.g. 1 byte/minute) before it can hold a request slot for the full `request_timeout_secs`. |

### `security` (optional)

Omit the block entirely to leave `/v1/files*` open to any caller
(backwards-compatible with pre-auth deployments). When present, the
block gates the backend-touching routes; the always-open `/`, `/health`
and `/api` are unaffected.

| Field                       | Type   | Default | Notes                                                            |
|-----------------------------|--------|---------|------------------------------------------------------------------|
| `inter_service_token_env`   | string | *unset* | Name of an env var whose value is the required bearer token. When set (and non-empty), `/v1/files` and `/v1/files/copy` require `Authorization: Bearer <token>`. Comparison is constant-time via the `subtle` crate; the token is never logged. |
| `trust_network`             | bool   | `false` | Suppresses the boot WARN about an unauth non-loopback listener. Set to `true` only when a reverse proxy or service mesh already authenticates every request before it reaches FileFerry. |

When the block is present, missing or wrong `Authorization` headers
receive:

```
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{"error":"unauthorized","message":"missing bearer token"}
```

or `"invalid bearer token"` on mismatch. `/`, `/health`, and `/api`
always return without auth so a reverse-proxy liveness / discovery
flow keeps working.

## Environment overrides

Only credentials are read from env vars, and only the ones you name
in the config file. There is no "override any field from an env
var" behaviour — that ambiguity was the source of the
`API_CORS_ORIGIN=` gotcha in S3-Ferry.

The special env var `FILEFERRY_CONFIG` selects which YAML file to
load; it does not change any values inside the file.

| Env var                                                                       | Effect                                                                                                     |
|-------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------|
| `RUST_LOG`                                                                    | Controls log verbosity via [`tracing_subscriber::EnvFilter`](https://docs.rs/tracing-subscriber). Default: `info,fileferry=info`. |
| `LOG_ANSI`                                                                    | `1`/`true` → force ANSI colour codes on stderr. `0`/`false` → force off. Absent → auto-detect via `atty` (off under Docker / systemd). |
| `FILEFERRY_CONFIG`                                                            | Path to YAML config; higher precedence than `./fileferry.yaml`.                                            |
| `FILEFERRY_OFFLINE`                                                           | `1` / `true` / `yes` (case-insensitive) → swap the S3 backend for an offline stub that fails every S3 call with `Upstream("offline mode: outbound blocked by FILEFERRY_OFFLINE")`. FS backend is unaffected. Use during pentest / break-tests so FileFerry cannot accidentally hit real S3. |
| the env var named by `security.inter_service_token_env`                       | Value is the required bearer token.                                                                        |
| the env vars named by `s3.access_key_id_env` / `s3.secret_access_key_env`     | S3 credentials, resolved at boot; unset or empty = hard boot failure.                                      |
