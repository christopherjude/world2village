//! `#[tauri::command]` entry points invoked from the frontend.
//!
//! Every command returns `Result<T, CommandError>` (see `dto.rs`) so the
//! frontend can distinguish "the Village service isn't installed yet" from
//! other failure kinds without string-sniffing. Validation of any
//! user-supplied data always happens here (or in `dto.rs`'s `TryFrom`
//! impls) via `village-core`'s constructors -- these commands never trust a
//! frontend-supplied value is already well-formed.

use tauri::Manager;
use uuid::Uuid;

use village_core::invite;
use village_core::mac::MacAddr;
use village_core::profile::{AdvancedSettings, Community, PassKey, ServerProfile, SupernodeAddr};
use village_ipc::protocol::{Request, ResolvedProfile, Response};

use crate::dto::{
    parse_id, AdvancedSettingsView, CommandError, ConnectionStatusView, RawProfileInput,
    ServerPatch, ServerProfileView,
};
use crate::elevate;
use crate::ipc_client;
use crate::state::{lock_config, AppState};

#[tauri::command]
pub fn list_servers(state: tauri::State<'_, AppState>) -> Result<Vec<ServerProfileView>, CommandError> {
    let config = lock_config(&state);
    Ok(config.servers.iter().map(ServerProfileView::from).collect())
}

#[tauri::command]
pub fn add_server_from_code(
    state: tauri::State<'_, AppState>,
    code: String,
) -> Result<ServerProfileView, CommandError> {
    let profile =
        invite::decode_invite(&code).map_err(|err| CommandError::ValidationError(err.to_string()))?;
    let view = ServerProfileView::from(&profile);

    let mut config = lock_config(&state);
    config.servers.push(profile);
    state.config_store.save(&config)?;

    Ok(view)
}

#[tauri::command]
pub fn update_server(
    state: tauri::State<'_, AppState>,
    id: String,
    patch: ServerPatch,
) -> Result<(), CommandError> {
    let uuid = parse_id(&id)?;

    let mut config = lock_config(&state);
    let profile = config
        .servers
        .iter_mut()
        .find(|p| p.id == uuid)
        .ok_or_else(|| CommandError::ValidationError("server not found".to_string()))?;

    if let Some(nickname) = patch.nickname {
        ServerProfile::validate_nickname(&nickname)?;
        profile.nickname = nickname;
    }
    if let Some(advanced_view) = patch.advanced {
        profile.advanced = AdvancedSettings::try_from(advanced_view)?;
    }

    state.config_store.save(&config)?;
    Ok(())
}

#[tauri::command]
pub fn delete_server(state: tauri::State<'_, AppState>, id: String) -> Result<(), CommandError> {
    let uuid = parse_id(&id)?;

    let mut config = lock_config(&state);
    let len_before = config.servers.len();
    config.servers.retain(|p| p.id != uuid);
    if config.servers.len() == len_before {
        return Err(CommandError::ValidationError("server not found".to_string()));
    }

    state.config_store.save(&config)?;
    Ok(())
}

#[tauri::command]
pub fn export_invite_code(state: tauri::State<'_, AppState>, id: String) -> Result<String, CommandError> {
    let uuid = parse_id(&id)?;

    let config = lock_config(&state);
    let profile = config
        .servers
        .iter()
        .find(|p| p.id == uuid)
        .ok_or_else(|| CommandError::ValidationError("server not found".to_string()))?;

    Ok(invite::encode_invite(profile))
}

/// Pure function (no state mutation) backing the Advanced/host screen's
/// "generate a code" action.
#[tauri::command]
pub fn generate_invite_code_from_fields(raw: RawProfileInput) -> Result<String, CommandError> {
    let profile = build_profile_from_raw(raw)?;
    Ok(invite::encode_invite(&profile))
}

/// Same field construction/validation as `generate_invite_code_from_fields`,
/// but also appends the result to the user's own server list and persists
/// it -- backs the Advanced/host screen's "also save to my list" action.
#[tauri::command]
pub fn save_raw_as_server(
    state: tauri::State<'_, AppState>,
    raw: RawProfileInput,
) -> Result<ServerProfileView, CommandError> {
    let profile = build_profile_from_raw(raw)?;
    let view = ServerProfileView::from(&profile);

    let mut config = lock_config(&state);
    config.servers.push(profile);
    state.config_store.save(&config)?;

    Ok(view)
}

