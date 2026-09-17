//! `fileferry doctor` — configuration + posture health check.
//!
//! T-22 (fleet stronghold, XTR pattern). Loads the same config the
//! runtime would load, runs the boot-time diagnostic pass + the
//! preflight WARN block, and prints a green/amber/red report to
//! stdout. Exits with status 0 (clean) or 1 (WARNs surfaced) so it
//! composes into CI probes.
//!
//! Invocation: `fileferry doctor [--config <path>]`.
//!
//! Deliberate omissions:
//! - No live wire probes (no S3 head-bucket, no `/health` self-hit).
//!   Doctor validates *config*, not network reachability, so it runs
//!   safely on a machine without S3 creds or an open port. Add a
//!   `--online` flag if the operator wants wire checks.
//! - No auto-remediation. Doctor prints WARNs and exits; fixing them
//!   is up to the operator (the WARN message names what to change).

use std::io::Write;
use std::net::SocketAddr;

use crate::boot_warnings::{preflight, BootWarning};
use crate::config::AppConfig;

/// Format the doctor report to any `Write` sink. Returns the number
/// of WARNs surfaced so the caller can pick an exit code.
///
/// Split from `run` so tests can assert the exact bytes emitted
/// without spawning a subprocess.
pub fn write_report<W: Write>(
    out: &mut W,
    cfg: &AppConfig,
    addr: &SocketAddr,
) -> std::io::Result<usize> {
    writeln!(out, "fileferry doctor {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(out, "==================================================")?;
    writeln!(out)?;
    writeln!(out, "Config summary:")?;
    writeln!(out, "  port                        {}", cfg.port)?;
    writeln!(out, "  bind (expected)             {}", addr)?;
    writeln!(
        out,
        "  fs.data_directory           {}",
        cfg.fs.data_directory.display()
    )?;
    writeln!(
        out,
        "  s3 backend                  {}",
        if cfg.s3.is_some() {
            "configured"
        } else {
            "disabled"
        }
    )?;
    writeln!(
        out,
        "  documentation_enabled       {}",
        cfg.documentation_enabled
    )?;
    writeln!(
        out,
        "  security.inter_service_tok  {}",
        if cfg.security.inter_service_token.is_some() {
            "SET (masked)"
        } else {
            "unset"
        }
    )?;
    writeln!(
        out,
        "  security.trust_network      {}",
        cfg.security.trust_network
    )?;
    writeln!(
        out,
        "  security.admin_enabled      {} (FILEFERRY_ADMIN_ENABLED)",
        cfg.security.admin_enabled
    )?;
    writeln!(
        out,
        "  limits.copy_inactivity_secs {}",
        cfg.limits.copy_inactivity_secs
    )?;
    writeln!(
        out,
        "  limits.max_request_bytes    {}",
        cfg.limits.max_request_bytes
    )?;
    writeln!(out)?;

    let warnings: Vec<BootWarning> = preflight(cfg, addr);
    if warnings.is_empty() {
        writeln!(out, "Preflight checks: OK  (0 warnings — green)")?;
    } else {
        writeln!(
            out,
            "Preflight checks: {} warning(s) surfaced — amber",
            warnings.len()
        )?;
        writeln!(out)?;
        for w in &warnings {
            writeln!(out, "  {}", w.message)?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "Exit code: {} ({} warning(s))",
        if warnings.is_empty() { 0 } else { 1 },
        warnings.len()
    )?;
    Ok(warnings.len())
}

/// Entry point invoked from `main`. Loads the config, walks the
/// checks, prints the report to stdout, returns the process exit
/// code.
pub fn run(explicit_config: Option<&std::path::Path>) -> i32 {
    let cfg = match crate::config::load(explicit_config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("doctor: config load failed: {e:#}");
            return 2;
        }
    };
    let addr: SocketAddr = ([0, 0, 0, 0], cfg.port).into();
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match write_report(&mut lock, &cfg, &addr) {
        Ok(0) => 0,
        Ok(_) => 1,
        Err(e) => {
            eprintln!("doctor: write failed: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, SecurityConfig};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn wildcard() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080)
    }
    fn loopback() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
    }

    fn render(cfg: &AppConfig, addr: &SocketAddr) -> (String, usize) {
        let mut buf = Vec::new();
        let n = write_report(&mut buf, cfg, addr).unwrap();
        (String::from_utf8(buf).unwrap(), n)
    }

    #[test]
    fn clean_config_on_loopback_reports_zero_warnings() {
        let (text, n) = render(&AppConfig::default(), &loopback());
        assert_eq!(n, 0);
        assert!(text.contains("Preflight checks: OK"), "{text}");
        assert!(text.contains("Exit code: 0"), "{text}");
    }

    #[test]
    fn wildcard_default_config_reports_expected_warnings() {
        // Default wildcard bind trips W-1 + W-4. Doctor output must
        // name both and reflect the amber state.
        let (text, n) = render(&AppConfig::default(), &wildcard());
        assert_eq!(n, 2);
        assert!(text.contains("W-1"), "output missing W-1: {text}");
        assert!(text.contains("W-4"), "output missing W-4: {text}");
        assert!(text.contains("Preflight checks: 2 warning"), "{text}");
        assert!(text.contains("Exit code: 1"), "{text}");
    }

    #[test]
    fn secrets_are_masked_in_report() {
        // Doctor must not print the bearer token or S3 credentials. The
        // report renders through `SecurityConfig`'s masked Debug for
        // the token field (via the `.is_some()` branch), and never
        // reads the raw string. Break-the-fix probe: send a distinctive
        // token, assert the raw value never appears in the output.
        let sensitive = "SUPERSECRET-TOKEN-DEADBEEF-CAFEBABE";
        let cfg = AppConfig {
            security: SecurityConfig {
                inter_service_token: Some(sensitive.to_string()),
                trust_network: false,
                admin_enabled: false,
            },
            ..AppConfig::default()
        };
        let (text, _) = render(&cfg, &loopback());
        assert!(
            !text.contains(sensitive),
            "doctor leaked the bearer token in its report: {text}"
        );
        // Positive: the "SET (masked)" placeholder IS emitted so an
        // operator can distinguish "token wired" from "token missing".
        assert!(text.contains("SET (masked)"), "{text}");
    }

    #[test]
    fn report_ends_with_exit_code_line() {
        // Break-the-fix probe: shell composability. A CI probe piping
        // doctor output into grep needs a stable last line. Assert the
        // last non-empty line begins with `Exit code:`.
        let (text, _) = render(&AppConfig::default(), &loopback());
        let last_nonempty = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        assert!(
            last_nonempty.starts_with("Exit code:"),
            "last non-empty line was {last_nonempty:?}"
        );
    }
}
