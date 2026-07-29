# 005 — Enforce `cors_origin` config field

## Filed
2026-07-29 — the `cors_origin` field is in `fileferry.yaml` and
parsed by `src/config.rs` for S3-Ferry parity, but no
`tower-http::cors::CorsLayer` reads it. Filed as a follow-up so
the field carries real meaning.

## Severity
Low. Operators terminate CORS at their reverse proxy in
production; the field is a convenience for local development.
Ship-blocking only if a downstream integrator was actually
relying on S3-Ferry's `API_CORS_ORIGIN` behaviour.

## Motivation
- Honour the config field FileFerry advertises
- Un-block browser-based demos that hit the API directly

## Fix / Design
Add `tower-http` feature `cors` to the dep. In `src/router.rs`:

```rust
if !state.config.cors_origin.is_empty() {
    let origins: Vec<HeaderValue> = state
        .config
        .cors_origin
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| HeaderValue::from_str(s).ok())
        .collect();
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);
    layers = layers.layer(cors);
}
```

Special case: `cors_origin: "*"` → `CorsLayer::permissive()`
(matches S3-Ferry `API_CORS_ORIGIN=*` semantics; document in the
config chapter that this is dev-only).

Update `book/src/configuration.md` to remove the "Reserved. Not
enforced yet" note and document the actual semantics.

## Acceptance
- [ ] `cors_origin: ""` → no CORS headers on responses (verify
  with integration test)
- [ ] `cors_origin: "https://ui.example.com"` → matches only
  that origin (verify with integration test on both matched and
  unmatched origin)
- [ ] `cors_origin: "*"` → permissive CORS (verify with test)
- [ ] `cors_origin: "https://a,https://b"` → matches both
  (verify comma-split works)
- [ ] `book/src/configuration.md` no longer says "not enforced
  yet"

## Estimated effort
0.5 day.

## Dependencies
None.

## Non-scope
- Preflight-request-only `Access-Control-Max-Age` tuning
  (tower-http default is fine)
- Cookie / credentials CORS mode
  (`Access-Control-Allow-Credentials`) — deliberately not
  supported; if operators need cookies, they terminate CORS
  upstream

## Risks
- Incorrect comma-splitting on operator input with URLs
  containing commas (query strings, but unlikely in an origin
  header). Mitigation: an origin per line + comma-separator both
  supported; parse errors log at WARN and drop the malformed
  entry
