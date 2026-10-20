// Kopfzeile

import { $, $$, state } from "./core.js";
import { t, tn } from "./i18n.js";
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

  let text = s.message || t("topbar.stopped");
  if (s.phase === "stopped") text = t("topbar.stopped");
  if (s.role === "server" && s.phase === "listening" && s.clients.length) {
    text = tn("topbar.connectedCount", s.clients.length);
  }
  // Als Client nur mit wem; die Adresse steht im Tooltip und in der Client-Ansicht
  if (s.role === "client" && s.phase === "connected" && s.server_name) text = t("topbar.connectedTo", { name: s.server_name });
  // Neue Versuche: stabil statt im Wechsel „Verbinde …“ und Fehler; Einzelheiten im Tooltip und in der Client-Ansicht
  if (s.role === "client" && s.phase === "connecting") text = s.retry_reason ? t("topbar.unreachable") : t("topbar.connecting");
  if (s.role === "server" && s.locked) text = t("topbar.statusLocked", { status: text, name: s.cursor_on });
  else if (s.role === "server" && s.game_hold) text = t("topbar.statusGameHold", { status: text, name: s.cursor_on });
  $("#statusText").textContent = text;
  status.title = s.role === "client" && s.retry_reason ? t("topbar.retrying", { reason: s.retry_reason }) : s.message || "";

  const toggle = $("#toggle");
  const isRunning = running();
  toggle.classList.toggle("running", isRunning);
  const label =
    role === "client" ? (isRunning ? t("topbar.disconnect") : t("topbar.connect")) : isRunning ? t("topbar.stop") : t("topbar.start");
  $("#toggleText").textContent = label;
}
