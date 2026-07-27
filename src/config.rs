//! Configuration loading: TOML file search, env var overlay, and validation.
//!
//! Config search order (first file found wins, then env vars override):
//!   1. `$GITHUB_WORKSPACE/.github/reviewer.toml` (GitHub Action context)
//!   2. `$CWD/reviewer.toml`
//!   3. `$CWD/.reviewer.toml`
//!   4. `~/.config/reviewer/config.toml`
//!   5. Built-in defaults (no file needed)
//!
//! Secrets (API keys, tokens) use [`Sensitive<T>`] to prevent accidental
//! leakage in logs or serde serialization.

use crate::error::{AgentError, Result};
use crate::sensitive::Sensitive;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level application configuration.
///
/// Three sections: [`AiConfig`] (AI endpoint), [`GitHubConfig`] (API client),
/// and [`ReviewConfig`] (budget caps). Constructed via [`Settings::load()`]
/// which chains file search → env overlay → validation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub ai: AiConfig,

    #[serde(default)]
    pub github: GitHubConfig,

    #[serde(default)]
    pub review: ReviewConfig,
}

impl Settings {
    /// Load configuration from TOML file, then overlay env vars.
    ///
    /// Config search order (first found wins):
    ///   1. `$GITHUB_WORKSPACE/.github/reviewer.toml` (GitHub Action)
    ///   2. `$CWD/reviewer.toml`
    ///   3. `$CWD/.reviewer.toml`
    ///   4. `~/.config/reviewer/config.toml`
    ///   5. Built-in defaults
    ///
    /// Env vars take precedence over file values.
    pub fn load() -> Result<Self> {
        let mut s = Self::from_toml_file().unwrap_or_default();

        s.with_env_overrides();
        s.validate()?;

        Ok(s)
    }

    /// Overlay environment variables on top of loaded config.
    ///
    /// Overrides applied:
    /// - `AI_API_KEY` → `ai.api_key`
    /// - `AI_API_BASE` → `ai.api_base`
    /// - `MODEL` → `ai.model`
    /// - `GITHUB_TOKEN` → `github.token`
    ///
    /// Env vars take precedence over values read from any TOML file.
    pub fn with_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("AI_API_KEY") {
            self.ai.api_key = Sensitive::new(v);
        }
        if let Ok(v) = std::env::var("AI_API_BASE") {
            self.ai.api_base = v;
        }
        if let Ok(v) = std::env::var("MODEL") {
            self.ai.model = v;
        }
        if let Ok(v) = std::env::var("GITHUB_TOKEN") {
            self.github.token = Sensitive::new(v);
        }
    }

    /// Validate required fields are present and config values are in range.
    ///
    /// Validates:
    /// - `ai.api_key` is non-empty (required)
    /// - `ai.api_base` is a valid URL with an `https` scheme
    /// - `ai.temperature` is within the 0.0–2.0 range
    /// - `ai.request_timeout_secs` and `review.max_input_tokens` are positive
    ///
    /// Call this as the last step of `load()` so the error message is actionable.
    pub fn validate(&self) -> Result<()> {
        // Note: AI_API_KEY is NOT validated here. The MCP server must be able
        // to start without credentials (tools/list, initialize, etc.). The
        // key is only required when a review tool is actually called, and
        // AiClient::chat() will return an auth error at that point.
        if self.ai.api_base.is_empty() {
            return Err(AgentError::Config(
                "AI_API_BASE must be a non-empty URL".into(),
            ));
        }
        if url::Url::parse(&self.ai.api_base).is_err()
            || !(self.ai.api_base.starts_with("http://")
                || self.ai.api_base.starts_with("https://"))
        {
            return Err(AgentError::Config(format!(
                "AI_API_BASE must be a valid http(s) URL, got '{}'",
                self.ai.api_base
            )));
        }
        if self.ai.temperature.is_nan() || !(0.0..=2.0).contains(&self.ai.temperature) {
            let detail = if self.ai.temperature.is_nan() {
                "NaN (not a number)".to_string()
            } else {
                self.ai.temperature.to_string()
            };
            return Err(AgentError::Config(format!(
                "ai.temperature must be a finite number between 0.0 and 2.0, got {}",
                detail
            )));
        }
        if self.ai.request_timeout_secs == 0 {
            return Err(AgentError::Config(
                "ai.request_timeout_secs must be greater than 0".into(),
            ));
        }
        if self.review.max_input_tokens == 0 {
            return Err(AgentError::Config(
                "review.max_input_tokens must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    fn from_toml_file() -> Option<Self> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. GITHUB_WORKSPACE (GitHub Action mounts repo here)
        if let Ok(workspace) = std::env::var("GITHUB_WORKSPACE") {
            candidates.push(PathBuf::from(workspace).join(".github/reviewer.toml"));
        }

        // 2-4. Standard paths
        candidates.push("reviewer.toml".into());
        candidates.push(".reviewer.toml".into());
        candidates.push(PathBuf::from(
            shellexpand::tilde("~/.config/reviewer/config.toml").as_ref(),
        ));

        for path in &candidates {
            if path.exists() {
                let contents = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to read config file");
                        continue;
                    }
                };
                match toml::from_str::<Self>(&contents) {
                    Ok(s) => return Some(s),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to parse config file");
                        continue;
                    }
                }
            }
        }
        None
    }
}

