# Failure modes

Every non-2xx response FileFerry emits carries a JSON body of the
shape:

```json
{
  "error": "<machine-readable code>",
  "message": "<human-readable detail>"
}
```

The `error` code is stable — operators and monitoring can grep for
it. The `message` may change between releases.

Every 4xx and 5xx response — including the ones surfaced by the
axum extractor layer for malformed queries and oversize bodies —
carries the same structured JSON envelope. Clients can rely on
`Content-Type: application/json` and a stable `error` code on
every failing response; there are no bare `text/plain` errors.

## Status code + error code matrix

| HTTP  | `error` code               | Cause                                                                                                       |
|-------|----------------------------|-------------------------------------------------------------------------------------------------------------|
| `400` | `invalid_path`             | `sourceFilePath` / `destinationFilePath` / `startAfter` failed validation — see "Path validation" below     |
| `400` | `same_storage_type`        | Copy request where `sourceStorageType == destinationStorageType` — S3-Ferry parity, prevents no-op copies    |
| `400` | `bad_query`                | `?type=` outside `{FS,S3}`, malformed `?limit=`, or any other query-string deserialisation failure          |
| `400` | `bad_body`                 | Malformed JSON body, missing required field, or an unknown field (`deny_unknown_fields` is on)              |
| `401` | `unauthorized`             | `security.inter_service_token_env` is set and the request has no `Authorization: Bearer <token>` header, OR the token doesn't match. `/`, `/health`, `/api` are unaffected. |
| `404` | `not_found`                | Source file doesn't exist in the requested backend. The response `message` is a fixed string — the requested path is NOT echoed (full input is in the operator log). |
| `413` | `body_too_large`           | Inbound request body exceeded `limits.max_request_bytes`. Structured 413 emitted from `TypedJson` via `DefaultBodyLimit`. |
| `413` | `list_limit_too_large`     | `GET /v1/files?limit=<n>` where `n > MAX_LIST_LIMIT` (`10_000`)                                              |
| `500` | `io_error`                 | Filesystem I/O failed mid-transfer, OR the transfer exceeded `limits.max_response_bytes` (LimitedReader)     |
| `500` | `internal_error`           | Bug in FileFerry — always paired with a WARN log line. File a task with the log snippet.                     |
| `502` | `upstream_error`           | S3 returned an error other than `NoSuchKey` (network, auth, quota, throttling); also emitted by the offline stub when `FILEFERRY_OFFLINE` is set. |
| `503` | `backend_not_configured`   | Request targets `S3` but no `s3:` block is configured in `fileferry.yaml`                                    |

## Path validation

FileFerry rejects any path that:

- is empty
- contains a null byte (`\0`)
- contains any character outside `[0-9 a-z A-Z - . _ /]`
- contains a segment equal to `.` or `..` (traversal / self-reference)

The bare-`.` rejection was added in the post-0.1.3-alpha hardening
pass; before that, `sourceFilePath: "."` resolved to the data
directory itself and leaked `500 io_error: Is a directory (os
error 21)` back to the caller. The validator now catches `.` and
`./`, `/.`, `a/./b`, `foo/.`, `./foo` alongside the existing `..`
patterns. `.hidden` / `..hidden` **filenames** stay valid — the
check is segment-exact, not prefix.

The whitelist is deliberately restrictive — a file-transfer proxy
has no legitimate use for spaces, unicode filenames, backslashes,
or `@` / `+` / `#`. It eliminates a large class of
encoding-mismatch and injection risks at the boundary.

If you need to broker files whose names violate this rule, rename
them on the source side before submitting the copy request.

## Size caps

Two independent caps:

| Cap                          | Enforced by                                    | Fires as                                                              |
|------------------------------|------------------------------------------------|-----------------------------------------------------------------------|
| `limits.max_request_bytes`   | `DefaultBodyLimit` + `TypedJson` (extractor)   | `413 body_too_large` (structured JSON) before the handler runs        |
| `limits.max_response_bytes`  | `LimitedReader` wrapper                        | `500 io_error` mid-transfer                                           |

The `max_response_bytes` cap fires when the SOURCE has streamed
more than the configured number of bytes into the destination — it
is a total-bytes cap on the transfer, not a per-chunk cap. The
partial file on the destination is **not** rolled back; operators
should treat a 500 mid-copy as "target may contain a truncated
file" and clean up if needed.

A future release may promote the cap-exceeded error to a
structured `413` with a `transfer_too_large` code. The current
mapping goes through `io_error` because the cap fires inside the
async stream, not at the axum HTTP layer.

## Response headers on every reply

Every response — success or failure — carries:

- **`traceparent: 00-<trace_id>-<span_id>-01`** and
  **`x-trace-id: <trace_id>`**. If the caller sends an inbound
  `traceparent`, the same `trace_id` is echoed back so log-shippers
  can correlate the request across services. Otherwise a fresh
  128-bit trace_id is synthesised.
- Five browser-side defaults: `Content-Security-Policy`,
  `Strict-Transport-Security`, `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`.
  These are belt-and-braces against a reverse-proxy misconfiguration.

## Startup failures

FileFerry fails hard on boot if any of the following hold:

- The config file at `--config` / `FILEFERRY_CONFIG` doesn't exist or can't be parsed
- The config file declares an `s3:` block but the referenced env vars are unset or empty
- The config file declares a `security:` block with `inter_service_token_env` and the referenced env var is unset or empty
- The config file sets a non-empty `cors_origin` (CORS is not implemented; terminate at reverse proxy)
- The config file's `s3.bucket_path` contains `..` or starts with `/`
- The FS backend's `data_directory` can't be created (permission error)
- The TCP port is already in use

All boot failures print a single-line error to stderr with enough
context to identify the missing/broken input. There is no retry
loop; the operator or supervisor is expected to fix the issue.

## Boot-time WARN diagnostics

FileFerry does not fail-hard for every unsafe posture — a few land
as boot WARNs so existing deployments keep working:

- **Non-loopback bind without auth.** When the listener is
  non-loopback AND `security.inter_service_token_env` is unset AND
  `security.trust_network=false`, boot logs a WARN naming the
  concrete impact (S3 API cost, cross-backend exfil) and how to
  fix it. Future releases may promote this to a boot refusal.
- **`FILEFERRY_OFFLINE=1` with an `s3:` block.** Boot logs a WARN
  that outbound is stubbed. Every S3 call will fail with
  `502 upstream_error` and the message `offline mode: outbound
  blocked by FILEFERRY_OFFLINE`.

## Audit log line per completed transfer

`POST /v1/files/copy` emits one INFO line per successful copy
named `file_transfer_completed` with structured fields:

```
source=FS destination=S3
source_path_hash=1a2b3c4d5e6f
destination_path_hash=9f8e7d6c5b4a
bytes=1048576 duration_ms=124
outcome=success trace_id=...
```

Paths are hashed (first 12 hex chars of SHA-256) so tenant
identifiers never appear in log shippers or SIEM tools. See
`CHANGELOG.md` entry FN-LOG-3 for the rationale.
