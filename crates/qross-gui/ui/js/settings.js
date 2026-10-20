// Einstellungen (Fenster mit den Bereichen)

import { $, $$, invoke, state } from "./core.js";
import { closeMenu, store, toast } from "./util.js";
import { LANG_KEY, language, locale, resolveLanguage, t } from "./i18n.js";
import { openSheet, setupSheet } from "./dialog.js";
import { saveSoon, updatePreview } from "./store.js";
import { renderClipLimit } from "./clipboard-files.js";
import { openNews } from "./news.js";
import { loadKeyNames, renderKeys, setupKeys } from "./keys.js";
import { activeProfile, renderProfiles, watchNetworks } from "./profiles.js";

/** Bereich, den die Einstellungen nach dem Neuladen (Sprachwechsel) wieder zeigen */
export const REOPEN_SETTINGS = "qross.reopenSettings";
/** Sprachnamen in ihrer eigenen Sprache, damit man aus jeder Sprache zurückfindet */
export const LANGUAGE_NAMES = { de: "Deutsch", en: "English" };

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

/** Gruppe „Profil …“ in der Navigation und ob Server bzw. Client gerade gelten */
export function renderProfileScope() {
  const profile = activeProfile();
  $("#navProfileGroup").textContent = profile ? t("settings.tab.profileNamed", { name: profile.name }) : t("settings.tab.profile");
  for (const id of ["#tab-server", "#tab-client"]) {
    const tab = $(id).textContent;
    $(id).setAttribute("aria-label", profile ? t("settings.tab.inProfile", { tab, name: profile.name }) : tab);
  }
  const role = state.settings.role;
  $("#serverRoleNote").hidden = role !== "client";
  $("#clientRoleNote").hidden = role !== "server";
}

export function renderSettings() {
  const prefs = state.prefs;
  if (!prefs) return;
  renderProfileScope();
  renderLanguage();
  renderKeys();
  const autostart = $("#prefAutostart");
  autostart.checked = prefs.autostart;
  autostart.disabled = !prefs.autostart_editable;
  autostart.closest(".setting").classList.toggle("disabled", !prefs.autostart_editable);
  // Grund, falls Qross sich nicht eintragen kann (Entwicklungsbuild, macOS-DMG, …), sonst wo der Eintrag liegt
  $("#autostartHint").textContent = prefs.autostart_note ?? t("settings.general.autostartPlace", { place: prefs.autostart_place });
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

/** Auswahl der Sprache; „Automatisch“ nennt die Sprache, die das System gerade ergibt. */
export function renderLanguage() {
  const auto = resolveLanguage("auto", state.systemLanguage);
  $("#prefLanguageAuto").textContent = t("settings.general.languageAuto", { language: LANGUAGE_NAMES[auto] });
  $("#prefLanguage").value = LANGUAGE_NAMES[state.settings.language] ? state.settings.language : "auto";
}

/** Was der Cursor-Finder zeigt, in Worten */
export function finderText(f) {
  if (f.ring && f.name) return t("settings.finder.both");
  if (f.ring) return t("settings.finder.ring");
  if (f.name) return t("settings.finder.name");
  return t("settings.finder.none");
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
    const server = status?.server_name;
    note.textContent = !connected
      ? t("settings.finder.clientNote")
      : fromServer
        ? t("settings.finder.fromServer", { server, what: finderText(fromServer) })
        : t("settings.finder.fromServerOff", { server });
  }
  $("#finderHint").textContent = t(state.desktop === "wayland" ? "settings.finder.wayland" : "settings.finder.all");
}

/** „heute um 14:02“, „gestern um 9:15“, sonst „am 3. Oktober um 14:02“ (englisch entsprechend) */
export function whenText(ms) {
  const d = new Date(ms);
  const time = d.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" });
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return t("settings.when.today", { time });
  if (d.toDateString() === yesterday.toDateString()) return t("settings.when.yesterday", { time });
  return t("settings.when.date", { date: d.toLocaleDateString(locale(), { day: "numeric", month: "long" }), time });
}

export function updateStatusText(u) {
  const version = u.version;
  switch (u.phase) {
    case "checking":
      return [t("settings.updates.checking"), ""];
    case "current":
      return [
        u.checked_at
          ? t("settings.updates.currentChecked", { version: u.current, when: whenText(u.checked_at) })
          : t("settings.updates.current", { version: u.current }),
        "",
      ];
    case "downloading":
      return [t("settings.updates.downloading", { version, progress: u.progress ?? 0 }), "new"];
    case "waiting":
      return [t("settings.updates.waiting", { version }), "new"];
    case "installing":
      return [t("settings.updates.installing", { version }), "new"];
    case "available": {
      const prompt = u.bundle === "msi" ? t("settings.updates.promptAdmin")
        : u.bundle === "deb" || u.bundle === "rpm" ? t("settings.updates.promptPassword") : "";
      return [[t("settings.updates.available", { version }), prompt].filter(Boolean).join(" "), "new"];
    }
    case "error": {
      const message = u.message || t("settings.updates.failed");
      return [u.checked_at ? `${message}. ${t("settings.updates.lastChecked", { when: whenText(u.checked_at) })}` : `${message}.`, "error"];
    }
    default:
      return [u.automatic ? t("settings.updates.first") : t("settings.updates.devBuild", { version: u.current }), ""];
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
  $("#settings").addEventListener("close", () => closeMenu());
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
  $("#prefLanguage").addEventListener("change", async (e) => {
    state.settings.language = e.target.value;
    await saveSoon.flush();
    const lang = resolveLanguage(e.target.value, state.systemLanguage);
    if (lang === language()) return;
    // Neu laden: So kommt jeder Text in der neuen Sprache, auch einmal aufgebaute Listen und Menüs.
    store(LANG_KEY, lang);
    store(REOPEN_SETTINGS, state.settingsSection);
    window.location.reload();
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
      if (!(await invoke("try_cursor_finder", style))) toast(t("settings.finder.waylandTry"));
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
