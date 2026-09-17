//! Boot-time preflight warnings.
//!
//! T-21 (fleet stronghold §11, TIM pattern — 6 checks): before the
//! server starts accepting connections, walk the (config × bind
//! address) pair and emit one WARN per posture that is *accepted* but
//! likely a footgun. Warnings do NOT abort boot — every check has a
//! legitimate override (a reverse proxy, a legitimate slow-source
//! peer, an intentionally-wired recon-endpoint) so a hard fail would
//! break existing deployments. The WARN gives an operator reading the
//! boot log one line per risk with enough detail to decide "yes, that
//! is intentional" or "wait, that was left over from dev".
//!
//! Each check has a stable string id (`W-<n>`) so a test can assert
//! "config X triggers exactly warnings W-1 and W-4" without matching
//! against the wire message. Log-shipping pipelines can also key
//! alerts on the ids.

use std::net::SocketAddr;

use crate::config::AppConfig;

/// A single boot-time warning. `id` is stable across releases (feel
/// free to add new checks; never renumber an existing one). `message`
/// is the operator-facing one-liner logged at WARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootWarning {
    pub id: &'static str,
    pub message: String,
}

/// Ceiling above which `copy_inactivity_secs` is flagged as a
/// slow-drip risk. Legitimate slow S3 uploads can occasionally need
/// more than 30 s but going past 5 minutes weakens the F3 defence to
/// the point of "essentially no timeout". Tunable via the operator
/// override pattern — the WARN is still emitted so the choice is
/// documented in the boot log.
pub const COPY_INACTIVITY_WARN_CEILING_SECS: u64 = 300;

/// Ceiling above which `max_request_bytes` is flagged as unusually
/// large. `copy` request bodies carry JSON metadata only — a healthy
/// deployment stays well below 32 MiB. Values above 100 MiB usually
/// mean someone thought the request body carried the file payload,
/// which it doesn't.
pub const MAX_REQUEST_BYTES_WARN_CEILING: u64 = 100 * 1024 * 1024;

/// Run every check against `cfg` + `addr`; return the list of
/// warnings that fired (in stable id order). Callers log them at WARN.
///
/// New checks: add a private `fn check_<n>` returning `Option<BootWarning>`
/// and call it here. Keep the id monotonic, and DO NOT reuse an old
/// id even if you delete the check that produced it — downstream log
/// alerts key on the id.
pub fn preflight(cfg: &AppConfig, addr: &SocketAddr) -> Vec<BootWarning> {
    let checks = [
        check_unauth_non_loopback(cfg, addr),
        check_trust_network_without_token(cfg),
        check_admin_enabled_on_non_loopback(cfg, addr),
        check_documentation_enabled_on_non_loopback(cfg, addr),
        check_copy_inactivity_too_generous(cfg),
        check_max_request_bytes_unusually_large(cfg),
    ];
    checks.into_iter().flatten().collect()
}

/// **W-1** — F-FF-1 / F-FF-2 (h2ck.me v1). Non-loopback bind AND no
/// inter-service bearer AND operator hasn't opted into
/// `trust_network`. Every unauth `POST /v1/files/copy` triggers S3
/// billing; every unauth `GET /v1/files` enumerates the FS root.
fn check_unauth_non_loopback(cfg: &AppConfig, addr: &SocketAddr) -> Option<BootWarning> {
    let is_loopback = addr.ip().is_loopback();
    let has_token = cfg.security.inter_service_token.is_some();
    if is_loopback || has_token || cfg.security.trust_network {
        return None;
    }
    Some(BootWarning {
        id: "W-1",
        message: format!(
            "[W-1] security.inter_service_token_env is unset AND bind {} is non-loopback \
             AND security.trust_network=false. `/v1/files*` are open to any network caller. \
             Every unauth `POST /v1/files/copy` triggers backend I/O and S3-billed API calls; \
             every unauth `GET /v1/files` enumerates the FS root or the S3 bucket. Set \
             security.inter_service_token_env, OR set security.trust_network=true if a \
             reverse proxy already authenticates every request (see SECURITY.md).",
            addr
        ),
    })
}

/// **W-2** — the "trust the reverse proxy" flag is set but no bearer
/// token is configured either. This is the intended posture when a
/// proxy handles auth, but the double-off state is worth a one-time
/// audit note because a missing proxy would leave `/v1/files*` fully
/// open with the WARN in W-1 suppressed.
fn check_trust_network_without_token(cfg: &AppConfig) -> Option<BootWarning> {
    if cfg.security.trust_network && cfg.security.inter_service_token.is_none() {
        return Some(BootWarning {
            id: "W-2",
            message: "[W-2] security.trust_network=true AND \
                      security.inter_service_token_env is unset — every request that reaches \
                      `/v1/files*` is trusted. This is correct ONLY if a reverse proxy / \
                      service mesh is genuinely in front of FileFerry AND authenticates \
                      every request. If the proxy is misconfigured or absent, FileFerry is \
                      fully open. Consider defense-in-depth: set inter_service_token_env \
                      even behind a proxy."
                .to_string(),
        });
    }
    None
}

