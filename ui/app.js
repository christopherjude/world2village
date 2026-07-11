// Tiny hand-rolled view router. No history/URL routing -- Village is a
// single-window utility app, so `showView(name)` swapping the contents of
// `#view-root` is enough.
//
// Also owns two small pieces of state that legitimately need to live above
// any single view:
//   - the shared "Set up Village" banner (any view can trigger it when a
//     service-talking command comes back `ServiceNotInstalled`)
//   - which saved server id was last asked to connect -- `get_status()`
//     reports connection state but not *which profile* it belongs to (see
//     `village_ipc::protocol::ConnectionStatus`), so the server list needs
//     some client-side memory of "which row did I press Connect on" to
//     decide which row's button should read "Disconnect".

import { ensureServiceInstalled } from "./api.js";
import * as serverList from "./views/server-list.js";
import * as connectStatus from "./views/connect-status.js";
import * as advancedHost from "./views/advanced-host.js";

const viewRoot = document.getElementById("view-root");
const navBack = document.getElementById("nav-back");
const advancedLink = document.getElementById("advanced-link");

const setupBanner = document.getElementById("setup-banner");
const setupBannerRun = document.getElementById("setup-banner-run");
const setupBannerDismiss = document.getElementById("setup-banner-dismiss");
const setupBannerStatus = document.getElementById("setup-banner-status");

const views = {
  "server-list": serverList,
  "connect-status": connectStatus,
  "advanced-host": advancedHost,
};

let currentView = null;
let pendingRetry = null;

export function showView(name, params) {
  const view = views[name];
  if (!view) {
    console.error(`Village: unknown view "${name}"`);
    return;
  }
  if (currentView && typeof currentView.unmount === "function") {
    currentView.unmount();
  }
  viewRoot.innerHTML = "";
  currentView = view;
  navBack.hidden = name === "server-list";
  view.mount(viewRoot, params || {});
}

navBack.addEventListener("click", () => showView("server-list"));
advancedLink.addEventListener("click", (event) => {
  event.preventDefault();
  showView("advanced-host");
});

/**
 * Shows the "Set up Village" banner. `retry` (optional) is an async
 * function re-run automatically once setup appears to have succeeded, so
 * the action that originally triggered the banner (e.g. "Connect") doesn't
 * have to be manually re-triggered by the user.
 */
export function showServiceSetupBanner(retry) {
  pendingRetry = retry || null;
  setupBannerStatus.hidden = true;
  setupBannerStatus.textContent = "";
  setupBannerStatus.classList.remove("banner-error");
  setupBanner.hidden = false;

  // Always bring the banner into view and give it a brief attention pulse,
  // even if it was already visible from an earlier attempt -- otherwise a
  // second failed connect while the banner is still up looks like nothing
  // happened.
  setupBanner.scrollIntoView({ behavior: "smooth", block: "center" });
  setupBanner.classList.add("attention-pulse");
  setTimeout(() => setupBanner.classList.remove("attention-pulse"), 600);
}

export function hideServiceSetupBanner() {
  setupBanner.hidden = true;
  pendingRetry = null;
}

setupBannerRun.addEventListener("click", async () => {
  setupBannerRun.disabled = true;
  setupBannerStatus.hidden = false;
  setupBannerStatus.classList.remove("banner-error");
  setupBannerStatus.textContent =
    "Requesting setup -- approve the Windows security prompt if one appears.";
  try {
    await ensureServiceInstalled();
    setupBannerStatus.textContent = "Setup requested. Retrying...";
    const retry = pendingRetry;
    // `ensure_service_installed` only *launches* the elevated installer and
    // returns immediately (see its doc comment in commands.rs) -- it does
    // not wait for the install to finish. Give it a moment before retrying
    // rather than assuming completion the instant this call returns.
    await new Promise((resolve) => setTimeout(resolve, 2000));
    if (retry) {
      await retry();
    }
    hideServiceSetupBanner();
  } catch (err) {
    setupBannerStatus.classList.add("banner-error");
    setupBannerStatus.textContent = `Setup didn't finish: ${err.message || err}`;
  } finally {
    setupBannerRun.disabled = false;
  }
});

setupBannerDismiss.addEventListener("click", () => hideServiceSetupBanner());

const ACTIVE_SERVER_KEY = "village.activeServerId";

/** The id of the server the user last pressed Connect on, if any. */
export function getActiveServerId() {
  return localStorage.getItem(ACTIVE_SERVER_KEY);
}

export function setActiveServerId(id) {
  if (id) {
    localStorage.setItem(ACTIVE_SERVER_KEY, id);
  } else {
    localStorage.removeItem(ACTIVE_SERVER_KEY);
  }
}

// Initial load: render the server list (home view).
//
// This is deferred to `DOMContentLoaded` rather than called synchronously
// here. `app.js` and the view modules import each other (view modules call
// `showView`/`showServiceSetupBanner`/etc. from event handlers; this file
// imports the view modules to route to them) -- a circular module graph.
// Per the module-evaluation order, whichever view module's `<script>` tag
// happens to come first in `index.html` ends up evaluating `app.js` as one
// of *its own* dependencies before that view module's own top-level `let`
// bindings run. Calling `showView` synchronously here can therefore run
// inside that nested evaluation, before the target view module has
// finished initializing, throwing a temporal-dead-zone
// "Cannot access '...' before initialization" error. `DOMContentLoaded`
// only fires after every module script in the document has finished
// evaluating, so waiting for it sidesteps the ordering hazard entirely.
document.addEventListener("DOMContentLoaded", () => showView("server-list"), { once: true });
