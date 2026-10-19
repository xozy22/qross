// Protokoll

import { $ } from "./core.js";
import { el, store } from "./util.js";

export const LEVELS = { TRACE: 0, DEBUG: 0, INFO: 1, WARN: 2, ERROR: 2 };

export function logLine(line) {
  const t = new Date(line.ts).toLocaleTimeString("de-DE");
  const node = el("div", { class: line.level, "data-rank": LEVELS[line.level] ?? 1 }, el("span", { class: "t", text: t }), el("span", { class: "l", text: line.level }), line.message);
  node.hidden = Number(node.dataset.rank) < Number($("#logLevel").value);
  return node;
}

export function appendLogs(lines) {
  if (!lines.length) return;
  const log = $("#log");
  const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 8;
  log.append(...lines.map(logLine));
  while (log.childElementCount > 1500) log.firstElementChild.remove();
  if (atBottom) log.scrollTop = log.scrollHeight;
  const last = lines[lines.length - 1];
  const lastNode = $("#logLast");
  lastNode.textContent = last.message;
  lastNode.className = `log-last ${last.level}`;
}

export function filterLogs() {
  const min = Number($("#logLevel").value);
  for (const node of $("#log").children) node.hidden = Number(node.dataset.rank) < min;
  $("#log").scrollTop = $("#log").scrollHeight;
}

export function setLogOpen(open) {
  $("#logPanel").dataset.open = String(open);
  $("#logToggle").setAttribute("aria-expanded", String(open));
  store("qross.logOpen", open ? "1" : "0");
  if (open) $("#log").scrollTop = $("#log").scrollHeight;
}
