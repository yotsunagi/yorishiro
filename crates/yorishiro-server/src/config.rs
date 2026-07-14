//! Optional `config.yml` support. This binary has always been configured entirely through
//! environment variables (see `.env.example`); this module lets an operator put the same
//! settings in a YAML file instead, without changing how any of them are actually consumed.
//!
//! It does this by reading the file (if present) and, for each setting it sets, writing the
//! corresponding environment variable -- but only if that variable isn't already set. Every
//! existing `std::env::var("YSR_...")` call site elsewhere in this crate and in
//! `yorishiro-core` is untouched: environment variables still win when both are set, and a
//! deployment with no `config.yml` behaves exactly as before.
//!
//! This module is private to the `yorishiro-server` binary (not re-exported from `lib.rs`), so
//! it has no effect on `yorishiro-hosted-server`, which embeds this crate's library API
//! directly rather than going through this binary's `main`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileConfig {
    database_url: Option<String>,
    bind: Option<String>,
    web_dir: Option<String>,
    cors_origins: Option<String>,
    max_tenants: Option<i64>,
    rust_log: Option<String>,
    #[serde(default)]
    embedding: EmbeddingConfig,
    #[serde(default)]
    logging: LoggingConfig,
    #[serde(default)]
    auth_rate_limit: AuthRateLimitConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EmbeddingConfig {
    provider: Option<String>,
    dimensions: Option<u32>,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    send_dimensions_param: Option<bool>,
    onnx_model_path: Option<String>,
    onnx_tokenizer_path: Option<String>,
    onnx_max_sequence_length: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LoggingConfig {
    target: Option<String>,
    dir: Option<String>,
    syslog_socket: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AuthRateLimitConfig {
    max: Option<u32>,
    window_secs: Option<u64>,
}

/// Sets `key` to `value` unless it's already set in the environment.
///
/// # Safety
///
/// Must only be called from a synchronous prologue in `main`, before the tokio runtime (or any
/// other thread) starts and before anything else reads or writes the environment -- `set_var`
/// is unsound under concurrent env access, which this ordering rules out.
unsafe fn apply_if_unset(key: &str, value: Option<String>) {
    if let Some(value) = value
        && std::env::var_os(key).is_none()
    {
        unsafe { std::env::set_var(key, value) };
    }
}

/// Loads `config.yml` (path overridable via `YSR_CONFIG_PATH`, defaulting to `config.yml` in
/// the working directory) and materializes its settings into the process environment. A
/// missing file is not an error -- it just means every setting stays exactly as the
/// environment already has it, which is the same as if this function were never called.
///
/// # Safety
///
/// See `apply_if_unset`: must be called from `main`'s synchronous prologue, before the tokio
/// runtime starts.
pub unsafe fn load_and_apply_env_overrides() -> Result<()> {
    let path = std::env::var("YSR_CONFIG_PATH").unwrap_or_else(|_| "config.yml".into());
    let path = Path::new(&path);
    if !path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file '{}'", path.display()))?;
    let config: FileConfig = serde_yaml_ng::from_str(&contents)
        .with_context(|| format!("failed to parse config file '{}'", path.display()))?;

    // SAFETY: forwarded from this function's own contract.
    unsafe {
        apply_if_unset("DATABASE_URL", config.database_url);
        apply_if_unset("YSR_BIND", config.bind);
        apply_if_unset("YSR_WEB_DIR", config.web_dir);
        apply_if_unset("YSR_CORS_ORIGINS", config.cors_origins);
        apply_if_unset(
            "YORISHIRO_MAX_TENANTS",
            config.max_tenants.map(|n| n.to_string()),
        );
        apply_if_unset("RUST_LOG", config.rust_log);

        apply_if_unset("YSR_EMBEDDING_PROVIDER", config.embedding.provider);
        apply_if_unset(
            "YSR_EMBEDDING_DIMENSIONS",
            config.embedding.dimensions.map(|n| n.to_string()),
        );
        apply_if_unset("YSR_EMBEDDING_BASE_URL", config.embedding.base_url);
        apply_if_unset("YSR_EMBEDDING_MODEL", config.embedding.model);
        apply_if_unset("YSR_EMBEDDING_API_KEY", config.embedding.api_key);
        apply_if_unset(
            "YSR_EMBEDDING_SEND_DIMENSIONS_PARAM",
            config
                .embedding
                .send_dimensions_param
                .map(|b| b.to_string()),
        );
        apply_if_unset("YSR_ONNX_MODEL_PATH", config.embedding.onnx_model_path);
        apply_if_unset(
            "YSR_ONNX_TOKENIZER_PATH",
            config.embedding.onnx_tokenizer_path,
        );
        apply_if_unset(
            "YSR_ONNX_MAX_SEQUENCE_LENGTH",
            config
                .embedding
                .onnx_max_sequence_length
                .map(|n| n.to_string()),
        );

        apply_if_unset("YSR_LOG_TARGET", config.logging.target);
        apply_if_unset("YSR_LOG_DIR", config.logging.dir);
        apply_if_unset("YSR_SYSLOG_SOCKET", config.logging.syslog_socket);

        apply_if_unset(
            "YSR_AUTH_RATE_LIMIT_MAX",
            config.auth_rate_limit.max.map(|n| n.to_string()),
        );
        apply_if_unset(
            "YSR_AUTH_RATE_LIMIT_WINDOW_SECS",
            config.auth_rate_limit.window_secs.map(|n| n.to_string()),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-wide state; serialize tests through this lock rather than racing
    // each other (same pattern as `yorishiro_core::tenancy`'s env tests).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        keys: Vec<&'static str>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(keys: Vec<&'static str>) -> Self {
            let lock = ENV_LOCK.lock().unwrap();
            for key in &keys {
                // SAFETY: serialized by ENV_LOCK, no other threads touch these keys.
                unsafe { std::env::remove_var(key) };
            }
            Self { keys, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in &self.keys {
                // SAFETY: serialized by ENV_LOCK, no other threads touch these keys.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    fn write_config(dir: &std::path::Path, yaml: &str) -> std::path::PathBuf {
        let path = dir.join("config.yml");
        std::fs::write(&path, yaml).unwrap();
        path
    }

    #[test]
    fn yaml_value_is_applied_when_env_is_unset() {
        let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
        // SAFETY: serialized by ENV_LOCK via EnvGuard.
        unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

        unsafe { load_and_apply_env_overrides() }.unwrap();

        assert_eq!(std::env::var("YSR_BIND").unwrap(), "127.0.0.1:9000");
    }

    #[test]
    fn env_var_wins_over_yaml_value() {
        let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "bind: 127.0.0.1:9000\n");
        // SAFETY: serialized by ENV_LOCK via EnvGuard.
        unsafe {
            std::env::set_var("YSR_CONFIG_PATH", &path);
            std::env::set_var("YSR_BIND", "127.0.0.1:1234");
        }

        unsafe { load_and_apply_env_overrides() }.unwrap();

        assert_eq!(std::env::var("YSR_BIND").unwrap(), "127.0.0.1:1234");
    }

    #[test]
    fn missing_config_file_is_a_no_op() {
        let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH", "YSR_BIND"]);
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_LOCK via EnvGuard.
        unsafe { std::env::set_var("YSR_CONFIG_PATH", dir.path().join("does-not-exist.yml")) };

        unsafe { load_and_apply_env_overrides() }.unwrap();

        assert!(std::env::var_os("YSR_BIND").is_none());
    }

    #[test]
    fn nested_embedding_settings_are_applied() {
        let _guard = EnvGuard::new(vec![
            "YSR_CONFIG_PATH",
            "YSR_EMBEDDING_PROVIDER",
            "YSR_EMBEDDING_DIMENSIONS",
            "YSR_ONNX_MODEL_PATH",
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "embedding:\n  provider: local\n  dimensions: 768\n  onnx_model_path: /models/model.onnx\n",
        );
        // SAFETY: serialized by ENV_LOCK via EnvGuard.
        unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

        unsafe { load_and_apply_env_overrides() }.unwrap();

        assert_eq!(std::env::var("YSR_EMBEDDING_PROVIDER").unwrap(), "local");
        assert_eq!(std::env::var("YSR_EMBEDDING_DIMENSIONS").unwrap(), "768");
        assert_eq!(
            std::env::var("YSR_ONNX_MODEL_PATH").unwrap(),
            "/models/model.onnx"
        );
    }

    #[test]
    fn unknown_key_is_a_hard_error() {
        let _guard = EnvGuard::new(vec!["YSR_CONFIG_PATH"]);
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), "not_a_real_setting: true\n");
        // SAFETY: serialized by ENV_LOCK via EnvGuard.
        unsafe { std::env::set_var("YSR_CONFIG_PATH", &path) };

        let err = unsafe { load_and_apply_env_overrides() }.unwrap_err();

        assert!(err.to_string().contains("failed to parse config file"));
    }
}
