//! External OTEL stream — **stubbed**.
//!
//! [LOCAL-DEV] Telemetry removed: the enterprise OTLP logs/metrics exporters
//! (opentelemetry / opentelemetry-otlp stack) are no longer compiled in. This
//! module keeps the public API surface — the settings gate state machine
//! (fail-closed semantics are load-bearing for the shell's startup gate), the
//! config types, and the lifecycle entry points as no-ops — so downstream
//! call sites compile unchanged.

use std::time::Duration;

pub mod config;
pub mod schema;
pub mod truncate;

pub use config::{ContentGates, ExternalOtelConfig, ExternalOtelFileConfig};

/// Identity *attributes* (plain id strings — never tokens). Derived from a
/// `CredentialSnapshot` at the telemetry-client init sites; updated post-auth
/// and on logout.
#[derive(Debug, Clone, Default)]
pub struct IdentityAttrs {
    pub user_id: Option<String>,
    pub organization_id: Option<String>,
    pub team_id: Option<String>,
    pub deployment_id: Option<String>,
}

impl IdentityAttrs {
    pub fn from_snapshot(snapshot: &xai_grok_auth::CredentialSnapshot) -> Self {
        Self {
            user_id: snapshot.user_id.clone(),
            organization_id: snapshot.organization_id.clone(),
            team_id: snapshot.team_id.clone(),
            deployment_id: snapshot.deployment_id.clone(),
        }
    }
}

/// Remote-settings policy for the external stream. **Restrictive-only by
/// construction**: there is deliberately no enable direction (remote settings
/// are fetched per-run and never persisted, so a remote "enable" could never
/// reach init).
#[derive(Debug, Clone, Copy, Default)]
pub struct ExternalOtelRemotePolicy {
    /// Remote-policy force-disable: flush, then drop subsequent emissions in-process.
    pub force_disable: bool,
    /// Force the content gates off regardless of local env/config.
    pub lock_content_gates: bool,
}

/// Snapshot of external-stream export-health counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportHealthSnapshot {
    pub records_dropped: u64,
    pub metric_exports_dropped: u64,
    pub export_failures: u64,
    pub export_successes: u64,
}

// ── Fail-closed settings gate (preserved verbatim) ─────────────────────────
//
/// Fail-closed OTEL gate. Defaults open; the leader closes it before init and
/// re-opens it when settings resolve.
///
/// Opening is the synchronizing event: `OtelGate::apply_and_open` applies the
/// remote force-disable (`active = false`) and then opens here, so an emitter
/// whose `Acquire` read observes the `Release` open also observes
/// `active = false`; the emit-path `active` load can therefore stay `Relaxed`.
/// The window-expiry open has no such pairing and relies on eventual
/// visibility, acceptable because the policy is tighten-only.
static SETTINGS_RESOLVED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

const DEFAULT_SETTINGS_GATE_MAX_WAIT: Duration = Duration::from_secs(30);

static SETTINGS_GATE_MAX_WAIT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(DEFAULT_SETTINGS_GATE_MAX_WAIT.as_millis() as u64);

static GATE_CLOSED_AT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn process_uptime_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    u64::try_from(
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// Set the bound on the fail-closed window.
pub fn set_settings_gate_max_wait(max_wait: Duration) {
    SETTINGS_GATE_MAX_WAIT_MS.store(
        u64::try_from(max_wait.as_millis()).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The current bound on the fail-closed window.
pub fn settings_gate_max_wait() -> Duration {
    Duration::from_millis(SETTINGS_GATE_MAX_WAIT_MS.load(std::sync::atomic::Ordering::Relaxed))
}

/// Close the gate (leader preinit + account switch).
pub fn suppress_external_otel_until_settings() {
    GATE_CLOSED_AT_MS.store(process_uptime_ms(), std::sync::atomic::Ordering::Relaxed);
    // `Release`: a reader that observes the close must also observe the
    // timestamp published just above, or it would measure this window from an
    // earlier close and open immediately.
    SETTINGS_RESOLVED.store(false, std::sync::atomic::Ordering::Release);
}

/// Open the gate. `Release` publishes the force-disable applied just before it.
pub fn mark_external_otel_settings_resolved() {
    if !SETTINGS_RESOLVED.swap(true, std::sync::atomic::Ordering::Release) {
        tracing::debug!("external otel: settings resolved, emission gate opened");
    }
}

/// Read the gate. `Acquire` pairs with the `Release` open (and with the
/// `Release` close that publishes the window start).
#[inline]
pub fn is_settings_gate_open() -> bool {
    SETTINGS_RESOLVED.load(std::sync::atomic::Ordering::Acquire) || settings_gate_window_expired()
}

#[cold]
fn settings_gate_window_expired() -> bool {
    let waited =
        process_uptime_ms().saturating_sub(GATE_CLOSED_AT_MS.load(std::sync::atomic::Ordering::Relaxed));
    if waited < SETTINGS_GATE_MAX_WAIT_MS.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        tracing::warn!(
            waited_ms = waited,
            "external otel: no fleet policy arrived within the bounded window; \
             emitting under local configuration (a policy that arrives later still applies)"
        );
    });
    true
}

// ── Lifecycle no-ops ────────────────────────────────────────────────────────

/// Initialize the external stream. Never activates: the exporters were
/// removed with the telemetry stack.
pub fn init(_cfg: Option<ExternalOtelConfig>) {
    // [LOCAL-DEV] no-op: external OTEL stream never activates.
}

/// The stream is never active.
pub fn is_active() -> bool {
    false
}

/// Map and emit one typed telemetry event. No-op.
pub fn emit<T: crate::events::TelemetryEvent>(_data: &T) {}

/// Store identity attributes. No-op (no consumer).
pub fn set_identity(_attrs: IdentityAttrs) {}

/// Apply a remote policy. No-op (nothing to tighten).
pub fn apply_remote_policy(_policy: ExternalOtelRemotePolicy) {}

/// Flush in-flight exports. No-op.
pub fn flush() {}

/// Shutdown the stream. No-op.
pub fn shutdown() {}

/// Export-health counters. Always `None` (no exporters).
pub fn export_health() -> Option<ExportHealthSnapshot> {
    None
}
