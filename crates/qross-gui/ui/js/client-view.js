// Client-Ansicht: Status, Verbindungsqualität, eingestellter Server, Serverwahl

import { $, PLATFORMS, state } from "./core.js";
import { el } from "./util.js";
import { renderClientSharing } from "./sharing.js";
import { renderFinder } from "./discovery.js";
import { renderClipLimit } from "./clipboard-files.js";
import { qualityEntry, updateQuality } from "./quality.js";
import { syncUsage } from "./usage.js";

/** Phase beim letzten Aufbau: Die Serverwahl klappt nur beim Wechsel auf oder zu, sonst bleibt sie, wie man sie lässt. */
let lastPhase = null;

export function clientPhase() {
  const s = state.status;
  if (s.role === "client") return s.phase === "connected" ? "connected" : "connecting";
  return s.phase === "error" ? "error" : "stopped";
}

export function renderClient() {
  const c = state.settings.client;
  const s = state.status;
  for (const [id, key] of [["#clientAddress", "address"], ["#clientName", "name"]]) {
    const input = $(id);
    if (document.activeElement !== input) input.value = c[key];
  }
  $("#clientDryRun").checked = c.dry_run;
  renderClipLimit();

  const phase = clientPhase();
  const connected = phase === "connected";
  const visual = $("#linkVisual");
  visual.dataset.state = phase;
  visual.dataset.cursor = connected && s.cursor_here ? "here" : "away";
  const found = state.found?.find((f) => f.address === c.address.trim());
  $("#visualServer").textContent = (connected && s.server_name) || found?.name || c.address || "Server";
  $("#visualClient").textContent = c.name || state.hostname || "Dieser Rechner";

  // Status: was gerade gilt; „Verbunden mit …“ steht schon in der Kopfzeile
  $("#clientStatus").dataset.phase = phase;
  const server = `„${s.server_name}“`;
  // Scheitern die Versuche, bleibt der Grund stehen, statt mit „Verbinde …“ abzuwechseln.
  const retry = s.retry_reason ? `${s.retry_reason.charAt(0).toUpperCase()}${s.retry_reason.slice(1)}. Qross versucht es weiter.` : null;
  const [headline, message] = {
    stopped: ["Nicht verbunden", "Wähl unten einen gefundenen Server oder gib seine Adresse ein."],
    connecting: retry ? ["Server nicht erreichbar", retry] : ["Suche den Server …", s.message],
    connected: s.cursor_here
      ? ["Der Cursor ist hier", `Zurück zu ${server} geht es über den Bildschirmrand.`]
      : [`Der Cursor ist auf ${server}`, "Schieb die Maus dort über den Bildschirmrand, um hierher zu wechseln."],
    error: ["Verbindung fehlgeschlagen", s.message],
  }[phase];
  $("#clientHeadline").textContent = headline;
  $("#clientMessage").textContent = message;
  renderClientSharing(connected ? s.sharing : null);

  // Latenz im Status, Verlauf und Messwerte in der Karte darunter
  const quality = connected ? qualityEntry("server") : null;
  const status = $("#clientStatus");
  const pill = status.querySelector(":scope > .qpill");
  if (quality && pill !== quality.pill) {
    pill?.remove();
    status.append(quality.pill);
  } else if (!quality) {
    pill?.remove();
  }
  $("#clientConn").hidden = !connected;
  const slot = $("#clientQuality");
  if (quality && slot.firstElementChild !== quality.panel) {
    slot.replaceChildren(quality.panel);
    updateQuality(quality);
  } else if (!quality && slot.firstElementChild) {
    slot.replaceChildren();
  }

  renderServerCard(phase);
  const finder = $("#finderBox");
  if (phase !== lastPhase) {
    // Verbunden: Serverwahl einklappen; sonst ist sie das Wichtigste und steht offen.
    if (connected) finder.open = false;
    else if (lastPhase === null || lastPhase === "connected" || !c.address.trim()) finder.open = true;
    lastPhase = phase;
  }
  renderFinder();
  // Eigene CPU und Arbeitsspeicher stehen in der Verbindungskarte: nur messen, solange sie zu sehen ist
  syncUsage();
}

/** Der eingestellte Server: Name und System, soweit bekannt (aus der Suche bzw. der Verbindung) */
function renderServerCard(phase) {
  const c = state.settings.client;
  const s = state.status;
  const address = c.address.trim();
  const card = $("#clientServer");
  card.hidden = !address;
  if (!address) return;
  const found = state.found?.find((f) => f.address === address);
  const name = (phase === "connected" && s.server_name) || found?.name || address;
  const os = found ? PLATFORMS[found.platform] || PLATFORMS.other : null;
  $("#serverCardOs").textContent = os ? os.short : "?";
  $("#serverCardOs").title = os ? os.label : "System unbekannt";
  $("#serverCardName").textContent = name;
  const meta = [address !== name ? address : null, os?.label].filter(Boolean).join(" · ");
  const extra = found?.trust === "mismatch"
    ? el("span", { class: "warn", text: "Fingerprint weicht vom gespeicherten ab" })
    : found?.swaps_modifiers
      ? el("span", { class: "swap", text: "Strg und Cmd werden automatisch getauscht" })
      : null;
  $("#serverCardMeta").replaceChildren(meta || (phase === "connected" ? "verbunden" : "noch nicht verbunden"), extra ? " · " : "", extra ?? "");
}
