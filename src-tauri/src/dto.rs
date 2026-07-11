//! Plain-string/primitive data-transfer shapes exchanged with the frontend
//! over `invoke`, kept deliberately separate from `village-core`'s
//! validated newtypes (`Community`, `PassKey`, `SupernodeAddr`, ...).
//!
//! Validation always happens at the boundary where a DTO is turned into (or
//! read from) a real `village_core::profile::ServerProfile` -- never
//! duplicated into these structs themselves.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use village_core::profile::{AdvancedSettings, Cipher, Compression, ProfileError, ServerProfile};
use village_ipc::protocol::ConnectionStatus;

/// A closed set of error "kinds" the frontend can distinguish, serialized
/// to JS as `{ "kind": "...", "message"?: "..." }`.
///
/// Every `#[tauri::command]` in this crate returns `Result<T, CommandError>`
/// (rather than a bare `String`) specifically so the frontend can tell
/// "the Village service isn't installed yet -- offer one-time setup" apart
/// from "setup is done but something else went wrong," without resorting to
/// sniffing a prefix out of a free-form error string.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum CommandError {
    /// The named pipe doesn't exist -- `village-service` has never been
    /// installed (or isn't running). The frontend should offer to run
    /// `ensure_service_installed` rather than show a generic error.
    ServiceNotInstalled,
    /// The service was reachable but returned an error, or an I/O error
    /// occurred talking to it after the pipe was successfully opened.
    ServiceError(String),
    /// The request itself was invalid: a malformed invite code, a field
    /// that failed `village-core`'s validation, an unknown server id, etc.
    ValidationError(String),
    /// Reading or writing the local config file failed.
    ConfigError(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::ServiceNotInstalled => {
                write!(f, "the Village service is not installed or not running")
            }
            CommandError::ServiceError(msg) => write!(f, "{msg}"),
            CommandError::ValidationError(msg) => write!(f, "{msg}"),
            CommandError::ConfigError(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<crate::ipc_client::IpcClientError> for CommandError {
    fn from(err: crate::ipc_client::IpcClientError) -> Self {
        match err {
            crate::ipc_client::IpcClientError::NotInstalled => CommandError::ServiceNotInstalled,
            other => CommandError::ServiceError(other.to_string()),
        }
    }
}

impl From<ProfileError> for CommandError {
    fn from(err: ProfileError) -> Self {
        CommandError::ValidationError(err.to_string())
    }
}

impl From<village_core::config::ConfigError> for CommandError {
    fn from(err: village_core::config::ConfigError) -> Self {
        CommandError::ConfigError(err.to_string())
    }
}

/// String labels for `village_core::profile::Cipher`/`Compression`, used at
/// the DTO boundary instead of the numeric `-A<n>`/`-z<n>` codes those enums
/// carry internally (those codes are an `edge.exe` argv implementation
/// detail the frontend has no reason to know about).
fn cipher_to_str(cipher: Cipher) -> &'static str {
    match cipher {
        Cipher::None => "none",
        Cipher::Twofish => "twofish",
        Cipher::Aes => "aes",
        Cipher::ChaCha20 => "chacha20",
        Cipher::Speck => "speck",
    }
}

fn cipher_from_str(value: &str) -> Result<Cipher, CommandError> {
    match value {
        "none" => Ok(Cipher::None),
        "twofish" => Ok(Cipher::Twofish),
        "aes" => Ok(Cipher::Aes),
        "chacha20" => Ok(Cipher::ChaCha20),
        "speck" => Ok(Cipher::Speck),
        other => Err(CommandError::ValidationError(format!(
            "unknown cipher \"{other}\""
        ))),
    }
}

fn compression_to_str(compression: Compression) -> &'static str {
    match compression {
        Compression::Lzo1x => "lzo1x",
        Compression::Zstd => "zstd",
    }
}

fn compression_from_str(value: &str) -> Result<Compression, CommandError> {
    match value {
        "lzo1x" => Ok(Compression::Lzo1x),
        "zstd" => Ok(Compression::Zstd),
        other => Err(CommandError::ValidationError(format!(
            "unknown compression \"{other}\""
        ))),
    }
}

/// Plain-value mirror of `village_core::profile::AdvancedSettings`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdvancedSettingsView {
    pub mtu: Option<u16>,
    pub header_encryption: bool,
    pub cipher: Option<String>,
    pub compression: Option<String>,
}

