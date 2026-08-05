# compat/ — Syntactic corpus of S3-Ferry user-authored files

This directory holds verbatim copies of the user-authored files
S3-Ferry ships. `tests/compat_corpus.rs` reads each file, extracts
every env-var name, and asserts each is on the curated
`COVERED_FIELDS` list at the top of that test file. A new field
appearing in a compat file with no `COVERED_FIELDS` entry fails
CI — this is the guard that prevents a silent drop of an S3-Ferry
config field during future refactoring.

## Contents

| Path | Origin (`../S3-Ferry/`) | Purpose |
|---|---|---|
| `s3-ferry/config/development.env` | `config/development.env` | Dev-profile env file S3-Ferry loads when `NODE_ENV=development` |
| `s3-ferry/config/production.env` | `config/production.env` | Production template (variable names only, no values) |
| `s3-ferry/config/test.env` | `config/test.env` | Test-profile env file used by `test/app.controller.e2e-spec.ts` |

## Fixtures NOT ported (and why)

| S3-Ferry file | Rationale |
|---|---|
| `Dockerfile`, `docker-compose.yml`, `nest-cli.json`, `tsconfig*.json`, `package*.json`, `.env.img`, `release.env` | Build/deploy metadata — not user-authored input to the running service. |
| `bump-version.sh`, `generate-changelog.sh`, `sync-version.sh` | Release tooling, not runtime input. |
| `localstack-init.sh` | Fixture for the LocalStack sidecar (not shipped in FileFerry's compose file). |
| `test/app.controller.e2e-spec.ts`, `test/jest-e2e.json` | Test source, not user-authored input format. |
| `docs/**` | Documentation; not a runtime input. |

## Adding new compat files

When S3-Ferry (or another upstream) grows a new user-authored format
we want to remain compatible with:

1. Drop a verbatim copy into `compat/<upstream-name>/<path>`.
2. Extend `tests/compat_corpus.rs` — both the extractor if the
   new file uses a different syntax and the `COVERED_FIELDS`
   list — after reviewing each new field against the FileFerry
   target contract.
3. Update this README's "Contents" table.
