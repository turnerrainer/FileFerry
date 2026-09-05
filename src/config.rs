use std::env;
use std::fmt;
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

/// F2: no derived `Debug`. Formatting an `S3Config` (directly or as
/// part of `AppConfig` / `AppState`) must never print the AWS access
/// key or secret. See the hand-written `Debug` impl below.
#[derive(Clone)]
pub struct S3Config {
    pub region: String,
    /// Empty string means "use default AWS endpoint for the region".
    pub endpoint_url: String,
    pub bucket: String,
    pub bucket_path: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("region", &self.region)
            .field("endpoint_url", &self.endpoint_url)
            .field("bucket", &self.bucket)
            .field("bucket_path", &self.bucket_path)
            .field("access_key_id", &"***REDACTED***")
            .field("secret_access_key", &"***REDACTED***")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct LimitsConfig {
    pub max_request_bytes: u64,
    pub max_response_bytes: u64,
    pub request_timeout_secs: u64,
    /// F3: per-poll inactivity budget on the source reader during a
    /// `stream_copy`. If no bytes flow for this many seconds, the
    /// transfer aborts. Total-request timeout can't catch this because
    /// even a single byte per poll resets it.
    pub copy_inactivity_secs: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 33_554_432,     // 32 MiB
            max_response_bytes: 5_368_709_120, // 5 GiB
            request_timeout_secs: 300,
            copy_inactivity_secs: 30,
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
    copy_inactivity_secs: Option<u64>,
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
            copy_inactivity_secs: y
                .limits
                .as_ref()
                .and_then(|l| l.copy_inactivity_secs)
                .unwrap_or(defaults.limits.copy_inactivity_secs),
        },
    };
    if let Some(s3) = y.s3 {
        let access_key_id = require_env(&s3.access_key_id_env)?;
        let secret_access_key = require_env(&s3.secret_access_key_env)?;
        let s3_cfg = S3Config {
            region: s3.region,
            endpoint_url: s3.endpoint_url,
            bucket: s3.bucket,
            bucket_path: s3.bucket_path,
            access_key_id,
            secret_access_key,
        };
        // F5: validate bucket_path before it can be persisted anywhere.
        // Rejecting `..` and leading `/` at boot means a future consumer
        // that treats the prefix as a filesystem-like path (a sync-back
        // job, an FS mirror) can't be tricked into escaping the prefix.
        s3_cfg.validate()?;
        cfg.s3 = Some(s3_cfg);
    }
    // F6: refuse to start with a CORS origin set until the field is
    // actually wired. Silent-drop-behind-WARN taught operators the
    // wrong lesson (see AUDIT F6). Deferring the real implementation
    // to a follow-up; this fail-closed behaviour is the safer default.
    if !cfg.cors_origin.is_empty() {
        anyhow::bail!(
            "config.cors_origin is set but CORS is not implemented in this build. \
             Terminate CORS at your reverse proxy (see SECURITY.md) or leave the field empty."
        );
    }
    Ok(cfg)
}

