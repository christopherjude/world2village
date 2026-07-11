// Home view: the saved-server list, per CLAUDE.md's product decisions --
// nickname visible, community/key never shown prominently (or at all: the
// `key` isn't even present on `ServerProfileView`). Each row has a
// Connect/Disconnect button reflecting whether it's the currently-active
// server, a small delete affordance, and a status dot.

import {
  listServers,
  addServerFromCode,
  deleteServer,
  connect,
  disconnect,
  getStatus,
} from "../api.js";
import {
  showView,
  showServiceSetupBanner,
  getActiveServerId,
  setActiveServerId,
} from "../app.js";

let container = null;
let listEl = null;
let errorBannerEl = null;
let addDialog = null;
let addTextarea = null;
let addErrorEl = null;

export function mount(root) {
  container = document.createElement("div");
  container.className = "view";
  container.innerHTML = `
    <h1>Your servers</h1>
    <div id="server-list-error" class="error-banner" hidden></div>
    <ul id="server-list" class="server-list"></ul>
    <div>
      <button id="add-server-button" type="button" class="primary-button">+ Add server</button>
    </div>

    <dialog id="add-server-dialog">
      <p class="dialog-title">Add a server</p>
      <p>Paste the invite code someone shared with you.</p>
      <div class="field">
        <label for="add-server-code">Invite code</label>
        <textarea id="add-server-code" placeholder="VLG1-..."></textarea>
      </div>
      <div id="add-server-error" class="error-banner" hidden></div>
      <div class="dialog-actions">
        <button id="add-server-cancel" type="button" class="secondary-button">Cancel</button>
        <button id="add-server-submit" type="button" class="primary-button">Add</button>
      </div>
    </dialog>
  `;
  root.appendChild(container);

  listEl = container.querySelector("#server-list");
  errorBannerEl = container.querySelector("#server-list-error");
  addDialog = container.querySelector("#add-server-dialog");
  addTextarea = container.querySelector("#add-server-code");
  addErrorEl = container.querySelector("#add-server-error");

  container.querySelector("#add-server-button").addEventListener("click", () => {
    addTextarea.value = "";
    addErrorEl.hidden = true;
    addDialog.showModal();
  });
  container.querySelector("#add-server-cancel").addEventListener("click", () => {
    addDialog.close();
  });
  container.querySelector("#add-server-submit").addEventListener("click", onAddSubmit);

  refresh();
}

export function unmount() {
  container = null;
  listEl = null;
  errorBannerEl = null;
  addDialog = null;
  addTextarea = null;
  addErrorEl = null;
}

function showListError(message) {
  if (!errorBannerEl) return;
  errorBannerEl.textContent = message;
  errorBannerEl.hidden = false;
}

function clearListError() {
  if (!errorBannerEl) return;
  errorBannerEl.hidden = true;
}

async function refresh() {
  clearListError();
  let servers;
  try {
    servers = await listServers();
  } catch (err) {
    showListError(err.message || "Couldn't load your servers.");
    return;
  }

  let status = null;
  try {
    status = await getStatus();
  } catch (err) {
    if (err.isServiceNotInstalled) {
      showServiceSetupBanner(refresh);
    }
    // Any other status error just means we can't tell which row (if any)
    // is active right now -- fall back to rendering everything idle rather
    // than blocking the list itself.
  }

  renderList(servers, status);
}

function renderList(servers, status) {
  if (!listEl) return;
  listEl.innerHTML = "";

  if (servers.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty-state";
    empty.textContent = "No servers yet. Add one with an invite code to get started.";
    listEl.appendChild(empty);
    return;
  }

  const activeId = getActiveServerId();
  const liveState = status ? status.state : null;
  const isSessionLive = liveState === "Connected" || liveState === "Starting";

  for (const server of servers) {
    const isActiveServer = isSessionLive && server.id === activeId;

    const row = document.createElement("li");
    row.className = "server-row";

    const dot = document.createElement("span");
    dot.className = "status-dot";
    if (isActiveServer) {
      dot.classList.add(liveState === "Connected" ? "is-connected" : "is-connecting");
    }

    const main = document.createElement("div");
    main.className = "server-row-main";
    const name = document.createElement("span");
    name.className = "server-nickname";
    name.textContent = server.nickname;
    main.appendChild(dot);
    main.appendChild(name);

    const actions = document.createElement("div");
    actions.className = "server-row-actions";

    const connectButton = document.createElement("button");
    connectButton.type = "button";
    if (isActiveServer) {
      connectButton.textContent = "Disconnect";
      connectButton.className = "secondary-button";
      connectButton.addEventListener("click", () => onDisconnectClick(connectButton));
    } else {
      connectButton.textContent = "Connect";
      connectButton.className = "primary-button";
      connectButton.addEventListener("click", () => onConnectClick(server, connectButton));
    }

    const deleteButton = document.createElement("button");
    deleteButton.type = "button";
    deleteButton.className = "icon-button";
    deleteButton.setAttribute("aria-label", `Remove ${server.nickname}`);
    deleteButton.textContent = "×";
    deleteButton.addEventListener("click", () => onDeleteClick(server));

    actions.appendChild(connectButton);
    actions.appendChild(deleteButton);

    row.appendChild(main);
    row.appendChild(actions);
    listEl.appendChild(row);
  }
}

async function onConnectClick(server, button) {
  button.disabled = true;
  button.textContent = "Connecting…";
  clearListError();
  try {
    await connect(server.id);
    setActiveServerId(server.id);
    showView("connect-status", { id: server.id, nickname: server.nickname });
  } catch (err) {
    if (err.isServiceNotInstalled) {
      showServiceSetupBanner(() => onConnectClick(server, button));
    } else {
      showListError(friendlyError(err, "Couldn't connect."));
    }
  } finally {
    if (button) {
      button.disabled = false;
      button.textContent = "Connect";
    }
  }
}

async function onDisconnectClick(button) {
  button.disabled = true;
  clearListError();
  try {
    await disconnect();
    setActiveServerId(null);
    await refresh();
  } catch (err) {
    if (err.isServiceNotInstalled) {
      showServiceSetupBanner(() => onDisconnectClick(button));
    } else {
      showListError(friendlyError(err, "Couldn't disconnect."));
    }
  } finally {
    if (button) button.disabled = false;
  }
}

async function onDeleteClick(server) {
  const confirmed = window.confirm(`Remove "${server.nickname}" from your server list?`);
  if (!confirmed) return;

  clearListError();
  try {
    await deleteServer(server.id);
    if (getActiveServerId() === server.id) {
      setActiveServerId(null);
    }
    await refresh();
  } catch (err) {
    showListError(friendlyError(err, "Couldn't remove that server."));
  }
}

async function onAddSubmit() {
  const code = addTextarea.value.trim();
  addErrorEl.hidden = true;

  if (!code) {
    addErrorEl.textContent = "Paste an invite code first.";
    addErrorEl.hidden = false;
    return;
  }

  try {
    await addServerFromCode(code);
    addDialog.close();
    await refresh();
  } catch (err) {
    addErrorEl.textContent = friendlyError(err, "That doesn't look like a valid invite code.");
    addErrorEl.hidden = false;
  }
}

/**
 * `village-core`'s validation errors (see `ProfileError`/`invite::decode_invite`)
 * are already written to be human-readable, so they're shown as-is. Only
 * genuinely opaque/unexpected error kinds fall back to `fallback`.
 */
function friendlyError(err, fallback) {
  if (err && err.kind === "ValidationError" && err.message) {
    return err.message;
  }
  if (err && err.message) {
    return err.message;
  }
  return fallback;
}
