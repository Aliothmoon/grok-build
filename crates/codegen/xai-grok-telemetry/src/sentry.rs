//! Sentry error reporting — **stubbed**.
//!
//! [LOCAL-DEV] Telemetry removed: the `sentry` crate is no longer a
//! dependency. `init` returns a no-op guard regardless of `SENTRY_DSN`, and
//! the config surface is preserved so call sites compile unchanged.

use std::sync::OnceLock;

/// Per-host config; everything that varies between binaries lives here.
pub struct Config {
    /// Sentry tag `client`, e.g. `"grok-pager"`.
    pub client: &'static str,
    pub client_version: &'static str,
    pub release: &'static str,
    /// When `true`, [`init`] returns a no-op guard regardless of `SENTRY_DSN`.
    pub disabled: bool,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

// ─── Public API ────────────────────────────────────────────────────────────

/// No-op guard standing in for the sentry `ClientInitGuard`.
pub struct ClientInitGuard;

/// Init Sentry + apply the process-wide scope tags. No-op guard; nothing is
/// ever sent.
pub fn init(config: Config) -> ClientInitGuard {
    let _ = CONFIG.get_or_init(|| config);
    ClientInitGuard
}

/// Flush in-flight events. No-op.
pub fn flush_on_shutdown() {}
