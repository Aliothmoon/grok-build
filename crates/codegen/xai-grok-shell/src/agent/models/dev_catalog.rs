//! models.dev community catalog → [`ModelEntry`] expansion.
//!
//! Fetches `https://models.dev/api.json` (a static, unauthenticated JSON
//! document maintained by the community, the same source opencode uses),
//! maps each provider whose credential env var is set into first-class
//! catalog entries keyed `provider/model`, and caches the result on disk so
//! restarts and offline sessions still see the catalog.
//!
//! All failures are non-fatal: the dev catalog is purely additive — built-in
//! models, the `/v1/models` prefetch, and user `[model.*]` overrides all keep
//! their existing precedence (see `resolve_model_list_with_dev`).

use std::collections::BTreeMap;

use indexmap::IndexMap;

use crate::agent::config::{Config as AgentConfig, ModelEntry, ModelEntryConfig};
use xai_grok_sampler::AuthScheme;
use xai_grok_sampling_types::ApiBackend;

pub(crate) const DEV_CATALOG_URL: &str = "https://models.dev/api.json";
pub(crate) const DEV_CATALOG_CACHE_FILE: &str = "models_dev_catalog.json";
/// Refresh at most once a day; a stale cache is still served (best-effort
/// offline support) and refreshed in the background.
pub(crate) const DEV_CATALOG_TTL: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// models.dev provider ids intentionally not expanded.
const SKIPPED_PROVIDERS: &[&str] = &[
    // xAI models are covered by the built-in catalog.
    "xai",
];

// ── Wire types (defensive subset of the models.dev schema) ─────────────────

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct DevCatalog {
    #[serde(flatten)]
    providers: BTreeMap<String, DevProvider>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct DevProvider {
    name: Option<String>,
    /// API dialect: "openai-completions" | "openai-responses" |
    /// "anthropic-messages" | "google-gemini" | ... — unknown values fall
    /// back to OpenAI chat-completions semantics.
    api: Option<String>,
    base_url: Option<String>,
    /// Name of the env var holding this provider's API key.
    env_key: Option<String>,
    models: BTreeMap<String, DevModel>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct DevModel {
    name: Option<String>,
    context_window: Option<u64>,
    max_tokens: Option<u64>,
    reasoning: bool,
    tool_use: Option<bool>,
    #[serde(default)]
    pricing: DevPricing,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct DevPricing {
    /// USD per input token, as a string ("0.000003").
    prompt: Option<String>,
    /// USD per output token, as a string.
    completion: Option<String>,
}

// ── Provider dialect mapping ────────────────────────────────────────────────

struct Dialect {
    api_backend: ApiBackend,
    auth_scheme: AuthScheme,
    extra_headers: IndexMap<String, String>,
    /// See [`ModelInfo::model_family`]: the family that mints this model's
    /// conversation items. A cross-family mid-session switch forces a
    /// (lossy) compaction, so it must match the wire dialect, not the brand.
    model_family: &'static str,
}

fn dialect_for(api: Option<&str>) -> Dialect {
    let mut extra_headers = IndexMap::new();
    let (api_backend, auth_scheme) = match api.unwrap_or_default() {
        "anthropic" | "anthropic-messages" | "anthropic-completions" => {
            extra_headers.insert("anthropic-version".to_string(), "2023-06-01".to_string());
            (ApiBackend::Messages, AuthScheme::XApiKey)
        }
        "openai-responses" => (ApiBackend::Responses, AuthScheme::Bearer),
        _ => (ApiBackend::ChatCompletions, AuthScheme::Bearer),
    };
    let model_family = match api_backend {
        ApiBackend::Messages => "anthropic",
        _ => "openai",
    };
    Dialect {
        api_backend,
        auth_scheme,
        extra_headers,
        model_family,
    }
}

/// Reasoning-effort menu for models.dev models that set `reasoning: true`.
/// A conservative cross-provider set; xAI-specific levels stay on built-ins.
fn default_reasoning_efforts() -> Vec<xai_grok_sampling_types::ReasoningEffortOption> {
    use xai_grok_sampling_types::ReasoningEffort as RE;
    [("low", RE::Low), ("medium", RE::Medium), ("high", RE::High)]
        .into_iter()
        .map(
            |(id, value)| xai_grok_sampling_types::ReasoningEffortOption {
                id: id.to_string(),
                label: id.to_string(),
                value,
                description: None,
                default: value == RE::Medium,
            },
        )
        .collect()
}

// ── Expansion ───────────────────────────────────────────────────────────────

/// `[models] dev_catalog` gate: `Some(false)` disables the whole feature.
pub(crate) fn enabled(cfg: &AgentConfig) -> bool {
    cfg.models.dev_catalog != Some(false)
}

fn env_var_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

/// Expand a parsed models.dev document into catalog entries.
///
/// A provider is expanded only when models.dev carries a `base_url` and an
/// `env_key`, and that env var is actually set — models the user cannot
/// authenticate to never appear in the picker.
pub(crate) fn expand(json: &str) -> IndexMap<String, ModelEntry> {
    let catalog: DevCatalog = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "dev catalog: parse failed");
            return IndexMap::new();
        }
    };
    let mut out = IndexMap::new();
    for (pid, provider) in &catalog.providers {
        if SKIPPED_PROVIDERS.contains(&pid.as_str()) {
            continue;
        }
        let Some(base_url) = provider.base_url.as_deref() else {
            tracing::debug!(provider = %pid, "dev catalog: no base_url, skipping");
            continue;
        };
        let Some(env_key) = provider.env_key.as_deref() else {
            tracing::debug!(provider = %pid, "dev catalog: no env_key, skipping");
            continue;
        };
        if !env_var_set(env_key) {
            tracing::debug!(provider = %pid, env_key, "dev catalog: env unset, skipping");
            continue;
        }
        let dialect = dialect_for(provider.api.as_deref());
        let provider_label = provider.name.clone().unwrap_or_else(|| pid.clone());
        for (mid, model) in &provider.models {
            let key = format!("{pid}/{mid}");
            let context_window = model
                .context_window
                .and_then(std::num::NonZeroU64::new)
                .unwrap_or_else(|| std::num::NonZeroU64::new(200_000).expect("nonzero"));
            let mut description = format!(
                "via {provider_label} · {}k ctx",
                context_window.get() / 1000
            );
            if let Some((pin, pout)) = price_per_million(&model.pricing) {
                description.push_str(&format!(" · ${pin}/${pout} per M tok"));
            }
            let config = ModelEntryConfig {
                id: Some(key.clone()),
                model: mid.clone(),
                model_family: Some(dialect.model_family.to_string()),
                base_url: base_url.to_string(),
                name: Some(model.name.clone().unwrap_or_else(|| mid.clone())),
                description: Some(description),
                max_completion_tokens: model.max_tokens.map(|t| t as u32),
                temperature: None,
                top_p: None,
                api_key: None,
                env_key: Some(crate::agent::config::EnvKeys::single(env_key)),
                api_backend: dialect.api_backend.clone(),
                auth_scheme: Some(dialect.auth_scheme),
                reasoning_effort: None,
                supports_reasoning_effort: model.reasoning,
                reasoning_efforts: if model.reasoning {
                    default_reasoning_efforts()
                } else {
                    Vec::new()
                },
                extra_headers: dialect.extra_headers.clone(),
                context_window,
                auto_compact_threshold_percent: None,
                system_prompt_label: None,
                api_base_url: None,
                use_concise: false,
                agent_type: "grok-build-plan".to_string(),
                inference_idle_timeout_secs: None,
                max_retries: None,
                hidden: false,
                supported_in_api: true,
                supports_backend_search: false,
                compactions_remaining: None,
                compaction_at_tokens: None,
                show_model_fingerprint: false,
                stream_tool_calls: None,
                laziness_detector: Default::default(),
            };
            out.insert(key, ModelEntry::from_config_entry(&config));
        }
    }
    out
}

