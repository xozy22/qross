// Client-Ansicht

import { $, state } from "./core.js";
import { renderClientSharing } from "./sharing.js";
import { renderFinder } from "./discovery.js";
import { renderClipLimit } from "./clipboard-files.js";
import { qualityEntry } from "./quality.js";

export function renderClient() {
  const c = state.settings.client;
  const s = state.status;
  for (const [id, key] of [["#clientAddress", "address"], ["#clientName", "name"]]) {
    const input = $(id);
    if (document.activeElement !== input) input.value = c[key];
  }
  $("#clientDryRun").checked = c.dry_run;
  renderClipLimit();

  const isClient = s.role === "client";
  let phase = "stopped";
  if (isClient) phase = s.phase === "connected" ? "connected" : "connecting";
  else if (s.phase === "error") phase = "error";

  const visual = $("#linkVisual");
  visual.dataset.state = phase;
  visual.dataset.cursor = isClient && s.cursor_here ? "here" : "away";
  $("#visualServer").textContent = (isClient && s.server_name) || c.address || "Server";
  $("#visualClient").textContent = c.name || state.hostname || "Dieser Rechner";

  const hero = $(".client-hero");
  hero.dataset.phase = phase;
  const headline = {
    stopped: "Nicht verbunden",
    connecting: "Suche den Server …",
    connected: `Verbunden mit „${s.server_name}“`,
    error: "Verbindung fehlgeschlagen",
  }[phase];
  $("#clientHeadline").textContent = headline;

  let message = "Wähl einen gefundenen Server oder gib seine Adresse ein.";
  if (phase === "connecting") message = s.message;
  if (phase === "connected") {
    message = s.cursor_here
      ? "Der Cursor ist gerade auf diesem Rechner."
      : "Schieb die Maus am Server über den Bildschirmrand, um hierher zu wechseln.";
  }
  if (phase === "error") message = s.message;
  $("#clientMessage").textContent = message;
  renderClientSharing(phase === "connected" ? s.sharing : null);

  const qualitySlot = $("#clientQuality");
  qualitySlot.hidden = phase !== "connected";
  if (phase === "connected") {
    const quality = qualityEntry("server");
    if (qualitySlot.firstElementChild !== quality.pill) qualitySlot.replaceChildren(quality.pill, quality.panel);
  } else if (qualitySlot.firstElementChild) {
    qualitySlot.replaceChildren();
  }
  renderFinder();
}