impl From<&AdvancedSettings> for AdvancedSettingsView {
    fn from(advanced: &AdvancedSettings) -> Self {
        Self {
            mtu: advanced.mtu,
            header_encryption: advanced.header_encryption,
            cipher: advanced.cipher.map(cipher_to_str).map(str::to_string),
            compression: advanced
                .compression
                .map(compression_to_str)
                .map(str::to_string),
        }
    }
}

impl TryFrom<AdvancedSettingsView> for AdvancedSettings {
    type Error = CommandError;

    fn try_from(view: AdvancedSettingsView) -> Result<Self, Self::Error> {
        Ok(AdvancedSettings {
            mtu: view.mtu,
            header_encryption: view.header_encryption,
            cipher: view.cipher.as_deref().map(cipher_from_str).transpose()?,
            compression: view
                .compression
                .as_deref()
                .map(compression_from_str)
                .transpose()?,
        })
    }
}

/// A saved server profile as shown in the frontend's server list.
///
/// Deliberately omits the raw passphrase (`key`) -- nothing in the server
/// list UI needs to display it, and profile fields sourced from a trusted
/// invite code aren't meant to be casually hand-edited (see `ServerPatch`'s
/// doc comment) so there's no edit form that would need it echoed back
/// either.
#[derive(Debug, Clone, Serialize)]
pub struct ServerProfileView {
    pub id: String,
    pub nickname: String,
    pub community: String,
    pub supernode: String,
    pub advanced: AdvancedSettingsView,
}

impl From<&ServerProfile> for ServerProfileView {
    fn from(profile: &ServerProfile) -> Self {
        Self {
            id: profile.id.to_string(),
            nickname: profile.nickname.clone(),
            community: profile.community.as_str().to_string(),
            supernode: profile.supernode.to_string(),
            advanced: AdvancedSettingsView::from(&profile.advanced),
        }
    }
}

/// Raw, unvalidated fields typed directly into the Advanced/host screen.
/// Turned into a real `ServerProfile` (surfacing validation errors as
/// readable messages) by `commands::build_profile_from_raw`.
#[derive(Debug, Clone, Deserialize)]
pub struct RawProfileInput {
    pub nickname: String,
    pub community: String,
    pub key: String,
    pub supernode: String,
    pub mtu: Option<u16>,
    pub header_encryption: bool,
    pub cipher: Option<String>,
    pub compression: Option<String>,
}

/// What's editable on an already-saved server profile after import.
///
/// Only `nickname` and `advanced` are patchable here. `community`/`key`/
/// `supernode` are not: those come from a trusted invite code (or the
/// Advanced/host screen's own validated construction), and letting a user
/// hand-edit them in place would mean re-deriving a "is this still a valid
/// profile" check for every partial edit combination. The simplest
/// defensible choice is to require delete + re-add (via a fresh invite
/// code, or the Advanced screen) for those fields instead.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerPatch {
    pub nickname: Option<String>,
    pub advanced: Option<AdvancedSettingsView>,
}

/// Turns a `Uuid`-shaped string from the frontend into a real `Uuid`,
/// mapping a malformed one to a `CommandError::ValidationError` rather than
/// panicking or propagating a raw parse error.
pub fn parse_id(id: &str) -> Result<Uuid, CommandError> {
    Uuid::parse_str(id)
        .map_err(|_| CommandError::ValidationError(format!("\"{id}\" is not a valid server id")))
}

/// Frontend-friendly mirror of `village_ipc::protocol::ConnectionStatus`,
/// polled via `commands::get_status` -- see that command's doc comment for
/// the expected polling cadence.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state")]
pub enum ConnectionStatusView {
    Idle,
    Starting,
    Connected {
        overlay_ip: String,
        since_unix_secs: u64,
    },
    Error {
        message: String,
    },
}

impl From<ConnectionStatus> for ConnectionStatusView {
    fn from(status: ConnectionStatus) -> Self {
        match status {
            ConnectionStatus::Idle => ConnectionStatusView::Idle,
            ConnectionStatus::Starting => ConnectionStatusView::Starting,
            ConnectionStatus::Connected {
                overlay_ip,
                since_unix_secs,
            } => ConnectionStatusView::Connected {
                overlay_ip,
                since_unix_secs,
            },
            ConnectionStatus::Error { message } => ConnectionStatusView::Error { message },
        }
    }
}
