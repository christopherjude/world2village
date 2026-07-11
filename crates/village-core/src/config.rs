//! On-disk configuration: the per-install identity (MAC) and the saved
//! server profile list. Owned by the unelevated GUI process, never the
//! service.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mac::MacAddr;
use crate::profile::ServerProfile;

const CURRENT_CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub mac: MacAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub identity: Identity,
    pub servers: Vec<ServerProfile>,
}

impl Config {
    /// A fresh default config: a freshly generated per-install MAC and an
    /// empty server list.
    pub fn new_default() -> Self {
        Self {
            version: CURRENT_CONFIG_VERSION,
            identity: Identity {
                mac: MacAddr::generate_random(),
            },
            servers: Vec::new(),
        }
    }
}

/// Errors returned by [`ConfigStore::load`]/[`ConfigStore::save`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] io::Error),
    #[error("failed to parse config file: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Reads/writes the single JSON config file.
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The default config file location: `<config dir>/config.json` under
    /// the OS-appropriate per-app config directory (e.g.
    /// `%APPDATA%\Village\config.json` on Windows).
    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("com", "village", "Village")
            .map(|dirs| dirs.config_dir().join("config.json"))
            .unwrap_or_else(|| PathBuf::from("config.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the config from disk. If the file doesn't exist yet, returns a
    /// fresh default config (with a newly generated MAC) rather than an
    /// error — this is the expected state on first run.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Config::new_default());
            }
            Err(err) => return Err(ConfigError::Io(err)),
        };
        let config: Config = serde_json::from_slice(&bytes)?;
        Ok(config)
    }

    /// Saves the config to disk atomically: writes to a `.tmp` sibling file
    /// then renames it over the real path, so a crash mid-write can never
    /// leave a partially-written config file at `self.path`.
    pub fn save(&self, cfg: &Config) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(cfg)?;
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{AdvancedSettings, Community, PassKey, SupernodeAddr};
    use uuid::Uuid;

    fn store_in(dir: &tempfile::TempDir) -> ConfigStore {
        ConfigStore::new(dir.path().join("config.json"))
    }

    #[test]
    fn load_when_absent_creates_sensible_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);

        let cfg = store.load().unwrap();
        assert_eq!(cfg.version, CURRENT_CONFIG_VERSION);
        assert!(cfg.servers.is_empty());
        // Just sanity-check the MAC has the locally-administered bit set,
        // proving a real MAC was generated (not all-zero/default).
        assert_eq!(cfg.identity.mac.as_bytes()[0] & 0x02, 0x02);
    }

    #[test]
    fn save_then_load_round_trips_config_with_servers() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);

        let mut cfg = Config::new_default();
        cfg.servers.push(ServerProfile {
            id: Uuid::new_v4(),
            nickname: "Test Server".to_string(),
            community: Community::new("generals").unwrap(),
            key: PassKey::new("supersecret").unwrap(),
            supernode: SupernodeAddr::new("sn.example.com:7654").unwrap(),
            advanced: AdvancedSettings::default(),
        });

        store.save(&cfg).unwrap();
        assert!(store.path().exists());

        let loaded = store.load().unwrap();
        assert_eq!(loaded.version, cfg.version);
        assert_eq!(loaded.identity.mac, cfg.identity.mac);
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].nickname, "Test Server");
        assert_eq!(loaded.servers[0].community, cfg.servers[0].community);
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        store.save(&Config::new_default()).unwrap();

        let tmp_path = store.path().with_extension("json.tmp");
        assert!(!tmp_path.exists());
        assert!(store.path().exists());
    }

    #[test]
    fn default_path_ends_with_config_json() {
        let path = ConfigStore::default_path();
        assert_eq!(path.file_name().unwrap(), "config.json");
    }
}
