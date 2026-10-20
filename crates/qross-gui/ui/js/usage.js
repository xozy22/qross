// CPU und Arbeitsspeicher von Qross je Rechner: Die eigenen Werte fragt die Oberfläche ab, die eines
// Clients meldet er (Ereignis `usage`), beides nur, solange eine Anzeige davon zu sehen ist.

import { invoke } from "./core.js";
import { el, infoIcon } from "./util.js";
import { NBSP, decimal, formatPct } from "./quality.js";

/** Texte dieses Abschnitts, beisammen für eine spätere Übersetzung */
export const USAGE_TEXT = {
  here: "Qross hier",
  there: "Qross dort",
  cpu: "CPU",
  core: "eines Kerns",
  memory: "RAM",
  processes: (n) => (n === 1 ? "in 1 Prozess" : `in ${n} Prozessen`),
  measuring: "Wird gemessen …",
  silent: (name) => `„${name}“ meldet das nicht (Qross vor 0.14).`,
  help: [
    "Was Qross auf diesem Rechner braucht, einschließlich seiner Web-Oberfläche und aller Hilfsprozesse.",
    "CPU: Rechenzeit der letzten 2 Sekunden in Prozent eines Prozessorkerns (100 % = ein Kern voll ausgelastet).",
    "RAM: Arbeitsspeicher, den Qross für sich belegt, wie im Task-Manager bzw. in der Aktivitätsanzeige (unter Linux zählen geteilte Seiten anteilig).",
  ].join("\n\n"),
};

/** So oft fragt die Oberfläche die eigenen Werte ab, solange sie zu sehen sind */
export const USAGE_POLL_MS = 2000;

/** Prozent eines Kerns, z. B. „0,4 %“ */
export function formatCpu(percent) {
  return percent == null ? "–" : formatPct(percent);
}

/** Arbeitsspeicher wie im Task-Manager und in der Aktivitätsanzeige (1 MB = 1024 KB), z. B. „128 MB“ */
export function formatMemory(bytes) {
  const mb = bytes / 1048576;
  if (mb >= 1024) return `${decimal(mb / 1024, 1)}${NBSP}GB`;
  return `${decimal(mb, mb < 10 ? 1 : 0)}${NBSP}MB`;
}

/** Eine Zeile, z. B. „CPU 0,4 % eines Kerns · RAM 128 MB“; ohne Werte „Wird gemessen …“ */
export function usageLine(usage) {
  if (!usage) return USAGE_TEXT.measuring;
  return `${USAGE_TEXT.cpu} ${formatCpu(usage.cpu)} ${USAGE_TEXT.core} · ${USAGE_TEXT.memory} ${formatMemory(usage.memory)}`;
}

/** Zuletzt gemessen: hier und beim Client, dessen Werte der Server gerade zeigt */
let local = null;
let remote = null;
/** Client, dessen Werte der Server zeigen soll (undefined: dem Backend noch nichts gesagt) */
let watched;
let pollTimer = null;
let polling = false;
/** Anzeigen der Werte: { peer, node, refresh }; peer null: dieser Rechner */
const views = new Set();

function valueOf(peer) {
  if (peer == null) return local;
  return peer === watched ? remote : null;
}

/** Abschnitt „Qross dort“ bzw. „Qross hier“ im Panel einer Verbindung (`peer`: Client, null: dieser Rechner).
 *  `silent()` sagt, ob der Client das gar nicht meldet (ältere Version). */
export function usageBlock(peer, silent = () => false) {
  const block = {
    cpu: el("span", { class: "v" }),
    memory: el("span", { class: "v" }),
    memorySub: el("span", { class: "s" }),
    note: el("p", { class: "qusage-note" }),
  };
  block.stats = el(
    "div",
    { class: "qstats" },
    el("div", { class: "qstat" }, el("span", { class: "k", text: USAGE_TEXT.cpu }), block.cpu, el("span", { class: "s", text: USAGE_TEXT.core })),
    el("div", { class: "qstat" }, el("span", { class: "k", text: USAGE_TEXT.memory }), block.memory, block.memorySub),
  );
  const node = el(
    "div",
    { class: "qusage" },
    el("div", { class: "qusage-head" }, el("span", { text: peer == null ? USAGE_TEXT.here : USAGE_TEXT.there }), infoIcon(USAGE_TEXT.help)),
    block.stats,
    block.note,
  );
  const refresh = () => {
    const value = valueOf(peer);
    block.stats.hidden = !value;
    block.note.hidden = Boolean(value);
    block.note.textContent = peer != null && silent() ? USAGE_TEXT.silent(peer) : USAGE_TEXT.measuring;
    if (!value) return;
    block.cpu.textContent = formatCpu(value.cpu);
    block.memory.textContent = formatMemory(value.memory);
    block.memorySub.textContent = USAGE_TEXT.processes(value.processes);
  };
  return { peer, node, refresh };
}

/** Eine Zeile für Listen (dieser Rechner bzw. `peer`), die mit jeder Messung neu steht */
export function usageValue(peer = null) {
  const node = el("span", { class: "usage-line", title: USAGE_TEXT.help });
  const view = { peer, node, refresh: () => (node.textContent = usageLine(valueOf(peer))) };
  showUsage(view);
  return node;
}

/** Eine Anzeige ist (wieder) eingehängt: gleich mit dem neuesten Stand füllen und künftig mitführen. */
export function showUsage(view) {
  views.add(view);
  view.refresh();
}

function refreshAll() {
  for (const view of views) {
    if (view.node.isConnected) view.refresh();
    else views.delete(view);
  }
}

/** Anzeigen, die gerade zu sehen sind (offsetParent fehlt in versteckten Bereichen) */
function visibleViews() {
  if (document.visibilityState === "hidden") return [];
  return [...views].filter((view) => view.node.isConnected && view.node.offsetParent !== null);
}

/** Nach jedem Aufbau: Was zu sehen ist, wird gemessen, sonst nichts. */
export function syncUsage() {
  const visible = visibleViews();
  const peer = visible.find((view) => view.peer != null)?.peer ?? null;
  if (peer !== watched) {
    watched = peer;
    remote = null;
    invoke("watch_usage", { name: peer }).catch(() => {});
    refreshAll();
  }
  const poll = visible.some((view) => view.peer == null);
  if (poll && !pollTimer) {
    pollLocal();
    pollTimer = setInterval(pollLocal, USAGE_POLL_MS);
  } else if (!poll && pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function pollLocal() {
  if (polling) return;
  polling = true;
  try {
    local = await invoke("local_usage");
    refreshAll();
  } catch {
    // Beim nächsten Mal wieder
  } finally {
    polling = false;
  }
}

/** Meldung eines Clients (Ereignis `usage`); nur die des gerade gezeigten zählt */
export function onUsage(usage) {
  if (usage.peer == null || usage.peer !== watched) return;
  remote = usage;
  refreshAll();
}

/** Versteckt (Tray, minimiert) misst niemand; wieder sichtbar geht es gleich weiter. */
export function setupUsage() {
  document.addEventListener("visibilitychange", syncUsage);
}