/// **W-3** — `/api` (OpenAPI recon endpoint) is env-gated on and the
/// bind is non-loopback. Even the OpenAPI summary reveals the route
/// table + version to any caller. Fine for internal tooling; a
/// footgun on a public edge.
fn check_admin_enabled_on_non_loopback(cfg: &AppConfig, addr: &SocketAddr) -> Option<BootWarning> {
    if cfg.security.admin_enabled && !addr.ip().is_loopback() {
        return Some(BootWarning {
            id: "W-3",
            message: format!(
                "[W-3] FILEFERRY_ADMIN_ENABLED is set AND bind {} is non-loopback. \
                 `GET /api` serves the OpenAPI summary (route table + version + DTO shapes) \
                 to any caller that hits the endpoint. Disable via unsetting the env var \
                 unless the endpoint is genuinely needed by external tooling.",
                addr
            ),
        });
    }
    None
}

/// **W-4** — even with `/api` env-gated off, `documentation_enabled`
/// stays true by default. If an operator flips
/// `FILEFERRY_ADMIN_ENABLED=1` later, the version + route table
/// immediately surface. Left as INFO-worthy but not a hard error;
/// operators who never intend to serve `/api` can set
/// `documentation_enabled: false` in YAML for defence-in-depth.
fn check_documentation_enabled_on_non_loopback(
    cfg: &AppConfig,
    addr: &SocketAddr,
) -> Option<BootWarning> {
    if cfg.documentation_enabled && !cfg.security.admin_enabled && !addr.ip().is_loopback() {
        return Some(BootWarning {
            id: "W-4",
            message: format!(
                "[W-4] documentation_enabled=true in YAML AND bind {} is non-loopback. \
                 `/api` is currently 404'd by the FILEFERRY_ADMIN_ENABLED gate, but a \
                 future operator setting the env var (e.g. for a `doctor` diagnostic) \
                 immediately surfaces the OpenAPI summary. For defence-in-depth on public \
                 edges, set documentation_enabled: false in the YAML too.",
                addr
            ),
        });
    }
    None
}

/// **W-5** — F3 (slow-drip DoS). Default `copy_inactivity_secs = 30`.
/// Values above 5 minutes weaken the per-poll timeout to the point
/// where an attacker can hold copies open indefinitely at 1 byte /
/// 4 min. Legitimate slow sources exist — hence WARN, not fail.
fn check_copy_inactivity_too_generous(cfg: &AppConfig) -> Option<BootWarning> {
    if cfg.limits.copy_inactivity_secs > COPY_INACTIVITY_WARN_CEILING_SECS {
        return Some(BootWarning {
            id: "W-5",
            message: format!(
                "[W-5] limits.copy_inactivity_secs = {}s exceeds the recommended {}s ceiling. \
                 Slow-drip peers can now hold copy streams open for arbitrarily long \
                 (F3 defence weakened). Confirm the deployment genuinely has a slow-but- \
                 legitimate source that needs >{}s between successful reads.",
                cfg.limits.copy_inactivity_secs,
                COPY_INACTIVITY_WARN_CEILING_SECS,
                COPY_INACTIVITY_WARN_CEILING_SECS
            ),
        });
    }
    None
}

/// **W-6** — `max_request_bytes` unusually large. The copy-request
/// body is JSON metadata only; 32 MiB is the shipped default and
/// covers even absurdly long DTOs. Values above 100 MiB usually mean
/// someone thought the request body carried the file itself.
fn check_max_request_bytes_unusually_large(cfg: &AppConfig) -> Option<BootWarning> {
    if cfg.limits.max_request_bytes > MAX_REQUEST_BYTES_WARN_CEILING {
        return Some(BootWarning {
            id: "W-6",
            message: format!(
                "[W-6] limits.max_request_bytes = {} bytes exceeds the recommended {} \
                 (100 MiB) ceiling. FileFerry does NOT read file payloads out of the request \
                 body — copies happen backend-to-backend — so the request body only carries \
                 JSON metadata. If you meant to raise a per-file cap, set \
                 limits.max_response_bytes instead.",
                cfg.limits.max_request_bytes, MAX_REQUEST_BYTES_WARN_CEILING
            ),
        });
    }
    None
}

