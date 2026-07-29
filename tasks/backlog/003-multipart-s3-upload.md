# 003 — Multipart-upload for objects > 100 MiB

## Filed
2026-07-29 — S3's single-PUT ceiling is 5 GiB; anything larger
requires multipart. FileFerry inherits that ceiling. Filed as
a follow-up to task 002 (which handles the < 5 GiB case).

## Severity
Medium. Not blocking for the "typical S3-Ferry migration"
use-case; blocking for any operator moving media / backups
around.

## Motivation
Two motivations:

1. **Raise the object-size ceiling** past 5 GiB.
2. **Parallelise the wire** for large objects — sequential
   single-PUT is bandwidth-limited by round-trip latency on the
   final ACK.

## Fix / Design
Threshold at 100 MiB (configurable via a new
`s3.multipart_threshold_bytes` field, default 100 MiB). For
larger uploads use `create_multipart_upload` →
`upload_part` (parallel N workers) → `complete_multipart_upload`.

On any part failure, `abort_multipart_upload` cleans up. Log the
abort at WARN — it costs money if it silently leaks.

Chunk size: start at 8 MiB (S3 minimum is 5 MiB except for the
last part). Configurable via `s3.multipart_chunk_bytes`.

## Acceptance
- [ ] `s3.multipart_threshold_bytes` config field, default 100 MiB
- [ ] `s3.multipart_chunk_bytes` config field, default 8 MiB
- [ ] `S3Backend::write_all` picks single-PUT or multipart based
  on the threshold
- [ ] Test: upload a 250 MB payload FS → S3, verify byte-equal
- [ ] Test: mid-upload connection drop → abort called, no orphan
  multipart on the bucket
- [ ] Test: multipart_threshold_bytes = 1 → forces multipart for
  a 1 KiB payload; verify still byte-equal

## Estimated effort
2-3 days. The abort-on-failure branch is where the subtlety lives.

## Dependencies
- Task 002 (streaming upload) — this task builds on the same
  code path
- Task 004 (LocalStack CI) — needed to exercise the multipart
  wire in CI

## Non-scope
- Resumable uploads from a client checkpoint (that's TUS
  protocol scope, task 010+)
- Server-side encryption per-part (default SSE is fine)
- Cross-region replication

## Risks
- Leaking multipart uploads that were started but never
  completed and never aborted. S3 charges for the storage of
  the parts. Mitigation: lifecycle rule on the bucket
  (`AbortIncompleteMultipartUpload`) documented in the book as
  operator advice. FileFerry itself calls abort on any error
  path — verified by the "mid-upload drop" test above.
