use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use fileferry::backend::fs::FsBackend;
use fileferry::backend::offline::{offline_from_env, OfflineBackend};
use fileferry::backend::s3::S3Backend;
use fileferry::backend::{BackendRef, Backends};
use fileferry::boot_warnings::log_preflight;
use fileferry::config;
use fileferry::diagnose;
use fileferry::doctor;
use fileferry::model::StorageType;
use fileferry::router::{build_router, AppState};
use fileferry::shutdown::shutdown_signal;

fn main() -> ExitCode {
    // T-22: `fileferry doctor` is a synchronous config check. Handled
    // BEFORE we bring up the tokio runtime so it stays fast (no
    // multi-threaded scheduler init) and doesn't emit tracing lines
    // to stderr — doctor output is stdout only for shell composition.
    if std::env::args().any(|a| a == "doctor") {
        let explicit_config = parse_config_arg();
        return ExitCode::from(doctor::run(explicit_config.as_deref()) as u8);
    }
    match tokio_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fatal: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn tokio_main() -> anyhow::Result<()> {
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
    // T-21 (fleet stronghold §11, TIM pattern): numbered preflight
    // WARN block. Each check has a stable `W-<n>` id so log-alert
    // pipelines can key on the id without matching the wire message.
    // See src/boot_warnings.rs for the check list.
    log_preflight(&cfg, &addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    // T-23 / OWASP §34.4: wire SIGTERM + SIGINT to a graceful
    // shutdown so in-flight requests complete before the container
    // exits. Under `docker stop` (default 10 s grace) or a K8s
    // rolling-restart, this stops a `POST /v1/files/copy` from
    // dying half-way with no audit-log line. See src/shutdown.rs.
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shutdown complete");
    Ok(())
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
