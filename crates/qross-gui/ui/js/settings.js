// Einstellungen (Fenster mit den Bereichen)

import { $, $$, invoke, state } from "./core.js";
import { toast } from "./util.js";
import { openSheet, setupSheet } from "./dialog.js";
import { saveSoon, updatePreview } from "./store.js";
import { renderClipLimit } from "./clipboard-files.js";
import { openNews } from "./news.js";
import { loadKeyNames, renderKeys, setupKeys } from "./keys.js";
import { renderProfiles, watchNetworks } from "./profiles.js";

export async function refreshPrefs() {
  try {
    state.prefs = await invoke("get_prefs");
  } catch (e) {
    toast(String(e), "error");
  }
}

export async function openSettings(section) {
  const dialog = $("#settings");
  if (!dialog.open) {
    await loadKeyNames();
    await refreshPrefs();
    renderSettings();
    // Ohne Ziel wieder dort, wo man zuletzt war (bis zum Neustart)
    showSection(section || state.settingsSection);
    openSheet(dialog, $("#settingsClose"));
    renderProfiles();
    watchNetworks(true);
  } else if (section) {
    showSection(section);
  }
}

/** Zeigt einen Bereich der Einstellungen als eigene Seite; die anderen sind ausgeblendet. */
export function showSection(id) {
  if (!id || !$(`#${id}.settings-section`)) id = "set-general";
  state.settingsSection = id;
  for (const section of $$(".settings-section")) section.hidden = section.id !== id;
  for (const tab of $$(".settings-nav button")) {
    const active = tab.dataset.section === id;
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    // Schmales Fenster: Die Leiste liegt als Zeile oben und lässt sich schieben; den gewählten Bereich zeigen
    const nav = tab.parentElement;
    if (active && nav.scrollWidth > nav.clientWidth) {
      nav.scrollLeft = Math.max(0, Math.min(nav.scrollLeft, tab.offsetLeft - 12), tab.offsetLeft + tab.offsetWidth + 36 - nav.clientWidth);
    }
  }
  $("#settingsContent").scrollTop = 0;
}

/** Pfeiltasten, Pos1 und Ende wechseln zwischen den Bereichen (wie bei Registerkarten üblich). */
export function onSettingsNavKey(e) {
  const tabs = $$(".settings-nav button");
  const i = tabs.indexOf(document.activeElement);
  const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
  const next = step ? (i + step + tabs.length) % tabs.length : { Home: 0, End: tabs.length - 1 }[e.key];
  if (i < 0 || next === undefined) return;
  e.preventDefault();
  tabs[next].focus();
  showSection(tabs[next].dataset.section);
}

export function renderSettings() {
  const prefs = state.prefs;
  if (!prefs) return;
  renderKeys();
  const autostart = $("#prefAutostart");
  autostart.checked = prefs.autostart;
  autostart.disabled = !prefs.autostart_editable;
  autostart.closest(".setting").classList.toggle("disabled", !prefs.autostart_editable);
  // Grund, falls Qross sich nicht eintragen kann (Entwicklungsbuild, macOS-DMG, …), sonst wo der Eintrag liegt
  $("#autostartHint").textContent =
    prefs.autostart_note ?? `Qross startet bei der Anmeldung unsichtbar im Tray. Eintrag unter ${prefs.autostart_place}.`;
  $("#prefResume").checked = state.settings.resume_role;
  renderCursorFinder();
  $("#prefAutoUpdate").checked = state.settings.auto_update;
  $("#prefGameMode").checked = state.settings.game_mode;
  renderSaver();
  $("#prefKeepSecrets").checked = !state.settings.clipboard_share_concealed;
  $("#aboutVersion").textContent = state.version;
  $("#aboutDataDir").textContent = prefs.data_dir;
  renderClipLimit();
  renderUpdateStatus();
}

/** Was der Cursor-Finder zeigt, in Worten */
export function finderText(f) {
  if (f.ring && f.name) return "Ring und Name";
  if (f.ring) return "nur den Ring";
  if (f.name) return "nur den Namen";
  return "nichts";
}

/** Cursor-Finder: Als Server eingestellt und an die Clients weitergegeben; als Client gilt, was der Server will. */
export function renderCursorFinder() {
  const isClient = state.settings.role === "client";
  const status = state.status;
  const fromServer = isClient && status?.role === "client" && status.phase === "connected" ? status.cursor_finder ?? null : null;
  const shown = isClient ? fromServer ?? { ring: false, name: false } : state.settings.cursor_finder;
  $("#prefFinderRing").checked = shown.ring;
  $("#prefFinderName").checked = shown.name;
  for (const id of ["#prefFinderRing", "#prefFinderName"]) {
    $(id).disabled = isClient;
    $(id).closest(".setting").classList.toggle("disabled", isClient);
  }
  const note = $("#finderClientNote");
  note.hidden = !isClient;
  if (isClient) {
    const connected = status?.role === "client" && status.phase === "connected";
    note.textContent = !connected
      ? "Stellt der Server ein, für alle Rechner. Dieser Rechner ist gerade als Client eingestellt."
      : fromServer
        ? `Stellt der Server ein: „${status.server_name}“ zeigt hier ${finderText(fromServer)}.`
        : `Stellt der Server ein: „${status.server_name}“ zeigt hier nichts (aus oder Qross vor 0.8).`;
  }
  $("#finderHint").textContent =
    state.desktop === "wayland"
      ? "Unter Wayland lässt sich nichts über andere Fenster legen; dieser Rechner zeigt selbst nichts, Clients schon."
      : "Gilt für alle Rechner: Der Server gibt es an Clients ab Qross 0.8 weiter. Über Programmen im Vollbild erscheint nichts.";
}

