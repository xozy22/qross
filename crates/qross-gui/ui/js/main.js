// Gesamt: Neuaufbau der Oberfläche und Start

import { $, $$, invoke, listen, state } from "./core.js";
import { el, fillFingerprint, running, store, toast } from "./util.js";
import { confirmRoleChange } from "./dialog.js";
import { saveSoon, settingsChanged, updatePreview } from "./store.js";
import { renderTopbar } from "./topbar.js";
import { addScreen, renderGameNote, renderServer, setServerName } from "./layout.js";
import { renderClients } from "./clients.js";
import { renderClient } from "./client-view.js";
import { checkServer, searchServers } from "./discovery.js";
import { startFlow, toggle } from "./start.js";
import { appendLogs, filterLogs, setLogOpen } from "./log.js";
import { TOAST_KIND, setupClipboardFiles } from "./clipboard-files.js";
import { clearTransfers, onClipboardFiles, onTransfer } from "./transfers.js";
import { openNews, setupNews } from "./news.js";
import { openSettings, renderCursorFinder, renderProfileScope, renderUpdateStatus, setupSettings } from "./settings.js";
import { renderKeyModes } from "./keys.js";
import { pickProfile, renderProfilePick, renderProfiles, setupProfiles } from "./profiles.js";
import { onQuality, pruneQualities } from "./quality.js";
import { onLatency } from "./latency.js";
import { checkPermissions, renderPermissions } from "./permissions.js";
import { forgetPortal, refreshPortal, renderPortal } from "./portal.js";
import { renderSecureInput } from "./secure-input.js";
import { setupWindowControls } from "./window-controls.js";

export function render() {
  pruneQualities();
  renderPermissions();
  renderPortal();
  renderSecureInput();
  renderTopbar();
  renderProfilePick();
  renderGameNote();
  renderServer();
  renderClients();
  renderClient();
  // Verbundene Rechner melden, ob sie Zeichen eingeben können; als Client meldet der Server den Cursor-Finder
  if ($("#settings").open && state.settings) {
    renderKeyModes();
    renderCursorFinder();
    renderProfileScope();
  }
}

// Der Status kommt bei jedem Wechsel des Cursors: höchstens einmal je Bild neu aufbauen. Solange das
// Fenster nicht zu sehen ist, ruht requestAnimationFrame; gezeigt wird dann gleich der neueste Stand.
let renderQueued = false;
function renderSoon() {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    render();
  });
}