/// Convert models.dev per-token string prices to USD-per-million, truncated
/// to 2 decimals for the picker description.
fn price_per_million(pricing: &DevPricing) -> Option<(f64, f64)> {
    let parse = |v: &Option<String>| {
        v.as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|p| (p * 1_000_000.0 * 100.0).round() / 100.0)
    };
    Some((parse(&pricing.prompt)?, parse(&pricing.completion)?))
}

// ── Disk cache + fetch ──────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct DevCatalogCache {
    fetched_at: chrono::DateTime<chrono::Utc>,
    /// Raw models.dev JSON, re-expanded on load so entry mapping stays in
    /// sync with this build even when the cache was written by an older one.
    raw: String,
}

fn cache_path() -> std::path::PathBuf {
    crate::util::grok_home::grok_home().join(DEV_CATALOG_CACHE_FILE)
}

/// Load the cached catalog (stale caches are still served). Best-effort.
pub(crate) fn load_cached() -> IndexMap<String, ModelEntry> {
    let Ok(raw_file) = std::fs::read_to_string(cache_path()) else {
        return IndexMap::new();
    };
    let Ok(cache) = serde_json::from_str::<DevCatalogCache>(&raw_file) else {
        return IndexMap::new();
    };
    expand(&cache.raw)
}

fn cache_is_fresh() -> bool {
    let Ok(raw_file) = std::fs::read_to_string(cache_path()) else {
        return false;
    };
    serde_json::from_str::<DevCatalogCache>(&raw_file)
        .ok()
        .is_some_and(|c| {
            chrono::Utc::now().signed_duration_since(c.fetched_at)
                < chrono::Duration::from_std(DEV_CATALOG_TTL).unwrap_or_default()
        })
}

