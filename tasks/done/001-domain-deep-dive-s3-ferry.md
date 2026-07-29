# 001 — Domain deep-dive on `buerokratt/S3-Ferry`

## Filed
2026-07-29 — initial scaffolding of FileFerry; before writing any
Rust code we need a full read of the JVM/NestJS upstream and a
written record of what we're preserving vs changing.

## Landed
2026-07-29 — commit `<initial>` on branch `dev`. Deep-dive
completed; findings captured in [`../../docs/DESIGN.md`](../../docs/DESIGN.md)
§§3–7 (surface, preserves, changes, trade-offs). MVP implemented
against the same commit in the same day — see
[`../../CHANGELOG.md`](../../CHANGELOG.md) v0.1.0-rc.1.

## Severity
High. The whole reason for FileFerry-on-Rust is to preserve the
S3-Ferry HTTP contract; getting that surface wrong = breaking
downstream operators who swap their base URL.

## Motivation
Every "small proxy" rewrite is tempted to cut corners on the
compatibility surface. XTR-on-Rust did the discipline (deep-dive
before code) and it paid off — 17 upstream bugs surfaced during
the deep-dive and were pre-fixed in the MVP rather than
rediscovered in production. Do the same here.

## Fix / Design
Read every file under
[`../../../S3-Ferry/src/`](https://github.com/buerokratt/S3-Ferry/tree/dev/src),
plus its `.env` config templates and `Dockerfile`. Produce
`docs/DESIGN.md` with:

- §3 the endpoint table
- §5 the "PRESERVE from S3-Ferry" list (byte-identical semantics)
- §6 the "CHANGE from S3-Ferry" table (with reason for each)
- §7 known trade-offs + limitations of the FileFerry impl

## Acceptance
- [x] `docs/DESIGN.md` exists with §§1–9
- [x] Every S3-Ferry endpoint appears in §3 with its FileFerry
  behaviour
- [x] Every behavioural delta appears in §6 with a reason
- [x] Every deferred item appears in §7 with a target task ID
- [x] MVP RC (v0.1.0-rc.1) shipped in the same commit set, per
  DEV-REQUIREMENTS §11 (every commit references a task file)

## Estimated effort
0.5 day. Actual: 0.5 day (deep-dive), same day for the MVP.

## Dependencies
None — first task on the roadmap.

## Non-scope
- Any advanced feature from `RESEARCH.md` (auth, versioning,
  virus scanning, etc.) — those live in `RESEARCH.md` and are
  explicitly deferred beyond v0.1.
- Feature-parity with all P0/P1 tables in `RESEARCH.md` — the
  MVP delivers **S3-Ferry parity** only; anything beyond is a
  separate task.

## Risks
- Missing a subtle behavioural detail in S3-Ferry that a
  downstream operator relies on. Mitigation: read the actual JS
  source, not the docs; port the path validator character-by-
  character; keep the response envelopes verbatim.
- Drift between `docs/DESIGN.md` claims and actual implementation.
  Mitigation: DESIGN.md §6 is enforced by integration tests
  (`tests/http_api.rs`) — any behavioural change is caught by a
  failing test rather than a stale doc.
