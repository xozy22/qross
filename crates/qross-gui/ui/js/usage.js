// CPU und Arbeitsspeicher von Qross je Rechner: Die eigenen Werte fragt die Oberfläche ab, die eines
// Clients meldet er (Ereignis `usage`), beides nur, solange eine Anzeige davon zu sehen ist.

import { invoke } from "./core.js";
import { el, infoIcon } from "./util.js";
import { formatNumber, t, tn } from "./i18n.js";
import { NBSP, formatPct } from "./quality.js";

/** Texte dieses Abschnitts aus dem Wörterbuch (Bereich „usage.“); CPU und RAM heißen überall so */
export const USAGE_TEXT = {
  get here() {
    return t("usage.here");
  },
  get there() {
    return t("usage.there");
  },
  cpu: "CPU",
  get core() {
    return t("usage.core");
  },
  memory: "RAM",
  processes: (n) => tn("usage.processes", n),
  get measuring() {
    return t("usage.measuring");
  },
  silent: (name) => t("usage.silent", { name }),
  get help() {
    return t("usage.help");
  },
};

/** Arten der Prozesse in der Reihenfolge, in der sie kommen (`parts` im Ereignis `usage`): „core“ ist Qross
 *  selbst, alle anderen zusammen sind seine Oberfläche */
export const USAGE_KINDS = [
  { key: "core", label: "usage.kindCore" },
  { key: "page", label: "usage.kindPage" },
  { key: "graphics", label: "usage.kindGraphics" },
  { key: "browser", label: "usage.kindBrowser" },
  { key: "network", label: "usage.kindNetwork" },
  { key: "other", label: "usage.kindOther" },
];

/** So oft fragt die Oberfläche die eigenen Werte ab, solange sie zu sehen sind */
export const USAGE_POLL_MS = 2000;

/** Prozent eines Kerns, z. B. „0,4 %“ bzw. „0.4%“ */
export function formatCpu(percent) {
  return percent == null ? "–" : formatPct(percent);
}

/** Arbeitsspeicher wie im Task-Manager und in der Aktivitätsanzeige (1 MB = 1024 KB), z. B. „128 MB“ */
export function formatMemory(bytes) {
  const mb = bytes / 1048576;
  if (mb >= 1024) return `${formatNumber(mb / 1024, 1)}${NBSP}GB`;
  return `${formatNumber(mb, mb < 10 ? 1 : 0)}${NBSP}MB`;
}

/** Eine Zeile, z. B. „CPU 0,4 % eines Kerns · RAM 128 MB“; ohne Werte „Wird gemessen …“ */
export function usageLine(usage) {
  if (!usage) return USAGE_TEXT.measuring;
  return t("usage.line", { cpu: formatCpu(usage.cpu), memory: formatMemory(usage.memory) });
}

/** Arbeitsspeicher aufgeteilt, z. B. „Kern 8 MB · Oberfläche 95 MB“. Null, wenn der Rechner keine Teile meldet
 *  (Qross bis 0.14.0) oder es keine Oberfläche gibt (Client auf der Kommandozeile). */
export function usageSplit(usage) {
  const parts = usage?.parts ?? [];
  const core = parts.filter((part) => part.kind === "core");
  const ui = parts.filter((part) => part.kind !== "core");
  if (!ui.length) return null;
  const memory = (list) => formatMemory(list.reduce((sum, part) => sum + part.memory, 0));
  return t("usage.split", { core: memory(core), ui: memory(ui) });
}

/** Je Art eine Zeile, z. B. „Seite: CPU 0,2 % · RAM 30 MB“ */
export function usagePartLines(usage) {
  return (usage?.parts ?? []).map((part) => {
    const kind = USAGE_KINDS.find((k) => k.key === part.kind) ?? USAGE_KINDS.at(-1);
    return t("usage.partLine", { kind: t(kind.label), cpu: formatCpu(part.cpu), memory: formatMemory(part.memory) });
  });
}

/** Die Aufteilung unter CPU und RAM; der Tooltip nennt jede Art einzeln. */
function splitNode() {
  return el("span", { class: "usage-split", tabindex: "0", hidden: true });
}

function showSplit(node, usage) {
  const text = usageSplit(usage);
  node.hidden = !text;
  if (!text) return;
  const lines = usagePartLines(usage);
  node.textContent = text;
  node.title = lines.join("\n");
  node.setAttribute("aria-label", [text, ...lines].join(". "));
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
    split: splitNode(),
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
    block.split,
    block.note,
  );
  const refresh = () => {
    const value = valueOf(peer);
    block.stats.hidden = !value;
    block.note.hidden = Boolean(value);
    block.note.textContent = peer != null && silent() ? USAGE_TEXT.silent(peer) : USAGE_TEXT.measuring;
    showSplit(block.split, value);
    if (!value) return;
    block.cpu.textContent = formatCpu(value.cpu);
    block.memory.textContent = formatMemory(value.memory);
    block.memorySub.textContent = USAGE_TEXT.processes(value.processes);
  };
  return { peer, node, refresh };
}

/** Für Listen (dieser Rechner bzw. `peer`): eine Zeile mit CPU und RAM, darunter die Aufteilung; beides steht mit
 *  jeder Messung neu. */
export function usageValue(peer = null) {
  const line = el("span", { class: "usage-line", title: USAGE_TEXT.help });
  const split = splitNode();
  const node = el("span", { class: "usage-value" }, line, split);
  const refresh = () => {
    const value = valueOf(peer);
    line.textContent = usageLine(value);
    showSplit(split, value);
  };
  showUsage({ peer, node, refresh });
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
