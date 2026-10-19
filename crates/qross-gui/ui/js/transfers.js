// Dateiübertragungen

import { $, invoke, state } from "./core.js";
import { el, icon, running, toast } from "./util.js";
import { TOAST_KIND } from "./clipboard-files.js";
import { NBSP } from "./quality.js";

/** Erst ab dieser Laufzeit erscheint eine Zeile; kürzere Übertragungen würden nur aufblitzen. */
export const TRANSFER_SHOW_AFTER_MS = 500;
/** So lange bleibt eine beendete Zeile stehen */
export const TRANSFER_LINGER_MS = { done: 5000, cancelled: 4000, failed: 9000 };
/** Angezeigte Übertragungen: id → { data, node, parts, final, timer }. Die Knoten bleiben bestehen
 *  und werden nur aktualisiert, damit bei vier Meldungen pro Sekunde nichts flackert. */
export const transfers = new Map();

export function transferRow(t) {
  const parts = {
    name: el("strong", { class: "tname" }),
    route: el("span", { class: "troute" }),
    pct: el("span", { class: "tpct" }),
    fill: el("span", { class: "tfill" }),
    outcome: el("span", { class: "toutcome" }),
    size: el("span", { class: "tsize" }),
    speed: el("span", { class: "tspeed" }),
  };
  parts.bar = el("div", { class: "tbar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100" }, parts.fill);
  parts.cancel = el("button", {
    type: "button",
    class: "btn ghost small tcancel",
    text: "Abbrechen",
    "aria-label": `Übertragung von ${t.name} abbrechen`,
    onclick: () => cancelTransfer(t.id),
  });
  const node = el(
    "div",
    { class: "transfer", "data-direction": t.direction, "data-state": "running" },
    icon(t.direction === "sending" ? "up" : "down"),
    el(
      "div",
      { class: "tmain" },
      el("div", { class: "tline" }, parts.name, parts.route, parts.pct),
      parts.bar,
      el("div", { class: "tmeta" }, parts.outcome, parts.size, parts.speed),
    ),
    parts.cancel,
  );
  return { node, parts, final: false, timer: null };
}

export function showPercent(entry, percent) {
  const whole = Math.floor(percent);
  entry.parts.pct.textContent = `${whole}${NBSP}%`;
  entry.parts.fill.style.width = `${percent}%`;
  entry.parts.bar.setAttribute("aria-valuenow", whole);
}

/** Meldung aus dem Backend (etwa 4×/s je Übertragung, zum Schluss mit Ergebnis) */
export function onTransfer(t) {
  let entry = transfers.get(t.id);
  if (!entry) {
    // Kurze Übertragungen bekommen keine Zeile, ein Ende ohne Zeile braucht keine.
    if (t.state !== "running" || t.elapsed_ms < TRANSFER_SHOW_AFTER_MS) return;
    entry = transferRow(t);
    transfers.set(t.id, entry);
    $("#transfers").append(entry.node);
    $("#transfers").hidden = false;
  }
  // Schon beendet, etwa durch „Kopierte Dateien sind bereit“
  if (entry.final) return;
  entry.data = t;
  const { parts } = entry;
  parts.name.textContent = t.name;
  parts.name.title = t.name;
  parts.route.textContent = t.route;
  parts.size.textContent = t.size;
  parts.speed.textContent = t.speed;
  showPercent(entry, t.percent);
  if (t.state !== "running") finishTransfer(entry, t.outcome, t.state);
}

/** Ergebnis in die Zeile schreiben; sie verschwindet nach ein paar Sekunden. */
export function finishTransfer(entry, text, state) {
  entry.final = true;
  entry.node.dataset.state = state;
  entry.parts.outcome.textContent = text || "";
  entry.parts.cancel.disabled = true;
  if (state === "done") showPercent(entry, 100);
  clearTimeout(entry.timer);
  entry.timer = setTimeout(() => removeTransfer(entry), TRANSFER_LINGER_MS[state] ?? 4000);
}

export function removeTransfer(entry) {
  entry.node.classList.add("leaving");
  setTimeout(() => {
    if (transfers.get(entry.data.id) === entry) transfers.delete(entry.data.id);
    entry.node.remove();
    $("#transfers").hidden = transfers.size === 0;
  }, 200);
}

/** Gestoppt oder neu gestartet: Meldungen der alten Instanz kommen nicht mehr. */
export function clearTransfers() {
  if (!transfers.size) return;
  for (const entry of transfers.values()) {
    clearTimeout(entry.timer);
    entry.node.remove();
  }
  transfers.clear();
  $("#transfers").hidden = true;
}

export async function cancelTransfer(id) {
  const entry = transfers.get(id);
  if (entry) entry.parts.cancel.disabled = true;
  try {
    // Das Ende kommt als Meldung, auch wenn die Übertragung gerade von selbst fertig wurde.
    await invoke("cancel_transfer", { id });
  } catch (e) {
    toast(String(e), "error");
    if (entry && !entry.final) entry.parts.cancel.disabled = false;
  }
}

/** Zeile für kopierte Dateien, die gerade von `peer` ankommen (oder eben angekommen sind) */
export function clipboardRow(peer) {
  return [...transfers.values()]
    .reverse()
    .find((e) => e.data.kind === "clipboard" && e.data.direction === "receiving" && e.data.peer === peer);
}

/** Hinweis zu kopierten Dateien: „bereit“ und Fehler landen in deren Zeile, sonst als Toast. */
export function onClipboardFiles(message) {
  if (!message.text) return;
  const row = (message.event === "ready" || message.event === "failed") && clipboardRow(message.peer);
  if (row) finishTransfer(row, message.text, message.event === "ready" ? "done" : "failed");
  else toast(message.text, TOAST_KIND[message.tone] ?? "");
}