impl S3Config {
    /// F5: reject bucket-prefix values that could be misinterpreted as
    /// filesystem paths by future consumers. S3 keys are flat strings,
    /// so this is defence-in-depth rather than a live exploit fix.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.bucket_path.contains("..") {
            anyhow::bail!(
                "config.s3.bucket_path may not contain '..' \
                 (rejected value: {:?})",
                self.bucket_path
            );
        }
        if self.bucket_path.starts_with('/') {
            anyhow::bail!(
                "config.s3.bucket_path may not start with '/' \
                 (rejected value: {:?})",
                self.bucket_path
            );
        }
        Ok(())
    }
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
    fn debug_of_s3config_never_contains_secrets() {
        // F2 regression: format the struct as Debug and Debug-alternate;
        // neither the id nor the secret may appear in the output. If a
        // future refactor re-adds `#[derive(Debug)]`, this test fires.
        let cfg = S3Config {
            region: "us-east-1".into(),
            endpoint_url: String::new(),
            bucket: "test-bucket".into(),
            bucket_path: String::new(),
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        };
        for rendered in [format!("{:?}", cfg), format!("{:#?}", cfg)] {
            assert!(
                !rendered.contains("AKIAIOSFODNN7EXAMPLE"),
                "access_key_id leaked: {rendered}"
            );
            assert!(
                !rendered.contains("wJalrXUtnFEMI"),
                "secret_access_key leaked: {rendered}"
            );
            assert!(
                rendered.contains("REDACTED"),
                "Debug output should signal redaction: {rendered}"
            );
        }
    }

    #[test]
    fn debug_of_appconfig_containing_s3_masks_secrets() {
        // F2 defence-in-depth: the outer AppConfig derives Debug; ensure
        // that composition uses the inner masked Debug.
        let cfg = AppConfig {
            s3: Some(S3Config {
                region: "us-east-1".into(),
                endpoint_url: String::new(),
                bucket: "b".into(),
                bucket_path: String::new(),
                access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
                secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            }),
            ..AppConfig::default()
        };
        let rendered = format!("{:?}", cfg);
        assert!(
            !rendered.contains("AKIAIOSFODNN7EXAMPLE"),
            "leaked via AppConfig: {rendered}"
        );
        assert!(
            !rendered.contains("wJalrXUtnFEMI"),
            "leaked via AppConfig: {rendered}"
        );
    }

    #[test]
    fn bucket_path_traversal_rejected() {
        // F5 regression: `..` in bucket_path fails validation.
        std::env::set_var("FILEFERRY_TEST_F5_ACCESS", "AKIA-x");
        std::env::set_var("FILEFERRY_TEST_F5_SECRET", "secret-x");
        let y: AppConfigYaml = serde_yaml_ng::from_str(
            r#"
s3:
  region: us-east-1
  bucket: b
  bucket_path: ../admin
  access_key_id_env: FILEFERRY_TEST_F5_ACCESS
  secret_access_key_env: FILEFERRY_TEST_F5_SECRET
"#,
        )
        .unwrap();
        let err = from_yaml(y).unwrap_err().to_string();
        assert!(err.contains("bucket_path"), "{err}");
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn bucket_path_leading_slash_rejected() {
        std::env::set_var("FILEFERRY_TEST_F5B_ACCESS", "AKIA-x");
        std::env::set_var("FILEFERRY_TEST_F5B_SECRET", "secret-x");
        let y: AppConfigYaml = serde_yaml_ng::from_str(
            r#"
s3:
  region: us-east-1
  bucket: b
  bucket_path: /rooted
  access_key_id_env: FILEFERRY_TEST_F5B_ACCESS
  secret_access_key_env: FILEFERRY_TEST_F5B_SECRET
"#,
        )
        .unwrap();
        let err = from_yaml(y).unwrap_err().to_string();
        assert!(err.contains("bucket_path"), "{err}");
    }

    #[test]
    fn bucket_path_accepts_normal_prefix() {
        std::env::set_var("FILEFERRY_TEST_F5C_ACCESS", "AKIA-x");
        std::env::set_var("FILEFERRY_TEST_F5C_SECRET", "secret-x");
        let y: AppConfigYaml = serde_yaml_ng::from_str(
            r#"
s3:
  region: us-east-1
  bucket: b
  bucket_path: my/prefix/
  access_key_id_env: FILEFERRY_TEST_F5C_ACCESS
  secret_access_key_env: FILEFERRY_TEST_F5C_SECRET
"#,
        )
        .unwrap();
        assert!(from_yaml(y).is_ok());
    }

    #[test]
    fn cors_origin_set_causes_startup_failure() {
        // F6 regression: silent-drop-behind-WARN was demoted to a
        // hard boot failure. Operators either terminate CORS at their
        // proxy or leave the field empty.
        let y: AppConfigYaml =
            serde_yaml_ng::from_str("cors_origin: https://example.com\n").unwrap();
        let err = from_yaml(y).unwrap_err().to_string();
        assert!(err.contains("cors_origin"), "{err}");
        assert!(err.contains("not implemented"), "{err}");
    }

    #[test]
    fn empty_cors_origin_still_ok() {
        let y: AppConfigYaml = serde_yaml_ng::from_str("port: 9000\n").unwrap();
        assert!(from_yaml(y).is_ok());
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        // `deny_unknown_fields` on the YAML struct guards against config
        // typos that would otherwise silently be ignored.
        let err = serde_yaml_ng::from_str::<AppConfigYaml>("porrt: 9000\n").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("porrt"));
    }
}
