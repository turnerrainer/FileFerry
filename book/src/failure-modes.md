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

## Status code + error code matrix

| HTTP  | `error` code               | Cause                                                                                                       |
|-------|----------------------------|-------------------------------------------------------------------------------------------------------------|
| `400` | `invalid_path`             | `sourceFilePath` or `destinationFilePath` failed validation — see "Path validation" below                    |
| `400` | `same_storage_type`        | Copy request where `sourceStorageType == destinationStorageType` — S3-Ferry parity, prevents no-op copies    |
| `400` | (axum default)             | Malformed JSON body, missing required field, or `?type=` outside `{FS,S3}`                                  |
| `404` | `not_found`                | Source file doesn't exist in the requested backend                                                          |
| `413` | *(HTTP client)*            | Inbound request body exceeded `limits.max_request_bytes` (raised by `tower-http`)                            |
| `500` | `io_error`                 | Filesystem I/O failed mid-transfer, OR the transfer exceeded `limits.max_response_bytes` (LimitedReader)     |
| `500` | `internal_error`           | Bug in FileFerry — always paired with a WARN log line. File a task with the log snippet.                     |
| `502` | `upstream_error`           | S3 returned an error other than `NoSuchKey` (network, auth, quota, throttling)                              |
| `503` | `backend_not_configured`   | Request targets `S3` but no `s3:` block is configured in `fileferry.yaml`                                    |

## Path validation

FileFerry rejects any path that:

- is empty
- contains a null byte (`\0`)
- contains any character outside `[0-9 a-z A-Z - . _ /]`
- contains a `..` segment (traversal)

This is deliberately restrictive — a file-transfer proxy has no
legitimate use for spaces, unicode filenames, backslashes, or
`@` / `+` / `#`. The whitelist eliminates a large class of
encoding-mismatch and injection risks at the boundary.

If you need to broker files whose names violate this rule, rename
them on the source side before submitting the copy request.

## Size caps

Two independent caps:

| Cap                          | Enforced by             | Fires as                                        |
|------------------------------|-------------------------|-------------------------------------------------|
| `limits.max_request_bytes`   | `tower-http` layer      | `413 Payload Too Large` before the handler runs |
| `limits.max_response_bytes`  | `LimitedReader` wrapper | `500 io_error` mid-transfer                     |

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

## Startup failures

FileFerry fails hard on boot if any of the following hold:

- The config file at `--config` / `FILEFERRY_CONFIG` doesn't exist or can't be parsed
- The config file declares an `s3:` block but the referenced env vars are unset or empty
- The FS backend's `data_directory` can't be created (permission error)
- The TCP port is already in use

All boot failures print a single-line error to stderr with enough
context to identify the missing/broken input. There is no retry
loop; the operator or supervisor is expected to fix the issue.
