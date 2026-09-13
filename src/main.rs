use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use fileferry::backend::fs::FsBackend;
use fileferry::backend::offline::{offline_from_env, OfflineBackend};
use fileferry::backend::s3::S3Backend;
use fileferry::backend::{BackendRef, Backends};
use fileferry::config;
use fileferry::diagnose;
use fileferry::model::StorageType;
use fileferry::router::{build_router, AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let explicit_config = parse_config_arg();
    let cfg = config::load(explicit_config.as_deref())?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        port = cfg.port,
        fs_root = %cfg.fs.data_directory.display(),
        s3_enabled = cfg.s3.is_some(),
        "starting FileFerry"
    );

    // Boot-time diagnostic pass — warn on any accepted-but-unwired
    // field set to a non-default value.
    diagnose::diagnose(&cfg);

    let fs_backend: BackendRef =
        Arc::new(FsBackend::new(&cfg.fs).context("initialising FS backend")?);
    // Fleet stronghold §9.1: `FILEFERRY_OFFLINE=1` (or `true` / `yes`)
    // replaces the S3 backend with a stub that fails every call with
    // `Upstream("offline mode: ...")`. Never accidentally hit a live
    // upstream during a pentest / break-test session. FS stays real
    // because it doesn't leave the process.
    let offline = offline_from_env();
    let s3_backend: Option<BackendRef> = if let Some(s3_cfg) = &cfg.s3 {
        if offline {
            tracing::warn!(
                "FILEFERRY_OFFLINE is set — S3 backend is stubbed; every outbound call \
                 returns Upstream(\"offline mode\") without contacting AWS/MinIO"
            );
            Some(Arc::new(OfflineBackend::new(StorageType::S3)))
        } else {
            Some(Arc::new(S3Backend::new(s3_cfg).await?))
        }
    } else {
        if offline {
            tracing::info!(
                "FILEFERRY_OFFLINE is set but no s3: block configured — nothing to stub"
            );
        }
        None
    };
    let backends = Backends {
        fs: fs_backend,
        s3: s3_backend,
    };

    let state = AppState {
        backends,
        config: Arc::new(cfg.clone()),
    };
    let router = build_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    warn_if_unauth_non_loopback(&cfg, &addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, router).await?;
    Ok(())
}

/// F-FF-1 / F-FF-2 (h2ck.me v1 public-exposure): warn loudly when the
/// listener is reachable from outside the local host AND no inter-
/// service bearer is configured AND the operator hasn't opted into the
/// "trust the reverse proxy" posture. The WARN names the concrete
/// impact (S3 API cost, cross-backend data exfil) so an operator reading
/// the boot log knows the risk without leaving the log.
///
/// Kept as a WARN, not a boot refusal, to preserve backwards
/// compatibility with existing v0.1.x deployments. Fleet stronghold
/// §3.1 (FLEET-STRONGHOLDS.md) says future releases should promote
/// this to a hard refuse.
fn warn_if_unauth_non_loopback(cfg: &fileferry::config::AppConfig, addr: &SocketAddr) {
    let is_loopback = addr.ip().is_loopback();
    let has_token = cfg.security.inter_service_token.is_some();
    if is_loopback || has_token || cfg.security.trust_network {
        return;
    }
    tracing::warn!(
        bind = %addr,
        "security.inter_service_token_env is unset AND bind is non-loopback AND \
         security.trust_network=false. `/v1/files*` are open to any network caller. \
         Every unauth `POST /v1/files/copy` triggers backend I/O and S3-billed API \
         calls; every unauth `GET /v1/files` enumerates the FS root or the S3 bucket. \
         Set security.inter_service_token_env to the name of an env var whose value \
         is the required bearer token, OR set security.trust_network=true if a \
         reverse proxy / service mesh already authenticates every request before it \
         reaches FileFerry (see SECURITY.md)."
    );
}

fn init_tracing() {
    // Audit LOG-v1 FN-LOG-1: emit ANSI colour codes only when stderr is
    // a TTY. Under Docker / systemd, ship plain-text logs for SIEM.
    use std::io::IsTerminal;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,fileferry=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
}

/// Parse `--config <path>` from argv. Kept tiny — a full CLI parser
/// (`clap`) is more surface area than one flag warrants.
fn parse_config_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--config" {
            return args.next().map(PathBuf::from);
        }
        if let Some(rest) = a.strip_prefix("--config=") {
            return Some(PathBuf::from(rest));
        }
    }
    None
}
