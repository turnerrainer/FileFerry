# 004 — Live-S3 integration test job via LocalStack

## Filed
2026-07-29 — the current test suite covers the router-level seam
and the S3Backend code paths that don't need a wire (config,
list-with-prefix, key-composition). It does NOT actually exercise
the S3 client against a live endpoint. Filed as a follow-up to
enable that.

## Severity
Medium. Not shipping-blocking (the wire is exercised at operator
deploy time), but every subsequent S3-related task (002, 003)
wants this in place so their acceptance tests can assert on
real wire behaviour.

## Motivation
- Prevent silent regressions in `aws-sdk-s3` upgrades — e.g. a
  breaking change in `force_path_style` semantics
- Catch protocol-level bugs (auth-header assembly, region
  handling, MinIO-compat quirks) BEFORE they reach a shipping
  RC
- Give tasks 002 and 003 a place to run their perf assertions

## Fix / Design
Add a new job to `.github/workflows/tests.yml`:

```yaml
  s3-live:
    runs-on: ubuntu-latest
    services:
      localstack:
        image: localstack/localstack:3.3.0
        ports: ["4566:4566"]
        env:
          SERVICES: s3
    steps:
      - uses: actions/checkout@v4
      - name: Wait for LocalStack
        run: |
          for i in $(seq 1 60); do
            if curl -sf http://localhost:4566/_localstack/health >/dev/null; then
              break
            fi
            sleep 1
          done
      - name: Provision bucket
        run: |
          # awscli via pip; use --endpoint-url pointing at :4566
          pip install awscli
          aws --endpoint-url=http://localhost:4566 \
              s3 mb s3://fileferry-test-bucket
      - name: Run S3 live tests
        env:
          FILEFERRY_S3_ACCESS_KEY_ID: test
          FILEFERRY_S3_SECRET_ACCESS_KEY: test
          AWS_ENDPOINT_URL: http://localhost:4566
          AWS_REGION: us-east-1
        run: cargo test --features localstack-tests --test s3_live
```

Plus a new `tests/s3_live.rs` gated behind a `localstack-tests`
feature (added to `[features]` in `Cargo.toml`; empty feature —
only affects which test binaries get built).

## Acceptance
- [ ] `tests/s3_live.rs` exists, gated by feature
- [ ] Tests: list + open_read + write_all against LocalStack,
  each round-trip byte-equal
- [ ] Test: NoSuchKey → `FerryError::NotFound` (not `Upstream`)
- [ ] Test: bad credentials → `FerryError::Upstream`
- [ ] `tests.yml` s3-live job green on both amd64 and (if
  practical — LocalStack may not have arm64) amd64 only
- [ ] Docs: `book/src/getting-started.md` gains a "Run S3 tests
  locally with LocalStack" section

## Estimated effort
1-2 days. Most time in getting the LocalStack service block +
awscli bucket provisioning right in CI.

## Dependencies
None.

## Non-scope
- Testing against a real AWS account — LocalStack is the CI
  target
- Testing against MinIO / R2 / B2 — LocalStack covers the
  S3-API side; provider-specific quirks are the operator's
  problem
- Perf assertions — those come with task 002

## Risks
- LocalStack image bloat balloons CI time. Mitigation: pin the
  image version, cache the layer, keep the s3-live job separate
  from the main build-test matrix so failures are attributable
- The feature gate must genuinely no-op when not enabled;
  accidental compile-in of localstack test code into the shipped
  binary would be a security-review finding. Mitigation: the
  feature only affects `[[test]]` targets, not `[dependencies]`
