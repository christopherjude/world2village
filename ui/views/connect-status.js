// Connection status view: big, obvious connection state, and once
// `ConnectionStatusView` reports `Connected`, the overlay IP shown large,
// monospace, copyable, with a "Your community IP is" lead-in label
// (see CLAUDE.md's product decisions). Polls `get_status` every ~1.5s
// while this view is active (per the polling-cadence note on the Rust
// `get_status` command) and stops polling once the view is left.

import { getStatus, disconnect } from "../api.js";
import { showView, showServiceSetupBanner, setActiveServerId } from "../app.js";

const POLL_INTERVAL_MS = 1500;

let container = null;
let panelEl = null;
let labelEl = null;
let nicknameEl = null;
let ipBlockEl = null;
let ipValueEl = null;
let errorEl = null;
let disconnectButton = null;
let copyButton = null;

let pollTimer = null;
let currentNickname = "";

// Tolerance for isolated transient poll failures (e.g. a pipe-teardown
// race -- see village-service's pipe.rs) so a single one-off failure
// doesn't flicker the UI into an "Error" state: only render Error once
// this many consecutive polls have failed. Reset to 0 on any success.
const MAX_CONSECUTIVE_FAILURES = 2;
let consecutiveFailures = 0;

export function mount(root, params) {
  currentNickname = (params && params.nickname) || "";

  container = document.createElement("div");
  container.className = "view";
  container.innerHTML = `
    <h1>Connection status</h1>
    <div id="status-panel" class="status-panel state-idle">
      <div id="status-label" class="status-label">Connecting</div>
      <div id="status-nickname" class="status-server-name"></div>

      <div id="overlay-ip-block" class="overlay-ip-block" hidden>
        <div class="overlay-ip-label">Your community IP is</div>
        <div id="overlay-ip-value" class="overlay-ip" tabindex="0"></div>
        <div class="copy-row">
          <button id="copy-ip-button" type="button" class="secondary-button">Copy</button>
        </div>
      </div>

      <div id="status-error" class="error-message" hidden></div>

      <div class="status-actions">
        <button id="disconnect-button" type="button" class="danger-button">Disconnect</button>
      </div>
    </div>
  `;
  root.appendChild(container);

  panelEl = container.querySelector("#status-panel");
  labelEl = container.querySelector("#status-label");
  nicknameEl = container.querySelector("#status-nickname");
  ipBlockEl = container.querySelector("#overlay-ip-block");
  ipValueEl = container.querySelector("#overlay-ip-value");
  errorEl = container.querySelector("#status-error");
  disconnectButton = container.querySelector("#disconnect-button");
  copyButton = container.querySelector("#copy-ip-button");

  nicknameEl.textContent = currentNickname;
  disconnectButton.addEventListener("click", onDisconnectClick);
  copyButton.addEventListener("click", onCopyClick);

  startPolling();
}

export function unmount() {
  stopPolling();
  container = null;
  panelEl = null;
  labelEl = null;
  nicknameEl = null;
  ipBlockEl = null;
  ipValueEl = null;
  errorEl = null;
  disconnectButton = null;
  copyButton = null;
}

function startPolling() {
  stopPolling();
  poll();
  pollTimer = setInterval(poll, POLL_INTERVAL_MS);
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function poll() {
  try {
    const status = await getStatus();
    consecutiveFailures = 0;
    render(status);
  } catch (err) {
    if (err.isServiceNotInstalled) {
      // Definitive state, not a transient blip -- surface it immediately
      // and pause polling until setup succeeds (the banner's retry
      // callback resumes it).
      consecutiveFailures = 0;
      stopPolling();
      showServiceSetupBanner(() => {
        startPolling();
      });
      return;
    }

    consecutiveFailures += 1;
    if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
      render({ state: "Error", message: err.message });
    }
    // Else: tolerate this isolated failure -- leave the previously
    // rendered state on screen rather than flickering to Error.
  }
}

const STATE_LABELS = {
  Idle: "Idle",
  Starting: "Connecting",
  Connected: "Connected",
  Error: "Error",
};

const STATE_CLASSES = {
  Idle: "state-idle",
  Starting: "state-connecting",
  Connected: "state-connected",
  Error: "state-error",
};

function render(status) {
  if (!panelEl) return;

  const state = status.state || "Idle";

  panelEl.classList.remove("state-idle", "state-connecting", "state-connected", "state-error");
  panelEl.classList.add(STATE_CLASSES[state] || "state-idle");
  labelEl.textContent = STATE_LABELS[state] || state;

  const isConnected = state === "Connected";
  ipBlockEl.hidden = !isConnected;
  if (isConnected) {
    ipValueEl.textContent = status.overlay_ip;
  }

  const isError = state === "Error";
  errorEl.hidden = !isError;
  if (isError) {
    errorEl.textContent = status.message || "Something went wrong.";
  }
}

async function onCopyClick() {
  if (!ipValueEl || !ipValueEl.textContent) return;
  try {
    await navigator.clipboard.writeText(ipValueEl.textContent);
    const original = copyButton.textContent;
    copyButton.textContent = "Copied";
    setTimeout(() => {
      if (copyButton) copyButton.textContent = original;
    }, 1200);
  } catch (err) {
    console.error("Village: clipboard write failed", err);
  }
}

async function onDisconnectClick() {
  disconnectButton.disabled = true;
  try {
    await disconnect();
    setActiveServerId(null);
    showView("server-list");
  } catch (err) {
    if (err.isServiceNotInstalled) {
      showServiceSetupBanner(onDisconnectClick);
    } else {
      render({ state: "Error", message: err.message || "Couldn't disconnect." });
    }
  } finally {
    if (disconnectButton) disconnectButton.disabled = false;
  }
}
