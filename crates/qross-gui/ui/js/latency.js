// Verzögerung der Eingabekette

import { state } from "./core.js";
import { el } from "./util.js";
import { formatNumber, t, tn } from "./i18n.js";
import { NBSP, qualityEntry } from "./quality.js";

/** Teile der Kette in der Reihenfolge, in der eine Eingabe sie durchläuft */
export const CHAIN_PARTS = [
  { key: "server", label: "latency.server", detail: "latency.serverDetail" },
  { key: "network", label: "latency.network", detail: "latency.networkDetail" },
  { key: "client", label: "latency.client", detail: "latency.clientDetail" },
];
export const CHAIN_KINDS = [
  { key: "mouse", label: "latency.mouse", none: "latency.mouseNone", what: "latency.mouseWhat" },
  { key: "keys", label: "latency.keys", none: "latency.keysNone", what: "latency.keysWhat" },
];
/** Absätze der Erklärung hinter dem „?“ */
export const CHAIN_HELP = ["latency.helpSpan", "latency.helpSamples", "latency.helpParts", "latency.helpClocks", "latency.helpExcluded"];

/** Mikrosekunden als Millisekunden, fein genug für Loopback (0,04) */
export function chainNumber(us) {
  const ms = us / 1000;
  if (ms < 0.01) return `<${NBSP}${formatNumber(0.01, 2)}`;
  return formatNumber(ms, ms < 1 ? 2 : ms < 10 ? 1 : 0);
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
      el("span", { class: "k", text: t(kind.label) }),
      el("div", { class: "line" }, row.value, row.p95),
      row.bar,
      el("div", { class: "qchain-parts" }, ...Object.values(row.parts)),
    );
    block.rows[kind.key] = row;
    return row.node;
  });
  block.list = el("div", { class: "qchain-rows" }, ...rows);
  const help = CHAIN_HELP.map((key) => t(key)).join("\n\n");
  block.node = el(
    "div",
    { class: "qchain" },
    el(
      "div",
      { class: "qchain-head" },
      el("span", { text: t("latency.title") }),
      el("span", { class: "qinfo", tabindex: "0", role: "img", "aria-label": help, title: help, text: "?" }),
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
  block.empty.textContent = block.peer ? t("latency.emptyPeer", { name: block.peer }) : t("latency.empty");
  const reports = !latency || latency.client_reports;
  block.note.hidden = !any || reports;
  block.note.textContent = t("latency.oldClient", { name: block.peer });
  const uncertain = any && latency.uncertainty_us != null;
  block.accuracy.hidden = !uncertain;
  block.accuracy.textContent = uncertain ? t("latency.accuracy", { uncertainty: chainMs(latency.uncertainty_us) }) : "";
  if (!any) return;
  for (const kind of CHAIN_KINDS) updateChainRow(block.rows[kind.key], kind, latency[kind.key], latency.uncertainty_us);
}

export function updateChainRow(row, kind, chain, uncertainty) {
  const full = Boolean(chain?.total);
  if (!chain) {
    row.value.textContent = "–";
    row.p95.textContent = t(kind.none);
  } else if (full) {
    row.value.textContent = chainMs(chain.total.p50_us);
    row.p95.textContent = t("latency.p95", { value: chainMs(chain.total.p95_us) });
  } else {
    row.value.textContent = chainMs(chain.server.p50_us);
    row.p95.textContent = t("latency.p95ServerOnly", { value: chainMs(chain.server.p95_us) });
  }
  row.bar.hidden = !chain;
  for (const part of CHAIN_PARTS) {
    const spread = chain?.[part.key];
    const seg = row.segs[part.key];
    seg.hidden = !spread;
    if (spread) seg.style.flexGrow = String(Math.max(spread.p50_us, 1));
    row.parts[part.key].hidden = !spread;
    row.parts[part.key].textContent = spread ? `${t(part.label)} ${chainNumber(spread.p50_us)}` : "";
  }
  if (!chain) {
    row.node.title = `${t(kind.label)}: ${t(kind.none)}`;
    row.node.setAttribute("aria-label", row.node.title);
    return;
  }
  const lines = [tn("latency.samples", full ? chain.reported : chain.samples, { kind: t(kind.label), what: t(kind.what) })];
  if (full) lines.push(t("latency.total", { median: chainMs(chain.total.p50_us), p95: chainMs(chain.total.p95_us) }));
  for (const part of CHAIN_PARTS) {
    const spread = chain[part.key];
    if (!spread) continue;
    const plusMinus = part.key === "network" && uncertainty != null ? ` (±${NBSP}${chainMs(uncertainty)})` : "";
    lines.push(`${t(part.label)}, ${t(part.detail)}: ${chainMs(spread.p50_us)} / ${chainMs(spread.p95_us)}${plusMinus}`);
  }
  if (!full) lines.push(t("latency.newerClient"));
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
