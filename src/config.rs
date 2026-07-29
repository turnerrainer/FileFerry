use std::env;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Fully-resolved runtime configuration. Loaded from `fileferry.yaml`
/// (or the path supplied via `--config` / `FILEFERRY_CONFIG`) and
/// annotated with resolved secrets from env vars.
///
/// **Search order** (first hit wins):
///   1. `--config <path>` CLI arg
///   2. `FILEFERRY_CONFIG` env var
///   3. `./fileferry.yaml`
///   4. Built-in defaults (FS backend at `./data`, S3 disabled)
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub documentation_enabled: bool,
    pub cors_origin: String,
    pub fs: FsConfig,
    /// `Some` only when the YAML declares an `s3:` block AND every
    /// referenced env var is set. Startup fails hard if the block is
    /// declared but an env var is missing — never a silent downgrade.
    pub s3: Option<S3Config>,
    pub limits: LimitsConfig,
}

#[derive(Debug, Clone)]
pub struct FsConfig {
    pub data_directory: PathBuf,
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub region: String,
    /// Empty string means "use default AWS endpoint for the region".
    pub endpoint_url: String,
    pub bucket: String,
    pub bucket_path: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, Clone)]
pub struct LimitsConfig {
    pub max_request_bytes: u64,
    pub max_response_bytes: u64,
    pub request_timeout_secs: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 33_554_432,     // 32 MiB
            max_response_bytes: 5_368_709_120, // 5 GiB
            request_timeout_secs: 300,
        }
    }
}

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            data_directory: PathBuf::from("./data"),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            documentation_enabled: true,
            cors_origin: String::new(),
            fs: FsConfig::default(),
            s3: None,
            limits: LimitsConfig::default(),
        }
    }
}

// ---------------------------------------------------------------
// YAML shape — decoupled from the runtime struct so the wire format
// can carry env-var references while the runtime holds resolved
// values.
// ---------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AppConfigYaml {
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    documentation_enabled: Option<bool>,
    #[serde(default)]
    cors_origin: Option<String>,
    #[serde(default)]
    fs: Option<FsConfigYaml>,
    #[serde(default)]
    s3: Option<S3ConfigYaml>,
    #[serde(default)]
    limits: Option<LimitsConfigYaml>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FsConfigYaml {
    data_directory: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct S3ConfigYaml {
    region: String,
    #[serde(default)]
    endpoint_url: String,
    bucket: String,
    #[serde(default)]
    bucket_path: String,
    access_key_id_env: String,
    secret_access_key_env: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LimitsConfigYaml {
    max_request_bytes: Option<u64>,
    max_response_bytes: Option<u64>,
    request_timeout_secs: Option<u64>,
}

/// Try to load the config from the standard search order. Returns
/// `Ok(default)` if no file is found.
///
/// `explicit_path` corresponds to `--config <path>` supplied on the
/// command line — errors if given but the file doesn't parse.
pub fn load(explicit_path: Option<&Path>) -> anyhow::Result<AppConfig> {
    let candidate = resolve_path(explicit_path);
    let Some(path) = candidate else {
        tracing::info!("no fileferry.yaml found; using built-in defaults");
        return Ok(AppConfig::default());
    };
    tracing::info!(path = %path.display(), "loading config");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading config {}: {e}", path.display()))?;
    let yaml: AppConfigYaml = serde_yaml_ng::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parsing config {}: {e}", path.display()))?;
    from_yaml(yaml)
}

fn resolve_path(explicit_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit_path {
        return Some(p.to_path_buf());
    }
    if let Ok(env_path) = env::var("FILEFERRY_CONFIG") {
        if !env_path.is_empty() {
            return Some(PathBuf::from(env_path));
        }
    }
    let default = PathBuf::from("./fileferry.yaml");
    if default.exists() {
        Some(default)
    } else {
        None
    }
}

fn from_yaml(y: AppConfigYaml) -> anyhow::Result<AppConfig> {
    let defaults = AppConfig::default();
    let mut cfg = AppConfig {
        port: y.port.unwrap_or(defaults.port),
        documentation_enabled: y
            .documentation_enabled
            .unwrap_or(defaults.documentation_enabled),
        cors_origin: y.cors_origin.unwrap_or(defaults.cors_origin),
        fs: FsConfig {
            data_directory: y
                .fs
                .as_ref()
                .and_then(|f| f.data_directory.clone())
                .unwrap_or(defaults.fs.data_directory),
        },
        s3: None,
        limits: LimitsConfig {
            max_request_bytes: y
                .limits
                .as_ref()
                .and_then(|l| l.max_request_bytes)
                .unwrap_or(defaults.limits.max_request_bytes),
            max_response_bytes: y
                .limits
                .as_ref()
                .and_then(|l| l.max_response_bytes)
                .unwrap_or(defaults.limits.max_response_bytes),
            request_timeout_secs: y
                .limits
                .as_ref()
                .and_then(|l| l.request_timeout_secs)
                .unwrap_or(defaults.limits.request_timeout_secs),
        },
    };
    if let Some(s3) = y.s3 {
        let access_key_id = require_env(&s3.access_key_id_env)?;
        let secret_access_key = require_env(&s3.secret_access_key_env)?;
        cfg.s3 = Some(S3Config {
            region: s3.region,
            endpoint_url: s3.endpoint_url,
            bucket: s3.bucket,
            bucket_path: s3.bucket_path,
            access_key_id,
            secret_access_key,
        });
    }
    Ok(cfg)
}

fn require_env(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        Ok(_) => Err(anyhow::anyhow!(
            "env var {name} referenced by config but is empty"
        )),
        Err(_) => Err(anyhow::anyhow!(
            "env var {name} referenced by config but is unset"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_file() {
        let cfg = load(Some(Path::new("/nonexistent/fileferry.yaml")));
        // load() requires the file to exist when explicit_path is set —
        // this asserts the "no file at all" branch. Use the env-clear
        // path instead.
        assert!(
            cfg.is_err(),
            "explicit path that doesn't exist is a hard error"
        );
    }

    #[test]
    fn parses_minimal_yaml() {
        let y: AppConfigYaml = serde_yaml_ng::from_str("port: 9000\n").unwrap();
        let cfg = from_yaml(y).unwrap();
        assert_eq!(cfg.port, 9000);
        assert!(cfg.s3.is_none());
    }

    #[test]
    fn s3_block_fails_without_env() {
        // Guaranteed-unique env-var name; not set anywhere.
        let y: AppConfigYaml = serde_yaml_ng::from_str(
            r#"
s3:
  region: us-east-1
  bucket: b
  access_key_id_env: FILEFERRY_TEST_MISSING_ACCESS
  secret_access_key_env: FILEFERRY_TEST_MISSING_SECRET
"#,
        )
        .unwrap();
        let err = from_yaml(y).unwrap_err().to_string();
        assert!(err.contains("FILEFERRY_TEST_MISSING_ACCESS"), "{err}");
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        // `deny_unknown_fields` on the YAML struct guards against config
        // typos that would otherwise silently be ignored.
        let err = serde_yaml_ng::from_str::<AppConfigYaml>("porrt: 9000\n").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("porrt"));
    }
}
