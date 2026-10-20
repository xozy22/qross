// Client-Ansicht: Status, Verbindungsqualität, eingestellter Server, Serverwahl

import { $, PLATFORMS, state } from "./core.js";
import { t } from "./i18n.js";
import { el } from "./util.js";
import { renderClientSharing } from "./sharing.js";
import { renderFinder } from "./discovery.js";
import { renderClipLimit } from "./clipboard-files.js";
import { qualityEntry, updateQuality } from "./quality.js";

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
  $("#visualServer").textContent = (connected && s.server_name) || found?.name || c.address || t("clientView.server");
  $("#visualClient").textContent = c.name || state.hostname || t("common.thisComputer");

  // Status: was gerade gilt; „Verbunden mit …“ steht schon in der Kopfzeile
  $("#clientStatus").dataset.phase = phase;
  const server = s.server_name;
  // Scheitern die Versuche, bleibt der Grund stehen, statt mit „Verbinde …“ abzuwechseln.
  const retry = s.retry_reason
    ? t("clientView.retry", { reason: `${s.retry_reason.charAt(0).toUpperCase()}${s.retry_reason.slice(1)}` })
    : null;
  const [headline, message] = {
    stopped: [t("clientView.notConnected"), t("clientView.stoppedHint")],
    connecting: retry ? [t("clientView.unreachable"), retry] : [t("clientView.searching"), s.message],
    connected: s.cursor_here
      ? [t("clientView.cursorHere"), t("clientView.backTo", { server })]
      : [t("clientView.cursorOn", { server }), t("clientView.moveHere")],
    error: [t("clientView.failed"), s.message],
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
  $("#serverCardOs").title = os ? os.label : t("clientView.osUnknown");
  $("#serverCardName").textContent = name;
  const meta = [address !== name ? address : null, os?.label].filter(Boolean).join(" · ");
  const extra = found?.trust === "mismatch"
    ? el("span", { class: "warn", text: t("discovery.fingerprintMismatch") })
    : found?.swaps_modifiers
      ? el("span", { class: "swap", text: t("discovery.swapsModifiers") })
      : null;
  $("#serverCardMeta").replaceChildren(meta || (phase === "connected" ? t("clients.connected") : t("clientView.notYetConnected")), extra ? " · " : "", extra ?? "");
}