/// Fetch the models.dev document and refresh the disk cache. Returns the
/// expanded entries, or an empty map on any failure (caller keeps the stale
/// cache in that case).
pub(crate) async fn fetch_and_cache() -> IndexMap<String, ModelEntry> {
    let client = xai_grok_http::shared_client();
    let resp = client
        .get(DEV_CATALOG_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await;
    let body = match resp {
        Ok(r) if r.status().is_success() => r.text().await,
        Ok(r) => {
            tracing::info!(status = %r.status(), "dev catalog: fetch non-success");
            return IndexMap::new();
        }
        Err(e) => {
            tracing::info!(error = %e, "dev catalog: fetch failed");
            return IndexMap::new();
        }
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => {
            tracing::info!(error = %e, "dev catalog: body read failed");
            return IndexMap::new();
        }
    };
    // Validate before caching so a broken document never poisons the cache.
    let entries = expand(&body);
    let cache = DevCatalogCache {
        fetched_at: chrono::Utc::now(),
        raw: body,
    };
    if let Ok(json) = serde_json::to_string(&cache)
        && let Err(e) = std::fs::write(cache_path(), json)
    {
        tracing::warn!(error = %e, "dev catalog: cache write failed");
    }
    tracing::debug!(count = entries.len(), "dev catalog: refreshed");
    entries
}

/// Whether a background refresh is worth doing right now.
pub(crate) fn needs_refresh() -> bool {
    !cache_is_fresh()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"{
            "anthropic": {
                "name": "Anthropic",
                "api": "anthropic-messages",
                "base_url": "https://api.anthropic.com/v1",
                "env_key": "ANTHROPIC_API_KEY",
                "models": {
                    "claude-opus-4-6": {
                        "name": "Claude Opus 4.6",
                        "context_window": 200000,
                        "max_tokens": 32000,
                        "reasoning": true,
                        "tool_use": true,
                        "pricing": { "prompt": "0.000003", "completion": "0.000015" }
                    }
                }
            },
            "openai": {
                "name": "OpenAI",
                "api": "openai-responses",
                "base_url": "https://api.openai.com/v1",
                "env_key": "OPENAI_API_KEY",
                "models": {
                    "gpt-5.2": { "name": "GPT-5.2", "context_window": 400000 }
                }
            },
            "xai": {
                "base_url": "https://api.x.ai/v1",
                "env_key": "XAI_API_KEY",
                "models": { "grok-9": {} }
            },
            "nokey": {
                "base_url": "https://example.com/v1",
                "models": { "m": {} }
            }
        }"#
    }

    #[test]
    fn expands_only_mapped_providers_with_credentials() {
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-test");
            std::env::set_var("OPENAI_API_KEY", "sk-test");
            std::env::remove_var("XAI_API_KEY");
        }
        let entries = expand(sample());
        assert!(entries.contains_key("anthropic/claude-opus-4-6"));
        assert!(entries.contains_key("openai/gpt-5.2"));
        // xai skipped by policy; nokey has no env_key.
        assert!(!entries.contains_key("xai/grok-9"));
        assert!(!entries.contains_key("nokey/m"));
    }

    #[test]
    fn anthropic_dialect_uses_messages_and_x_api_key() {
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-test") };
        let entries = expand(sample());
        let e = &entries["anthropic/claude-opus-4-6"];
        assert_eq!(e.info.api_backend, ApiBackend::Messages);
        assert_eq!(e.info.auth_scheme, AuthScheme::XApiKey);
        assert!(e.info.extra_headers.contains_key("anthropic-version"));
        assert!(e.info.supports_reasoning_effort);
        assert_eq!(e.info.reasoning_efforts.len(), 3);
        // description carries provider + context + pricing
        let d = e.info.description.as_deref().unwrap();
        assert!(d.contains("Anthropic") && d.contains("200k ctx"));
    }

    #[test]
    fn openai_dialect_defaults_to_declared_api() {
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-test") };
        let entries = expand(sample());
        let e = &entries["openai/gpt-5.2"];
        assert_eq!(e.info.api_backend, ApiBackend::Responses);
        assert_eq!(e.info.auth_scheme, AuthScheme::Bearer);
        assert!(!e.info.supports_reasoning_effort);
    }

    #[test]
    fn unknown_provider_defaults_to_chat_completions() {
        let d = dialect_for(Some("weird-dialect"));
        assert_eq!(d.api_backend, ApiBackend::ChatCompletions);
        assert_eq!(d.auth_scheme, AuthScheme::Bearer);
    }

    #[test]
    fn price_per_million_converts() {
        let p = DevPricing {
            prompt: Some("0.000003".into()),
            completion: Some("0.000015".into()),
        };
        assert_eq!(price_per_million(&p), Some((3.0, 15.0)));
    }
}
