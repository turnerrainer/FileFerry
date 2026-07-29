use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use fileferry::backend::fs::FsBackend;
use fileferry::backend::s3::S3Backend;
use fileferry::backend::{BackendRef, Backends};
use fileferry::config;
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

    let fs_backend: BackendRef =
        Arc::new(FsBackend::new(&cfg.fs).context("initialising FS backend")?);
    let s3_backend: Option<BackendRef> = if let Some(s3_cfg) = &cfg.s3 {
        Some(Arc::new(S3Backend::new(s3_cfg).await?))
    } else {
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, router).await?;
    Ok(())
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,fileferry=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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