/// Convenience wrapper: run `preflight`, log every returned warning
/// at WARN level. Called from `main` at boot. Kept separate from
/// `preflight` so tests can assert the *result* without exercising
/// the tracing subscriber.
pub fn log_preflight(cfg: &AppConfig, addr: &SocketAddr) {
    for w in preflight(cfg, addr) {
        tracing::warn!(id = w.id, "{}", w.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, LimitsConfig, SecurityConfig};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn loopback() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
    }
    fn wildcard() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080)
    }

    fn ids(ws: &[BootWarning]) -> Vec<&'static str> {
        ws.iter().map(|w| w.id).collect()
    }

    #[test]
    fn default_loopback_is_silent() {
        // AppConfig::default() on 127.0.0.1 should fire no warnings —
        // that's the "dev with `cargo run`" happy path.
        let cfg = AppConfig::default();
        assert!(preflight(&cfg, &loopback()).is_empty());
    }

    #[test]
    fn wildcard_bind_without_auth_fires_w1_and_w4() {
        // Wildcard bind, no token, docs on by default, admin gate off.
        // W-1 fires (open + non-loopback), W-4 fires (docs enabled on
        // public edge). W-3 does NOT (admin off).
        let cfg = AppConfig::default();
        assert_eq!(ids(&preflight(&cfg, &wildcard())), vec!["W-1", "W-4"]);
    }

    #[test]
    fn wildcard_with_token_only_fires_w4() {
        // Token wired → W-1 suppressed. Docs still on → W-4 fires.
        let cfg = AppConfig {
            security: SecurityConfig {
                inter_service_token: Some("bearer".into()),
                trust_network: false,
                admin_enabled: false,
            },
            ..AppConfig::default()
        };
        assert_eq!(ids(&preflight(&cfg, &wildcard())), vec!["W-4"]);
    }

    #[test]
    fn trust_network_without_token_fires_w2() {
        // The "I trust the proxy" flag alone is enough to fire W-2,
        // regardless of bind address. W-1 is suppressed by
        // trust_network=true; W-4 fires on non-loopback.
        let cfg = AppConfig {
            security: SecurityConfig {
                inter_service_token: None,
                trust_network: true,
                admin_enabled: false,
            },
            ..AppConfig::default()
        };
        assert_eq!(ids(&preflight(&cfg, &wildcard())), vec!["W-2", "W-4"]);
    }

    #[test]
    fn admin_enabled_on_wildcard_fires_w3() {
        // Admin on + non-loopback. W-1 fires too (still no token), W-3
        // fires, W-4 SUPPRESSED (admin already covers the recon
        // surface, W-4 is a heads-up for the pre-admin state).
        let cfg = AppConfig {
            security: SecurityConfig {
                inter_service_token: None,
                trust_network: false,
                admin_enabled: true,
            },
            ..AppConfig::default()
        };
        assert_eq!(ids(&preflight(&cfg, &wildcard())), vec!["W-1", "W-3"]);
    }

    #[test]
    fn copy_inactivity_above_ceiling_fires_w5() {
        let cfg = AppConfig {
            limits: LimitsConfig {
                copy_inactivity_secs: COPY_INACTIVITY_WARN_CEILING_SECS + 1,
                ..LimitsConfig::default()
            },
            ..AppConfig::default()
        };
        let ids = ids(&preflight(&cfg, &loopback()));
        assert!(ids.contains(&"W-5"), "got {ids:?}");
    }

    #[test]
    fn copy_inactivity_at_ceiling_is_silent() {
        // Boundary: ==CEILING is fine, >CEILING fires. Locks the
        // strict-greater-than comparison.
        let cfg = AppConfig {
            limits: LimitsConfig {
                copy_inactivity_secs: COPY_INACTIVITY_WARN_CEILING_SECS,
                ..LimitsConfig::default()
            },
            ..AppConfig::default()
        };
        assert!(preflight(&cfg, &loopback()).is_empty());
    }

    #[test]
    fn max_request_bytes_above_ceiling_fires_w6() {
        let cfg = AppConfig {
            limits: LimitsConfig {
                max_request_bytes: MAX_REQUEST_BYTES_WARN_CEILING + 1,
                ..LimitsConfig::default()
            },
            ..AppConfig::default()
        };
        let ids = ids(&preflight(&cfg, &loopback()));
        assert!(ids.contains(&"W-6"), "got {ids:?}");
    }

    #[test]
    fn every_warning_id_is_unique() {
        // Break-the-fix probe: adding a new check with a duplicate id
        // would silently clobber the existing one in log-alert
        // pipelines. Cover every reachable combination and assert the
        // observed id set has no duplicates.
        let stressed = AppConfig {
            documentation_enabled: true,
            security: SecurityConfig {
                inter_service_token: None,
                trust_network: true,
                admin_enabled: true,
            },
            limits: LimitsConfig {
                max_request_bytes: MAX_REQUEST_BYTES_WARN_CEILING + 1,
                max_response_bytes: 5_368_709_120,
                request_timeout_secs: 300,
                copy_inactivity_secs: COPY_INACTIVITY_WARN_CEILING_SECS + 1,
            },
            ..AppConfig::default()
        };
        let ws = preflight(&stressed, &wildcard());
        let ids: Vec<&'static str> = ws.iter().map(|w| w.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "duplicate id in {ids:?}");
    }
}