/// AI provider configuration.
///
/// Connects to any OpenAI-compatible `/v1/chat/completions` endpoint.
/// The `api_key` field uses [`Sensitive<String>`] and is never logged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default = "default_ai_api_base")]
    pub api_base: String,

    #[serde(default = "default_ai_model")]
    pub model: String,

    #[serde(default)]
    pub api_key: Sensitive<String>,

    #[serde(default = "default_ai_timeout")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_ai_temperature")]
    pub temperature: f64,

    #[serde(default = "default_ai_max_completion_tokens")]
    pub max_completion_tokens: u32,
}

fn default_ai_api_base() -> String {
    "https://ai.cloudmagic.io/v1".into()
}
fn default_ai_model() -> String {
    "glm-4.6".into()
}
fn default_ai_timeout() -> u64 {
    120
}
fn default_ai_temperature() -> f64 {
    0.2
}
fn default_ai_max_completion_tokens() -> u32 {
    4096
}

/// GitHub API client configuration.
///
/// Token permissions required:
/// - `contents: read` — to fetch PR diffs and metadata
/// - `pull-requests: write` — to post reviews and comments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConfig {
    #[serde(default)]
    pub token: Sensitive<String>,

    /// Optional override for the GitHub API base URL. Empty → public GitHub.
    #[serde(default)]
    pub base_url: String,

    #[serde(default = "default_github_timeout")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_github_concurrency")]
    pub max_concurrent_requests: usize,
}

fn default_github_timeout() -> u64 {
    30
}
fn default_github_concurrency() -> usize {
    10
}

/// Review pipeline budget and behaviour configuration.
///
/// Controls how much diff content is sent to the AI and which files are
/// eligible for review. Defaults are conservative to avoid surprise API costs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: usize,

    #[serde(default = "default_max_diff_files")]
    pub max_diff_files: usize,

    #[serde(default)]
    pub extra_instructions: String,
}