async function init() {
  // Vor get_state anmelden: Wechselt das Profil schon beim Start (Netzwerk), steckt das entweder im
  // Abbild oder kommt als Ereignis. Bis zum Abbild gibt es nichts zu aktualisieren.
  await listen("settings", (e) => {
    if (!state.settings) return;
    state.settings = e.payload;
    render();
    updatePreview();
    renderProfiles();
  });
  await listen("profiles", (e) => {
    if (!state.profiles) return;
    state.profiles = e.payload;
    renderProfilePick();
    renderProfiles();
    if ($("#settings").open) renderProfileScope();
  });
  const snap = await invoke("get_state");
  Object.assign(state, {
    settings: snap.settings,
    profiles: snap.profiles,
    status: snap.status,
    fingerprint: snap.fingerprint,
    hostname: snap.hostname,
    localIp: snap.local_ip,
    grid: snap.grid,
    platform: snap.platform,
    desktop: snap.desktop ?? state.desktop,
    version: snap.version,
  });
  // Titelleiste: braucht das System; ein Fehler darf den Rest der Oberfläche nicht aufhalten
  setupWindowControls().catch((e) => console.warn("Fensterknöpfe:", e));
  for (const node of $$("[data-own-fingerprint]")) fillFingerprint(node, snap.fingerprint);
  for (const node of $$("[data-data-dir]")) node.textContent = snap.data_dir;
  $("#clientName").placeholder = snap.hostname;
  appendLogs(snap.logs);
  setLogOpen(store("qross.logOpen") === "1");
  if (state.platform === "macos") {
    state.permissions = await invoke("permissions");
    // Zurück aus den Systemeinstellungen: sofort neu prüfen
    window.addEventListener("focus", checkPermissions);
  }
  render();
  updatePreview();
  refreshPortal();
  $("#portalForget").addEventListener("click", forgetPortal);

  await listen("status", (e) => {
    const before = state.status;
    state.status = e.payload;
    if (!running() || state.status.phase === "starting") clearTransfers();
    renderSoon();
    // Unter Wayland nach jedem Phasenwechsel (Start, Fehler, verbunden) die Portale neu ansehen
    if (before?.phase !== e.payload.phase || before?.role !== e.payload.role) refreshPortal();
  });
  await listen("log", (e) => appendLogs([e.payload]));
  await listen("quality", (e) => onQuality(e.payload));
  await listen("latency", (e) => onLatency(e.payload));
  await listen("transfer", (e) => onTransfer(e.payload));
  await listen("start-from-tray", () => {
    if (!running()) startFlow();
  });
  // Profil aus dem Tray, das die Rolle wechseln würde: erst hier nachfragen
  await listen("confirm-profile", (e) => pickProfile(e.payload));
  await listen("show-news", () => openNews());
  await listen("show-settings", (e) => openSettings(e.payload || undefined));
  await listen("update-status", (e) => {
    if (state.prefs) state.prefs.update = e.payload;
    renderUpdateStatus();
  });
  // Hinweise aus dem Backend (z. B. „Update verfügbar“); Fehler kommen nie als Hinweis.
  await listen("app-notice", (e) => {
    const { tone, text, action } = e.payload;
    const button = action === "install-update" ? { label: "Jetzt aktualisieren", run: () => invoke("install_update") } : null;
    toast(text, TOAST_KIND[tone] ?? "", button);
  });
  setupSettings();
  setupProfiles();
  // Kopierte Dateien: dezente Hinweise im Fenster (Texte kommen fertig aus dem Backend)
  await listen("clipboard-files", (e) => onClipboardFiles(e.payload));
  setupClipboardFiles();
  setupNews();

  for (const b of $$(".segmented button")) {
    b.addEventListener("click", async () => {
      if (state.settings.role === b.dataset.role) return;
      if (!(await confirmRoleChange(b.dataset.role))) return;
      state.settings.role = b.dataset.role;
      render();
      if (b.dataset.role === "client" && !running()) searchServers();
      await saveSoon.flush();
    });
  }
  $("#toggle").addEventListener("click", toggle);
  $("#addScreen").addEventListener("click", () => addScreen());
  $("#serverName").addEventListener("input", (e) => setServerName(e.target.value.trim()));
  $("#serverPort").addEventListener("input", (e) => {
    const port = Number(e.target.value);
    if (port >= 1 && port <= 65535) {
      state.settings.server.port = port;
      settingsChanged();
    }
  });
  $("#clientAddress").addEventListener("input", (e) => {
    state.settings.client.address = e.target.value;
    saveSoon();
    renderClient();
  });
  $("#clientName").addEventListener("input", (e) => {
    state.settings.client.name = e.target.value.trim();
    saveSoon();
    renderClient();
  });
  $("#clientDryRun").addEventListener("change", (e) => {
    state.settings.client.dry_run = e.target.checked;
    saveSoon.flush();
  });
  $("#clientAddress").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !running()) startFlow();
  });
  $("#checkServer").addEventListener("click", checkServer);
  $("#searchServers").addEventListener("click", searchServers);
  const numberSetting = (id, key, max) =>
    $(id).addEventListener("input", (e) => {
      const value = Number(e.target.value);
      if (e.target.value !== "" && Number.isInteger(value) && value >= 0 && value <= max) {
        state.settings.server[key] = value;
        settingsChanged();
      }
    });
  numberSetting("#cornerSize", "corner_size", 400);
  numberSetting("#switchDelay", "switch_delay_ms", 3000);
  $("#discovery").addEventListener("change", (e) => {
    state.settings.server.discovery = e.target.checked;
    settingsChanged({ immediate: true });
  });
  if (state.settings.role === "client" && !running()) searchServers();
  $("#logLevel").addEventListener("change", filterLogs);
  $("#logClear").addEventListener("click", () => {
    $("#log").replaceChildren();
    $("#logLast").textContent = "";
  });
  $("#logToggle").addEventListener("click", () => setLogOpen($("#logPanel").dataset.open !== "true"));
  // Das „?“ steht oft in einem <label>: Ein Klick darauf soll nur erklären, nicht den Schalter umlegen.
  document.addEventListener("click", (e) => {
    if (e.target.closest?.(".qinfo")) e.preventDefault();
  });
}

init().catch((e) => {
  document.body.prepend(el("div", { class: "note error", text: `Die Oberfläche konnte nicht geladen werden: ${e}` }));
});
