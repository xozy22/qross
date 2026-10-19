// Kopfzeile

import { $, $$, state } from "./core.js";
import { running } from "./util.js";

export function renderTopbar() {
  const role = state.settings.role;
  for (const b of $$(".segmented button")) b.setAttribute("aria-checked", String(b.dataset.role === role));
  $("#serverView").hidden = role !== "server";
  $("#clientView").hidden = role !== "client";

  const s = state.status;
  const status = $("#status");
  status.dataset.phase = s.phase;
  status.dataset.clients = s.clients.length ? "yes" : "no";
  status.dataset.locked = s.role === "server" && (s.locked || s.game_hold) ? "yes" : "no";

  let text = s.message || "Gestoppt";
  if (s.phase === "stopped") text = "Gestoppt";
  if (s.role === "server" && s.phase === "listening" && s.clients.length) {
    text = s.clients.length === 1 ? "1 Rechner verbunden" : `${s.clients.length} Rechner verbunden`;
  }
  // Als Client nur mit wem; die Adresse steht im Tooltip und in der Client-Ansicht
  if (s.role === "client" && s.phase === "connected" && s.server_name) text = `Verbunden mit „${s.server_name}“`;
  // Neue Versuche: stabil statt im Wechsel „Verbinde …“ und Fehler; Einzelheiten im Tooltip und in der Client-Ansicht
  if (s.role === "client" && s.phase === "connecting") text = s.retry_reason ? "Server nicht erreichbar" : "Verbindet …";
  if (s.role === "server" && s.locked) text += `, Cursor auf „${s.cursor_on}“ gesperrt`;
  else if (s.role === "server" && s.game_hold) text += `, Spielemodus hält den Cursor auf „${s.cursor_on}“`;
  $("#statusText").textContent = text;
  status.title = s.role === "client" && s.retry_reason ? `${s.retry_reason}. Qross versucht es weiter.` : s.message || "";

  const toggle = $("#toggle");
  const isRunning = running();
  toggle.classList.toggle("running", isRunning);
  const label = role === "client" ? (isRunning ? "Trennen" : "Verbinden") : isRunning ? "Stoppen" : "Starten";
  $("#toggleText").textContent = label;
}
