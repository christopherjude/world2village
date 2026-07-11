//! Application state managed by Tauri (`.manage(...)`): the loaded config
//! plus the store used to persist it back to disk.
//!
//! Deliberately does **not** hold a named-pipe connection to
//! `village-service`. The service may not be installed yet, may be mid
//! restart, or may simply not have a client connected at any given moment
//! -- rather than model a long-lived, possibly-stale connection as a field
//! here, `ipc_client::send_request` opens a fresh pipe handle per request.
//! IPC control-message volume is low (start/stop/status polling), so the
//! per-request connect overhead is not a meaningful cost.

use std::sync::{Mutex, MutexGuard};

use village_core::config::{Config, ConfigError, ConfigStore};

pub struct AppState {
    pub config: Mutex<Config>,
    pub config_store: ConfigStore,
}

impl AppState {
    /// Loads the config from its default OS-appropriate location, creating
    /// an in-memory default (with a freshly generated per-install MAC) if
    /// no config file exists yet -- see `ConfigStore::load`'s documented
    /// first-run behavior. Does not write anything to disk by itself; the
    /// first command that mutates the server list (e.g. `add_server_from_code`)
    /// is what actually persists it for the first time.
    pub fn load_or_default() -> Result<Self, ConfigError> {
        let config_store = ConfigStore::new(ConfigStore::default_path());
        let config = config_store.load()?;
        Ok(Self {
            config: Mutex::new(config),
            config_store,
        })
    }
}

/// Locks `state.config`, recovering from mutex poisoning (a prior panic
/// while a command held the lock) rather than propagating it -- one bad
/// request should not permanently wedge every future command that needs to
/// touch the config. Mirrors `village-service`'s `dispatch::lock_state`.
pub fn lock_config(state: &AppState) -> MutexGuard<'_, Config> {
    state.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