/// Starts (or restarts) the given saved profile's `edge.exe` session by
/// asking `village-service` to do so over the named pipe.
///
/// On `CommandError::ServiceNotInstalled` (the pipe doesn't exist), the
/// frontend should offer to call `ensure_service_installed` rather than
/// show a generic connection failure.
#[tauri::command]
pub fn connect(state: tauri::State<'_, AppState>, id: String) -> Result<(), CommandError> {
    let uuid = parse_id(&id)?;

    let resolved = {
        let config = lock_config(&state);
        let profile = config
            .servers
            .iter()
            .find(|p| p.id == uuid)
            .ok_or_else(|| CommandError::ValidationError("server not found".to_string()))?;
        resolved_profile_for(profile, config.identity.mac)
        // Lock is dropped here, before the blocking IPC call below -- a
        // slow/blocked pipe round trip should never hold up other commands
        // that only need the config lock (e.g. `list_servers`).
    };

    match ipc_client::send_request(&Request::StartProfile { profile: resolved })? {
        Response::Ok => Ok(()),
        Response::Error { code, message } => {
            Err(CommandError::ServiceError(format!("{code:?}: {message}")))
        }
        Response::Status(_) => Err(CommandError::ServiceError(
            "unexpected response from the Village service".to_string(),
        )),
    }
}

#[tauri::command]
pub fn disconnect(_state: tauri::State<'_, AppState>) -> Result<(), CommandError> {
    match ipc_client::send_request(&Request::Stop)? {
        Response::Ok => Ok(()),
        Response::Error { code, message } => {
            Err(CommandError::ServiceError(format!("{code:?}: {message}")))
        }
        Response::Status(_) => Err(CommandError::ServiceError(
            "unexpected response from the Village service".to_string(),
        )),
    }
}

/// Reports the current connection state as last known to `village-service`.
///
/// This is meant to be polled by the frontend on an interval -- roughly
/// every 1-2 seconds while a connect attempt is in flight (`Starting`) or a
/// session is active (`Connected`), so the UI's big connection-state
/// indicator and overlay IP stay current. The polling loop itself is
/// frontend work, not implemented here.
#[tauri::command]
pub fn get_status(_state: tauri::State<'_, AppState>) -> Result<ConnectionStatusView, CommandError> {
    match ipc_client::send_request(&Request::Status)? {
        Response::Status(status) => Ok(ConnectionStatusView::from(status)),
        Response::Error { code, message } => {
            Err(CommandError::ServiceError(format!("{code:?}: {message}")))
        }
        Response::Ok => Err(CommandError::ServiceError(
            "unexpected response from the Village service".to_string(),
        )),
    }
}

/// One-time setup: launches `village-service.exe install <resource-dir>`
/// elevated (triggering a single UAC prompt) so the Windows Service gets
/// registered, `edge.exe`/the tap-windows6 driver files get copied into
/// their locked-down `%ProgramData%\Village\bin` home, and the tap-windows6
/// adapter gets installed. See `elevate.rs` for the actual `ShellExecuteW`
/// call.
///
/// This only *launches* the installer; it does not wait for it to finish
/// (the elevated process runs independently, outside this app's process
/// tree). The frontend should poll `get_status`/retry the action that
/// prompted setup after a short delay rather than assume completion
/// immediately upon this command returning.
#[tauri::command]
pub fn ensure_service_installed(app: tauri::AppHandle) -> Result<(), CommandError> {
    let resource_dir = app.path().resource_dir().map_err(|err| {
        CommandError::ServiceError(format!("failed to resolve resource dir: {err}"))
    })?;
    let service_exe = resource_dir.join("village-service.exe");

    elevate::launch_service_installer(&service_exe, &resource_dir).map_err(CommandError::ServiceError)
}

/// Builds a validated `ServerProfile` from the Advanced/host screen's raw
/// string fields, surfacing any `village-core` validation error as a
/// readable `CommandError::ValidationError`.
fn build_profile_from_raw(raw: RawProfileInput) -> Result<ServerProfile, CommandError> {
    ServerProfile::validate_nickname(&raw.nickname)?;
    let community = Community::new(raw.community)?;
    let key = PassKey::new(raw.key)?;
    let supernode = SupernodeAddr::new(&raw.supernode)?;
    let advanced = AdvancedSettings::try_from(AdvancedSettingsView {
        mtu: raw.mtu,
        header_encryption: raw.header_encryption,
        cipher: raw.cipher,
        compression: raw.compression,
    })?;

    Ok(ServerProfile {
        id: Uuid::new_v4(),
        nickname: raw.nickname,
        community,
        key,
        supernode,
        advanced,
    })
}

/// Builds the wire-format `ResolvedProfile` for `Request::StartProfile`
/// from a saved profile plus the per-install MAC persisted in config.
fn resolved_profile_for(profile: &ServerProfile, mac: MacAddr) -> ResolvedProfile {
    ResolvedProfile {
        nickname: profile.nickname.clone(),
        community: profile.community.as_str().to_string(),
        key: profile.key.as_str().to_string(),
        supernode: profile.supernode.to_string(),
        mac: mac.to_string(),
        mtu: profile.advanced.mtu,
        header_encryption: profile.advanced.header_encryption,
        cipher: profile.advanced.cipher.map(|c| c.code()),
        compression: profile.advanced.compression.map(|c| c.code()),
    }
}
