// Single seam wrapping Tauri's `invoke`. Every exported function here maps
// 1:1 to a `#[tauri::command]` in `src-tauri/src/commands.rs` -- see that
// file and `src-tauri/src/dto.rs` for the exact argument/return shapes this
// module assumes.
//
// Tauri v2 exposes the JS API at `window.__TAURI__.core.invoke` (not v1's
// `window.__TAURI__.invoke`). For the browser-only static sanity check
// (`python3 -m http.server` + a stub), `window.__TAURI__` is provided by a
// hand-written stub script rather than the real Tauri runtime.

function invoke(command, args) {
  const tauri = window.__TAURI__;
  if (!tauri || !tauri.core || typeof tauri.core.invoke !== "function") {
    throw new Error(
      "window.__TAURI__.core.invoke is not available -- this page must run inside the Village app (or a stubbed sanity-check page)."
    );
  }
  return tauri.core.invoke(command, args);
}

/**
 * Normalized app-level error shape all `api.js` functions reject with.
 *
 * `kind` mirrors the Rust `CommandError` enum's `kind` tag
 * (`ServiceNotInstalled` | `ServiceError` | `ValidationError` |
 * `ConfigError`), or `"Unknown"` if the rejection didn't look like a
 * `CommandError` at all (e.g. `invoke` itself threw for an unrelated
 * reason). `message` is always a human-readable string safe to display.
 */
class VillageError extends Error {
  constructor(kind, message) {
    super(message);
    this.kind = kind;
    this.isServiceNotInstalled = kind === "ServiceNotInstalled";
  }
}

/**
 * Wire shape reference (see `dto.rs`'s `CommandError`):
 *   #[serde(tag = "kind", content = "message")]
 *   ServiceNotInstalled            -> { "kind": "ServiceNotInstalled" }
 *   ServiceError(String)           -> { "kind": "ServiceError", "message": "..." }
 *   ValidationError(String)        -> { "kind": "ValidationError", "message": "..." }
 *   ConfigError(String)            -> { "kind": "ConfigError", "message": "..." }
 */
function normalizeError(err) {
  if (err && typeof err === "object" && typeof err.kind === "string") {
    const fallbackMessages = {
      ServiceNotInstalled: "The Village background service is not installed or not running.",
    };
    const message = err.message || fallbackMessages[err.kind] || "Something went wrong.";
    return new VillageError(err.kind, message);
  }
  if (typeof err === "string") {
    return new VillageError("Unknown", err);
  }
  return new VillageError("Unknown", (err && err.message) || "Something went wrong.");
}

async function call(command, args) {
  try {
    return await invoke(command, args);
  } catch (err) {
    console.error("Village command failed:", command, err);
    throw normalizeError(err);
  }
}

export async function listServers() {
  return call("list_servers");
}

export async function addServerFromCode(code) {
  return call("add_server_from_code", { code });
}

export async function updateServer(id, patch) {
  return call("update_server", { id, patch });
}

export async function deleteServer(id) {
  return call("delete_server", { id });
}

export async function exportInviteCode(id) {
  return call("export_invite_code", { id });
}

export async function generateInviteCode(raw) {
  return call("generate_invite_code_from_fields", { raw });
}

export async function saveRawAsServer(raw) {
  return call("save_raw_as_server", { raw });
}

export async function connect(id) {
  return call("connect", { id });
}

export async function disconnect() {
  return call("disconnect");
}

export async function getStatus() {
  return call("get_status");
}

export async function ensureServiceInstalled() {
  return call("ensure_service_installed");
}

export { VillageError };
