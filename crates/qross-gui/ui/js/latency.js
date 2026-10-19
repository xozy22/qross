// Verzögerung der Eingabekette

import { $, state } from "./core.js";
import { el } from "./util.js";
import { NBSP, decimal, qualityEntry } from "./quality.js";

/** Teile der Kette in der Reihenfolge, in der eine Eingabe sie durchläuft */
export const CHAIN_PARTS = [
  { key: "server", label: "Server", detail: "Erfassung bis Versand" },
  { key: "network", label: "Netz", detail: "Versand bis Empfang beim Client" },
  { key: "client", label: "Client", detail: "Empfang bis eingespeist" },
];
export const CHAIN_KINDS = [
  { key: "mouse", label: "Maus", none: "noch keine Mausbewegung", what: "Mausbewegungen, jede 32." },
  { key: "keys", label: "Tasten", none: "noch kein Tastendruck", what: "Tastendrücke und Klicks" },
];
export const CHAIN_HELP = [
  "Wie lange eine Eingabe braucht: vom Abholen aus der Erfassung am Server, bis das Einspeisen am Client erledigt ist. Groß: Median, klein: 95. Perzentil der letzten Minute.",
  "Gemessen an Stichproben (jede 32. Mausbewegung, jeder Tastendruck und Klick), ohne zusätzliche Daten auf dem Eingabeweg.",
  "Server: Erfassung bis Versand. Netz: bis der Client die Eingabe gelesen hat. Client: bis sie eingespeist ist. Der Balken zeigt die Anteile am Median.",
  "Server und Client messen mit ihrer eigenen Uhr. Den Netzanteil schätzt Qross wie NTP aus den Rückmeldungen des Clients: Er stimmt, wenn Hin- und Rückweg gleich lang dauern, und liegt sonst höchstens um die angegebene Unsicherheit daneben.",
  "Nicht enthalten: die Zeit im Betriebssystem vor dem Hook und nach dem Einspeisen bis zum Bildschirm.",
].join("\n\n");

/** Mikrosekunden als Millisekunden, fein genug für Loopback (0,04) */
export function chainNumber(us) {
  const ms = us / 1000;
  if (ms < 0.01) return `<${NBSP}0,01`;
  return decimal(ms, ms < 1 ? 2 : ms < 10 ? 1 : 0);
}

export function chainMs(us) {
  return `${chainNumber(us)}${NBSP}ms`;
}

/** Abschnitt „Verzögerung der Eingabe“ im aufgeklappten Panel einer Verbindung */
export function chainBlock(entry, peer) {
  const block = { peer, rows: {} };
  block.accuracy = el("p", { class: "qchain-note" });
  block.empty = el("p", { class: "qchain-empty" });
  block.note = el("p", { class: "qchain-note", hidden: true });
  const rows = CHAIN_KINDS.map((kind) => {
    const row = {
      value: el("span", { class: "v" }),
      p95: el("span", { class: "s" }),
      segs: Object.fromEntries(CHAIN_PARTS.map((p) => [p.key, el("i", { class: p.key })])),
      parts: Object.fromEntries(CHAIN_PARTS.map((p) => [p.key, el("span", { class: p.key })])),
    };
    row.bar = el("div", { class: "qchain-bar", "aria-hidden": "true" }, ...Object.values(row.segs));
    row.node = el(
      "div",
      { class: "qchain-row", tabindex: "0" },
      el("span", { class: "k", text: kind.label }),
      el("div", { class: "line" }, row.value, row.p95),
      row.bar,
      el("div", { class: "qchain-parts" }, ...Object.values(row.parts)),
    );
    block.rows[kind.key] = row;
    return row.node;
  });
  block.list = el("div", { class: "qchain-rows" }, ...rows);
  block.node = el(
    "div",
    { class: "qchain" },
    el(
      "div",
      { class: "qchain-head" },
      el("span", { text: "Verzögerung der Eingabe" }),
      el("span", { class: "qinfo", tabindex: "0", role: "img", "aria-label": CHAIN_HELP, title: CHAIN_HELP, text: "?" }),
    ),
    block.empty,
    block.list,
    block.accuracy,
    block.note,
  );
  entry.chain = block;
  return block.node;
}

