//! Internal OTLP span-export layer — **stubbed**.
//!
//! [LOCAL-DEV] Telemetry removed: the opentelemetry / opentelemetry-otlp
//! export stack is no longer compiled in. The layer keeps its position in the
//! tracing registry (spans still exist locally for structured logging), but
//! nothing is ever exported off-host. Config types keep their shape so the
//! cross-crate constructors (shell `credential_provider`) compile unchanged.

use std::sync::Arc;

use xai_grok_auth::AuthCredentialProvider;

pub use xai_grok_auth::AuthCredentialProvider as _AuthCredentialProviderReexport;

/// Live credential source (kept for API compatibility; unused by the stub).
pub struct OtelLayerConfig {
    /// Live credential source. Read on every batch export to obtain a fresh
    /// bearer token for the OTLP `Authorization` header.
    pub credentials: Arc<dyn AuthCredentialProvider>,
    /// Value for the `X-XAI-Token-Auth` header (typically `"xai-grok-cli"`).
    pub token_header_value: String,
    /// Optional extra access key for traces.
    pub alpha_test_key: Option<String>,
    pub exporter: OtelExporterConfig,
}

/// Static identity of the client emitting telemetry.
#[derive(Debug, Clone, Copy)]
pub struct OtelClientInfo {
    /// Binary name (`grok-pager`) -> `client.name`.
    pub client_name: &'static str,
    /// Front-end client version -> `client.version`.
    pub client_version: &'static str,
    /// Engine build (version + commit) -> `service.version`.
    pub service_version: &'static str,
    /// How the session was launched (`cli`/`headless`/`agent`) -> `app.entrypoint`.
    pub app_entrypoint: &'static str,
}

/// OTLP trace-export transport settings.
#[derive(Debug, Default, Clone)]
pub struct OtelExporterConfig {
    /// Full OTLP traces endpoint URL (e.g. `https://cli-chat-proxy.grok.com/v1/traces`).
    pub traces_url: String,
    /// `OTEL_EXPORTER_OTLP_HEADERS` pairs.
    pub extra_headers: Vec<(String, String)>,
    /// `OTEL_TRACES_EXPORT_INTERVAL` batch flush interval. `None` = SDK default.
    pub export_interval: Option<std::time::Duration>,
    /// `OTEL_EXPORTER_OTLP_TIMEOUT` export timeout. `None` = 10s default.
    pub timeout: Option<std::time::Duration>,
    /// `false` when `OTEL_TRACES_EXPORTER=none`: spans created, never exported.
    pub enabled: bool,
}

/// A layer that discards everything: spans still flow through the tracing
/// registry (other layers see them), but no exporter is attached.
struct NoExportLayer;

impl<S> tracing_subscriber::layer::Layer<S> for NoExportLayer where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>
{
}

/// Creates the (no-op) OpenTelemetry bridge layer. Spans are never exported.
pub fn build_otel_layer<S>(
    _client: OtelClientInfo,
    _config: OtelLayerConfig,
) -> impl tracing_subscriber::layer::Layer<S>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    NoExportLayer
}

/// Shut down the (non-existent) OTLP pipeline. No-op.
pub fn shutdown_otel() {}

/// Guard type kept for API compatibility; no-op on drop.
pub struct OtelGuard;

/// RAII guard matching the upstream signature. No-op.
pub fn otel_guard() -> OtelGuard {
    OtelGuard
}
