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
```

## Field reference

### Top-level

| Field                  | Type    | Default | Notes                                                           |
|------------------------|---------|---------|-----------------------------------------------------------------|
| `port`                 | integer | `8080`  | TCP port for the HTTP server                                    |
| `documentation_enabled`| bool    | `true`  | If false, `GET /api` returns 404                                |
| `cors_origin`          | string  | `""`    | Reserved. Not enforced yet — a future release will add tower-http CORS |

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
| `max_request_bytes`     | u64    | `33554432`     | 32 MiB. Caps inbound HTTP body via `tower-http` request-body limit. |
| `max_response_bytes`    | u64    | `5368709120`   | 5 GiB. Caps the total bytes moved through `stream_copy`. Exceeding it aborts the transfer with `500 io_error` mid-stream. |
| `request_timeout_secs`  | u64    | `300`          | 5 min. Per-request timeout enforced by `tower-http`.           |

## Environment overrides

Only credentials are read from env vars, and only the ones you name
in the config file. There is no "override any field from an env
var" behaviour — that ambiguity was the source of the
`API_CORS_ORIGIN=` gotcha in S3-Ferry.

The special env var `FILEFERRY_CONFIG` selects which YAML file to
load; it does not change any values inside the file.

The `RUST_LOG` env var controls log verbosity via
[`tracing_subscriber::EnvFilter`](https://docs.rs/tracing-subscriber).
Default: `info,fileferry=info`.