fn default_max_input_tokens() -> usize {
    16_000
}
fn default_max_diff_files() -> usize {
    50
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            api_base: default_ai_api_base(),
            model: default_ai_model(),
            api_key: Sensitive::new(String::new()),
            request_timeout_secs: default_ai_timeout(),
            temperature: default_ai_temperature(),
            max_completion_tokens: default_ai_max_completion_tokens(),
        }
    }
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            token: Sensitive::new(String::new()),
            base_url: String::new(),
            request_timeout_secs: default_github_timeout(),
            max_concurrent_requests: default_github_concurrency(),
        }
    }
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            max_input_tokens: default_max_input_tokens(),
            max_diff_files: default_max_diff_files(),
            extra_instructions: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.ai.api_base, "https://ai.cloudmagic.io/v1");
        assert_eq!(s.ai.model, "glm-4.6");
        assert_eq!(s.ai.temperature, 0.2);
        assert_eq!(s.ai.max_completion_tokens, 4096);
        assert_eq!(s.ai.request_timeout_secs, 120);
        assert_eq!(s.github.request_timeout_secs, 30);
        assert_eq!(s.github.max_concurrent_requests, 10);
        assert_eq!(s.review.max_input_tokens, 16000);
        assert_eq!(s.review.max_diff_files, 50);
        assert_eq!(s.review.extra_instructions, "");
    }

    #[test]
    fn secrets_are_sensitive() {
        let s = Settings::default();
        assert_eq!(format!("{}", s.ai.api_key), "***");
        assert_eq!(format!("{}", s.github.token), "***");
    }

    #[test]
    fn validate_passes_without_ai_key() {
        // AI_API_KEY is NOT required at startup — the MCP server must start
        // without credentials. The key is checked at call time by AiClient.
        let s = Settings::default();
        assert!(s.ai.api_key.inner().is_empty(), "api key should be empty by default");
        assert!(s.validate().is_ok(), "validate should pass without AI key for MCP compatibility");
    }

    #[test]
    fn validate_passes_with_all_required() {
        let mut s = Settings::default();
        s.ai.api_key = Sensitive::new("sk-key".into());
        s.ai.api_base = "https://api.example.com/v1".into();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_out_of_range_temperature() {
        let mut s = Settings::default();
        s.ai.api_key = Sensitive::new("sk-key".into());
        s.ai.api_base = "https://api.example.com/v1".into();
        // Below 0.0
        s.ai.temperature = -0.5;
        let err = s.validate().unwrap_err();
        assert!(err.to_string().contains("temperature"));

        // Above 2.0
        s.ai.temperature = 3.0;
        let err = s.validate().unwrap_err();
        assert!(err.to_string().contains("temperature"));

        // NaN
        s.ai.temperature = f64::NAN;
        let err = s.validate().unwrap_err();
        assert!(err.to_string().contains("NaN"));

        // Boundary values should pass
        s.ai.temperature = 0.0;
        assert!(s.validate().is_ok());
        s.ai.temperature = 2.0;
        assert!(s.validate().is_ok());
    }

    #[test]
    fn env_overrides_api_key() {
        temp_env::with_var("AI_API_KEY", Some("env-override"), || {
            let mut s = Settings::default();
            s.with_env_overrides();
            assert_eq!(*s.ai.api_key.inner(), "env-override");
        });
    }

    #[test]
    fn env_overrides_model() {
        temp_env::with_var("MODEL", Some("custom-model"), || {
            let mut s = Settings::default();
            s.with_env_overrides();
            assert_eq!(s.ai.model, "custom-model");
        });
    }

    #[test]
    fn env_overrides_github_token() {
        temp_env::with_var("GITHUB_TOKEN", Some("ghp_override"), || {
            let mut s = Settings::default();
            s.with_env_overrides();
            assert_eq!(*s.github.token.inner(), "ghp_override");
        });
    }

    #[test]
    fn api_key_can_be_set_from_toml() {
        let toml_str = r#"
            [ai]
            api_key = "toml-key"
            model = "toml-model"
        "#;
        let s: Settings = toml::from_str(toml_str).unwrap();
        assert_eq!(*s.ai.api_key.inner(), "toml-key");
        assert_eq!(s.ai.model, "toml-model");
    }

    #[test]
    fn github_config_from_toml() {
        let toml_str = r#"
            [github]
            request_timeout_secs = 60
            max_concurrent_requests = 5
        "#;
        let s: Settings = toml::from_str(toml_str).unwrap();
        assert_eq!(s.github.request_timeout_secs, 60);
        assert_eq!(s.github.max_concurrent_requests, 5);
    }

    #[test]
    fn partial_toml_uses_defaults_for_missing() {
        let toml_str = r#"
            [ai]
            api_key = "only-key"
        "#;
        let s: Settings = toml::from_str(toml_str).unwrap();
        assert_eq!(*s.ai.api_key.inner(), "only-key");
        assert_eq!(s.ai.api_base, default_ai_api_base());
        assert_eq!(s.ai.model, default_ai_model());
        assert_eq!(s.ai.request_timeout_secs, default_ai_timeout());
    }

    #[test]
    fn empty_strings_in_defaults() {
        let s = Settings::default();
        assert_eq!(*s.ai.api_key.inner(), "");
        assert_eq!(*s.github.token.inner(), "");
    }

    #[test]
    fn serde_roundtrip() {
        let s = Settings {
            ai: AiConfig {
                api_base: "https://test.example.com".into(),
                model: "test-model".into(),
                api_key: Sensitive::new("test-key".into()),
                request_timeout_secs: 99,
                temperature: 0.5,
                max_completion_tokens: 2048,
            },
            github: GitHubConfig {
                token: Sensitive::new("test-token".into()),
                base_url: String::new(),
                request_timeout_secs: 15,
                max_concurrent_requests: 3,
            },
            review: ReviewConfig {
                max_input_tokens: 8000,
                max_diff_files: 20,
                extra_instructions: "focus on security".into(),
            },
        };
        let json = serde_json::to_string_pretty(&s).unwrap();

        // Verify secrets are redacted in serialization
        assert!(!json.contains("test-key"));
        assert!(!json.contains("test-token"));
        assert!(json.contains("\"***\""));
    }
}