export function updateLatency(entry) {
  const block = entry.chain;
  const latency = entry.latency;
  const any = latency && (latency.mouse || latency.keys);
  // Am Client erst, wenn der Server misst (ältere Server melden nichts); am Server immer.
  block.node.hidden = !latency && !block.peer;
  block.empty.hidden = Boolean(any);
  block.list.hidden = !any;
  block.empty.textContent = block.peer
    ? `Noch keine Eingaben gemessen. Sobald der Cursor auf „${block.peer}“ ist, misst Qross nebenbei mit.`
    : "Noch keine Eingaben gemessen.";
  const reports = !latency || latency.client_reports;
  block.note.hidden = !any || reports;
  block.note.textContent = `„${block.peer}“ meldet Netz- und Client-Anteil nicht (ältere Qross-Version); zu sehen ist nur der Server-Anteil.`;
  const uncertain = any && latency.uncertainty_us != null;
  block.accuracy.hidden = !uncertain;
  block.accuracy.textContent = uncertain ? `Netzanteil auf ±${NBSP}${chainMs(latency.uncertainty_us)} genau (Uhrenvergleich wie NTP)` : "";
  if (!any) return;
  for (const kind of CHAIN_KINDS) updateChainRow(block.rows[kind.key], kind, latency[kind.key], latency.uncertainty_us);
}

export function updateChainRow(row, kind, chain, uncertainty) {
  const full = Boolean(chain?.total);
  if (!chain) {
    row.value.textContent = "–";
    row.p95.textContent = kind.none;
  } else if (full) {
    row.value.textContent = chainMs(chain.total.p50_us);
    row.p95.textContent = `95${NBSP}%: ${chainMs(chain.total.p95_us)}`;
  } else {
    row.value.textContent = chainMs(chain.server.p50_us);
    row.p95.textContent = `95${NBSP}%: ${chainMs(chain.server.p95_us)}, nur Server`;
  }
  row.bar.hidden = !chain;
  for (const part of CHAIN_PARTS) {
    const spread = chain?.[part.key];
    const seg = row.segs[part.key];
    seg.hidden = !spread;
    if (spread) seg.style.flexGrow = String(Math.max(spread.p50_us, 1));
    row.parts[part.key].hidden = !spread;
    row.parts[part.key].textContent = spread ? `${part.label} ${chainNumber(spread.p50_us)}` : "";
  }
  if (!chain) {
    row.node.title = `${kind.label}: ${kind.none}`;
    row.node.setAttribute("aria-label", row.node.title);
    return;
  }
  const lines = [`${kind.label} (${kind.what}), letzte Minute: ${full ? chain.reported : chain.samples} Stichproben`];
  if (full) lines.push(`Gesamt: Median ${chainMs(chain.total.p50_us)}, 95${NBSP}% ${chainMs(chain.total.p95_us)}`);
  for (const part of CHAIN_PARTS) {
    const spread = chain[part.key];
    if (!spread) continue;
    const plusMinus = part.key === "network" && uncertainty != null ? ` (±${NBSP}${chainMs(uncertainty)})` : "";
    lines.push(`${part.label}, ${part.detail}: ${chainMs(spread.p50_us)} / ${chainMs(spread.p95_us)}${plusMinus}`);
  }
  if (!full) lines.push("Netz und Client: meldet erst ein neueres Qross auf dem Client");
  row.node.title = lines.join("\n");
  row.node.setAttribute("aria-label", lines.join(". "));
}

export function onLatency(latency) {
  const s = state.status;
  const peer = latency.peer;
  const connected =
    peer == null ? s.role === "client" && s.phase === "connected" : s.role === "server" && s.clients.some((c) => c.name === peer);
  if (!connected) return;
  const entry = qualityEntry(peer == null ? "server" : `client:${peer}`);
  entry.latency = latency;
  if (entry.panel.isConnected) updateLatency(entry);
}