/** „heute um 14:02“, „gestern um 9:15“, sonst „am 3. Oktober um 14:02“ */
export function whenText(ms) {
  const d = new Date(ms);
  const time = d.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return `heute um ${time}`;
  if (d.toDateString() === yesterday.toDateString()) return `gestern um ${time}`;
  return `am ${d.toLocaleDateString("de-DE", { day: "numeric", month: "long" })} um ${time}`;
}

export function updateStatusText(u) {
  const checked = u.checked_at ? `, geprüft ${whenText(u.checked_at)}` : "";
  switch (u.phase) {
    case "checking":
      return ["Suche nach Updates …", ""];
    case "current":
      return [`Qross ${u.current} ist aktuell${checked}.`, ""];
    case "downloading":
      return [`Update ${u.version} wird geladen … ${u.progress ?? 0} %`, "new"];
    case "waiting":
      return [`Update ${u.version} ist geladen und wird installiert, sobald Maus und Tastatur gerade nicht gebraucht werden.`, "new"];
    case "installing":
      return [`Update ${u.version} wird installiert, Qross startet gleich neu.`, "new"];
    case "available": {
      const prompt = u.bundle === "msi" ? " Die Installation fragt nach Administratorrechten."
        : u.bundle === "deb" || u.bundle === "rpm" ? " Die Installation fragt nach dem Passwort." : "";
      return [`Update ${u.version} ist verfügbar.${prompt}`, "new"];
    }
    case "error":
      return [`${u.message || "Prüfung fehlgeschlagen"}${u.checked_at ? `. Zuletzt erfolgreich geprüft ${whenText(u.checked_at)}.` : "."}`, "error"];
    default:
      return [u.automatic ? "Die erste Prüfung folgt kurz nach dem Start." : `Qross ${u.current}. Automatische Prüfung nur in installierten Versionen.`, ""];
  }
}

export function renderUpdateStatus() {
  const u = state.prefs?.update;
  if (!u) return;
  const [text, tone] = updateStatusText(u);
  const line = $("#updateStatus");
  line.textContent = text;
  line.dataset.tone = tone;
  const busy = ["checking", "downloading", "installing"].includes(u.phase);
  $("#updateCheck").disabled = busy;
  $("#updateInstall").hidden = u.phase !== "available";
}

/** Bereich „Bildschirmschoner“: wirkt nur als Server; Mitsperren nur mit Abgleich */
export function renderSaver() {
  const { screensaver_sync: sync, screensaver_lock_clients: lock } = state.settings;
  $("#saverClientNote").hidden = state.settings.role !== "client";
  $("#prefSaverSync").checked = sync;
  const lockBox = $("#prefSaverLock");
  lockBox.checked = sync && lock;
  lockBox.disabled = !sync;
  lockBox.closest(".setting").classList.toggle("disabled", !sync);
}

export function setupSettings() {
  setupKeys();
  $("#settingsBtn").addEventListener("click", () => openSettings());
  setupSheet($("#settings"), $("#settingsClose"));
  for (const b of $$(".settings-nav button")) b.addEventListener("click", () => showSection(b.dataset.section));
  $(".settings-nav").addEventListener("keydown", onSettingsNavKey);

  $("#prefAutostart").addEventListener("change", async (e) => {
    try {
      state.prefs.autostart = await invoke("set_autostart", { enabled: e.target.checked });
    } catch (err) {
      toast(String(err), "error");
      await refreshPrefs();
    }
    renderSettings();
  });
  $("#prefResume").addEventListener("change", (e) => {
    state.settings.resume_role = e.target.checked;
    saveSoon.flush();
  });
  $("#prefAutoUpdate").addEventListener("change", (e) => {
    state.settings.auto_update = e.target.checked;
    saveSoon.flush();
  });
  for (const [id, key] of [["#prefFinderRing", "ring"], ["#prefFinderName", "name"]]) {
    $(id).addEventListener("change", (e) => {
      state.settings.cursor_finder = { ...state.settings.cursor_finder, [key]: e.target.checked };
      updatePreview();
      saveSoon.flush();
    });
  }
  $("#finderTry").addEventListener("click", async () => {
    // Ist alles aus, zeigt „Ausprobieren“ trotzdem beides, damit man sieht, wie es aussähe.
    const ring = $("#prefFinderRing").checked, name = $("#prefFinderName").checked;
    const style = ring || name ? { ring, name } : { ring: true, name: true };
    try {
      if (!(await invoke("try_cursor_finder", style))) toast("Unter Wayland lässt sich der Cursor-Finder nicht zeigen.");
    } catch (e) {
      toast(String(e), "error");
    }
  });
  $("#prefGameMode").addEventListener("change", (e) => {
    state.settings.game_mode = e.target.checked;
    saveSoon.flush();
  });
  $("#prefSaverSync").addEventListener("change", (e) => {
    state.settings.screensaver_sync = e.target.checked;
    renderSaver();
    updatePreview();
    saveSoon.flush();
  });
  $("#prefSaverLock").addEventListener("change", (e) => {
    state.settings.screensaver_lock_clients = e.target.checked;
    updatePreview();
    saveSoon.flush();
  });
  $("#prefKeepSecrets").addEventListener("change", (e) => {
    state.settings.clipboard_share_concealed = !e.target.checked;
    updatePreview();
    saveSoon.flush();
  });
  $("#updateCheck").addEventListener("click", () => invoke("check_updates"));
  $("#updateInstall").addEventListener("click", () => invoke("install_update"));
  $("#openDataDir").addEventListener("click", () => invoke("open_data_dir").catch((e) => toast(String(e), "error")));
  $("#aboutProject").addEventListener("click", () => invoke("open_project_page").catch((e) => toast(String(e), "error")));
  $("#aboutNews").addEventListener("click", () => {
    $("#settings").close();
    openNews();
  });
}
