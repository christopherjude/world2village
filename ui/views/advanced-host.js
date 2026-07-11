// Advanced/host screen: reachable only via the small out-of-the-way footer
// link (see index.html / app.js), never part of primary navigation -- this
// is a host-only power-user screen for typing raw fields and generating an
// invite code, per CLAUDE.md's product decisions (community/key/supernode
// are never surfaced as separate fields anywhere else in the app).

import { generateInviteCode, saveRawAsServer } from "../api.js";
import { showServiceSetupBanner } from "../app.js";

let container = null;
let errorBannerEl = null;
let resultBlockEl = null;
let resultCodeEl = null;
let saveConfirmationEl = null;

let nicknameInput, communityInput, keyInput, supernodeInput;
let mtuInput, headerEncryptionInput, cipherSelect, compressionSelect;
let generateButton, saveButton, copyResultButton;

export function mount(root) {
  container = document.createElement("div");
  container.className = "view advanced-view";
  container.innerHTML = `
    <h1>Advanced: create an invite code</h1>
    <p class="advanced-note">
      This is a host-only screen for entering raw connection details and
      turning them into a single invite code you can share. Most people
      never need this -- if someone sent you a code, use "+ Add server" on
      the home screen instead.
    </p>

    <div id="advanced-error" class="error-banner" hidden></div>

    <form id="advanced-form">
      <div class="field">
        <label for="adv-nickname">Nickname</label>
        <input id="adv-nickname" type="text" placeholder="e.g. Dubai server" />
      </div>
      <div class="field">
        <label for="adv-community">Community</label>
        <input id="adv-community" type="text" placeholder="e.g. villagelan" />
      </div>
      <div class="field">
        <label for="adv-key">Key</label>
        <input id="adv-key" type="text" placeholder="shared passphrase" />
      </div>
      <div class="field">
        <label for="adv-supernode">Supernode (host:port)</label>
        <input id="adv-supernode" type="text" placeholder="e.g. supernode.example.com:7654" />
      </div>

      <details class="advanced-fields">
        <summary>More options (MTU, encryption, compression)</summary>
        <div class="field">
          <label for="adv-mtu">MTU</label>
          <input id="adv-mtu" type="number" min="0" max="65535" placeholder="leave blank for default" />
        </div>
        <div class="field checkbox-field">
          <input id="adv-header-encryption" type="checkbox" />
          <label for="adv-header-encryption">Header encryption</label>
        </div>
        <div class="field">
          <label for="adv-cipher">Cipher</label>
          <select id="adv-cipher">
            <option value="">Default</option>
            <option value="none">None</option>
            <option value="twofish">Twofish</option>
            <option value="aes">AES</option>
            <option value="chacha20">ChaCha20</option>
            <option value="speck">Speck</option>
          </select>
        </div>
        <div class="field">
          <label for="adv-compression">Compression</label>
          <select id="adv-compression">
            <option value="">Default</option>
            <option value="lzo1x">LZO1X</option>
            <option value="zstd">Zstd</option>
          </select>
        </div>
      </details>

      <div class="form-actions">
        <button id="adv-generate" type="button" class="secondary-button">Generate invite code</button>
        <button id="adv-save" type="button" class="primary-button">Save as my server</button>
      </div>
    </form>

    <div id="advanced-result" class="result-block" hidden>
      <label for="adv-result-code">Invite code</label>
      <div class="field">
        <input id="adv-result-code" class="readonly-code" type="text" readonly />
      </div>
      <button id="adv-copy-result" type="button" class="secondary-button">Copy</button>
    </div>

    <p id="adv-save-confirmation" class="advanced-note" hidden></p>
  `;
  root.appendChild(container);

  errorBannerEl = container.querySelector("#advanced-error");
  resultBlockEl = container.querySelector("#advanced-result");
  resultCodeEl = container.querySelector("#adv-result-code");
  saveConfirmationEl = container.querySelector("#adv-save-confirmation");

  nicknameInput = container.querySelector("#adv-nickname");
  communityInput = container.querySelector("#adv-community");
  keyInput = container.querySelector("#adv-key");
  supernodeInput = container.querySelector("#adv-supernode");
  mtuInput = container.querySelector("#adv-mtu");
  headerEncryptionInput = container.querySelector("#adv-header-encryption");
  cipherSelect = container.querySelector("#adv-cipher");
  compressionSelect = container.querySelector("#adv-compression");

  generateButton = container.querySelector("#adv-generate");
  saveButton = container.querySelector("#adv-save");
  copyResultButton = container.querySelector("#adv-copy-result");

  generateButton.addEventListener("click", onGenerateClick);
  saveButton.addEventListener("click", onSaveClick);
  copyResultButton.addEventListener("click", onCopyResultClick);
}

export function unmount() {
  container = null;
  errorBannerEl = null;
  resultBlockEl = null;
  resultCodeEl = null;
  saveConfirmationEl = null;
}

function collectRaw() {
  return {
    nickname: nicknameInput.value.trim(),
    community: communityInput.value.trim(),
    key: keyInput.value,
    supernode: supernodeInput.value.trim(),
    mtu: mtuInput.value ? Number(mtuInput.value) : null,
    header_encryption: headerEncryptionInput.checked,
    cipher: cipherSelect.value || null,
    compression: compressionSelect.value || null,
  };
}

function showError(message) {
  errorBannerEl.textContent = message;
  errorBannerEl.hidden = false;
}

function clearError() {
  errorBannerEl.hidden = true;
}

async function onGenerateClick() {
  clearError();
  saveConfirmationEl.hidden = true;
  resultBlockEl.hidden = true;
  generateButton.disabled = true;
  try {
    const code = await generateInviteCode(collectRaw());
    resultCodeEl.value = code;
    resultBlockEl.hidden = false;
  } catch (err) {
    showError(err.message || "Couldn't generate an invite code from those fields.");
  } finally {
    generateButton.disabled = false;
  }
}

async function onSaveClick() {
  clearError();
  saveConfirmationEl.hidden = true;
  saveButton.disabled = true;
  try {
    const saved = await saveRawAsServer(collectRaw());
    saveConfirmationEl.textContent = `Saved "${saved.nickname}" to your server list.`;
    saveConfirmationEl.hidden = false;
  } catch (err) {
    if (err.isServiceNotInstalled) {
      showServiceSetupBanner(onSaveClick);
    } else {
      showError(err.message || "Couldn't save that server.");
    }
  } finally {
    saveButton.disabled = false;
  }
}

async function onCopyResultClick() {
  if (!resultCodeEl.value) return;
  try {
    await navigator.clipboard.writeText(resultCodeEl.value);
    const original = copyResultButton.textContent;
    copyResultButton.textContent = "Copied";
    setTimeout(() => {
      if (copyResultButton) copyResultButton.textContent = original;
    }, 1200);
  } catch (err) {
    console.error("Village: clipboard write failed", err);
  }
}
