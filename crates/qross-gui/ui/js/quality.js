// Verbindungsqualität

import { state } from "./core.js";
import { el, infoIcon } from "./util.js";
import { formatList, formatNumber, t, tn } from "./i18n.js";
import { chainBlock, updateLatency } from "./latency.js";

/** Länge des Verlaufs in Sekunden (ein Messpunkt pro Sekunde) */
export const QUALITY_SECONDS = 60;
/** Stufe → gefüllte Balken; die Farbe kommt per CSS über `data-level`. */
export const QUALITY_BARS = { very_good: 4, good: 3, fair: 2, poor: 1 };
/** Stufe → angezeigter Name; `label` aus dem Backend ist nur fürs Protokoll (englisch). */
export const QUALITY_LEVELS = { very_good: "quality.veryGood", good: "quality.good", fair: "quality.fair", poor: "quality.poor" };
/** Einzelwerte, die die Stufe bestimmen (Felder von `rating`) */
export const QUALITY_METRICS = { rtt: "quality.metricRtt", jitter: "quality.metricJitter", loss: "quality.metricLoss" };
export const QUALITY_LIMITS = "quality.limits";
/** Je Verbindung ("server" bzw. "client:Name"): Messpunkte der letzten Minute und die Anzeige dazu */
export const qualities = new Map();
export const SVG_NS = "http://www.w3.org/2000/svg";
export const NBSP = " ";
export const CHART = { w: 240, h: 48 };
export let qualityIds = 0;

export function svgEl(tag, attrs = {}) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
}

/** Name der Stufe eines Messpunkts in der Sprache der Oberfläche */
export function levelName(q) {
  return QUALITY_LEVELS[q.level] ? t(QUALITY_LEVELS[q.level]) : q.label;
}

export function formatMs(ms) {
  if (ms < 0.1) return `<${NBSP}${formatNumber(0.1, 1)}${NBSP}ms`;
  return `${formatNumber(ms, ms < 10 ? 1 : 0)}${NBSP}ms`;
}

/** Prozent zu einer schon formatierten Zahl: „45 %“ bzw. „45%“ */
export function percentText(number) {
  return t("quality.percent", { value: number });
}

export function formatPct(pct) {
  if (pct === 0) return percentText(0);
  if (pct < 0.1) return `<${NBSP}${percentText(formatNumber(0.1, 1))}`;
  return percentText(formatNumber(pct, pct < 10 ? 1 : 0));
}

/** Bytes pro Sekunde, in denselben Einheiten wie das Protokoll (1 KB = 1000 B) */
export function formatRate(bytes) {
  if (bytes < 1000) return `${bytes}${NBSP}B/s`;
  const units = ["KB/s", "MB/s", "GB/s"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${formatNumber(value, value < 10 ? 1 : 0)}${NBSP}${units[unit]}`;
}

/** Obergrenze der Skala (6, 8, 10, 15, 20, 30 … ms), mit etwas Luft über dem höchsten Wert */
export function chartMax(values) {
  const target = Math.max(5, ...values) * 1.15;
  const power = 10 ** Math.floor(Math.log10(target));
  return [1, 1.5, 2, 3, 4, 5, 6, 8, 10].map((m) => m * power).find((v) => v >= target);
}

/** Pfeil für gesendet (hoch) bzw. empfangen (runter); Unicode-Pfeile erscheinen je nach Schrift farbig. */
export function rateArrow(up) {
  const node = svgEl("svg", { class: "qarrow", viewBox: "0 0 12 12", "aria-hidden": "true" });
  node.append(svgEl("path", { d: up ? "M6 10.5V2M2.5 5.5L6 2l3.5 3.5" : "M6 1.5V10M2.5 6.5L6 10l3.5-3.5" }));
  return node;
}

export function qualityBars() {
  const node = svgEl("svg", { class: "qbars", viewBox: "0 0 16 14", "aria-hidden": "true" });
  for (let i = 0; i < 4; i++) node.append(svgEl("rect", { x: i * 4 + 0.5, y: 10 - i * 3, width: 3, height: 4 + i * 3, rx: 1 }));
  return node;
}

/** Anzeige einer Verbindung: kompakte Plakette (Stufe + ms) für Listen und das Panel mit den Details für
 *  den Detailbereich. Die Knoten bleiben bestehen und werden nur aktualisiert, damit nichts flackert; das
 *  Panel rechnet nur, solange es im Dokument steht. */
export function qualityEntry(key) {
  let entry = qualities.get(key);
  if (entry) return entry;
  const id = `quality-${++qualityIds}`;
  entry = {
    samples: [],
    /** Spalte (0–59) unter dem Mauszeiger im Verlauf */
    hover: null,
    max: 10,
    pillBars: qualityBars(),
    pillText: el("span", { class: "qms" }),
    headBars: qualityBars(),
    level: el("strong", { class: "qlevel", title: t(QUALITY_LIMITS) }),
    reason: el("span", { class: "qreason" }),
    scale: el("span", { class: "qscale" }),
    line: svgEl("path", { class: "qline" }),
    area: svgEl("path", { class: "qarea" }),
    cross: el("div", { class: "qcross", hidden: true }),
    dot: el("div", { class: "qdot", hidden: true }),
    tip: el("div", { class: "qtip", hidden: true }),
    values: {},
    /** Letztes Ergebnis der Messung der Eingabekette (Ereignis `latency`) */
    latency: null,
  };
  const plot = svgEl("svg", { viewBox: `0 0 ${CHART.w} ${CHART.h}`, preserveAspectRatio: "none", "aria-hidden": "true" });
  plot.append(
    svgEl("line", { class: "qgrid", x1: 0, y1: 0.5, x2: CHART.w, y2: 0.5 }),
    svgEl("line", { class: "qgrid base", x1: 0, y1: CHART.h - 0.5, x2: CHART.w, y2: CHART.h - 0.5 }),
    entry.area,
    entry.line,
  );
  entry.chart = el("div", { class: "qchart", role: "img" }, plot, entry.scale, entry.cross, entry.dot, entry.tip);
  entry.chart.addEventListener("pointermove", (e) => {
    const rect = entry.chart.getBoundingClientRect();
    const column = Math.round(((e.clientX - rect.left) / rect.width) * (QUALITY_SECONDS - 1));
    entry.hover = Math.max(0, Math.min(QUALITY_SECONDS - 1, column));
    showQualityHover(entry);
  });
  entry.chart.addEventListener("pointerleave", () => {
    entry.hover = null;
    showQualityHover(entry);
  });

  const stat = (name, label) => {
    entry.values[name] = el("span", { class: "v" });
    entry.values[`${name}Sub`] = el("span", { class: "s" });
    return el("div", { class: `qstat ${name}` }, el("span", { class: "k", text: label }), entry.values[name], entry.values[`${name}Sub`]);
  };
  entry.values.rate = el("span");
  entry.values.rateSub = el("span");
  const rate = el(
    "div",
    { class: "qstat rate" },
    el("span", { class: "k", text: t("quality.throughput") }),
    el("span", { class: "v", title: t("quality.sent") }, rateArrow(true), entry.values.rate),
    el("span", { class: "v", title: t("quality.received") }, rateArrow(false), entry.values.rateSub),
  );
  entry.panel = el(
    "div",
    { class: "qpanel", id },
    el(
      "div",
      { class: "qhead" },
      entry.headBars,
      entry.level,
      entry.reason,
      infoIcon(t("quality.info")),
    ),
    el("div", {}, entry.chart, el("div", { class: "qaxis" }, el("span", { text: t("quality.axis") }), el("span", { text: t("quality.now") }))),
    el("div", { class: "qstats" }, stat("rtt", t("quality.rtt")), stat("jitter", t("quality.jitter")), stat("loss", t("quality.loss")), rate),
    chainBlock(entry, key.startsWith("client:") ? key.slice("client:".length) : null),
  );
  entry.pill = el("span", { class: "qpill" }, entry.pillBars, entry.pillText);
  qualities.set(key, entry);
  updateQuality(entry);
  return entry;
}

export function updateQuality(entry) {
  const last = entry.samples.at(-1);
  const bars = last ? QUALITY_BARS[last.level] ?? 0 : 0;
  for (const node of [entry.pill, entry.panel]) node.dataset.level = last ? last.level : "none";
  for (const svg of [entry.pillBars, entry.headBars]) {
    [...svg.children].forEach((rect, i) => rect.classList.toggle("on", i < bars));
  }
  entry.pillText.textContent = last ? formatMs(last.rtt_ms) : t("quality.measuringShort");
  entry.pill.title = last
    ? t("quality.pillTitle", { level: levelName(last), latency: formatMs(last.rtt_ms) })
    : t("quality.measuringTitle");
  // Details nur, solange das Panel zu sehen ist; beim Einhängen ruft der Detailbereich hier wieder auf.
  if (!entry.panel.isConnected) return;

  const v = entry.values;
  entry.level.textContent = last ? levelName(last) : t("quality.measuring");
  // Welche Werte die Stufe bestimmen, z. B. „wegen Latenz und Verlust“
  const reasons =
    last && last.level !== "very_good"
      ? Object.keys(QUALITY_METRICS).filter((k) => last.rating?.[k] === last.level).map((k) => t(QUALITY_METRICS[k]))
      : [];
  entry.reason.textContent = reasons.length ? t("quality.because", { reasons: formatList(reasons) }) : "";
  v.rtt.textContent = last ? formatMs(last.rtt_ms) : "–";
  v.rttSub.textContent = t("quality.roundTrip");
  v.jitter.textContent = last ? formatMs(last.jitter_ms) : "–";
  v.jitterSub.textContent = t("quality.jitterWindow");
  v.loss.textContent = last ? formatPct(last.loss_pct) : "–";
  v.lossSub.textContent = last && last.loss_window_s ? t("quality.lossWindow", { seconds: last.loss_window_s }) : "";
  v.rate.textContent = last ? formatRate(last.tx_bytes_per_s) : "–";
  v.rateSub.textContent = last ? formatRate(last.rx_bytes_per_s) : "–";
  drawQualityChart(entry);
  updateLatency(entry);
}

export function qualityY(entry, ms) {
  return CHART.h - 1 - (Math.min(ms, entry.max) / entry.max) * (CHART.h - 4);
}

export function drawQualityChart(entry) {
  const values = entry.samples.map((q) => q.rtt_ms);
  const n = values.length;
  entry.max = chartMax(values);
  entry.scale.textContent = `${formatNumber(entry.max, 0)}${NBSP}ms`;
  if (!n) {
    entry.line.setAttribute("d", "");
    entry.area.setAttribute("d", "");
    entry.chart.setAttribute("aria-label", t("quality.noHistory"));
    return;
  }
  const step = CHART.w / (QUALITY_SECONDS - 1);
  const points = values.map((ms, i) => [(QUALITY_SECONDS - n + i) * step, qualityY(entry, ms)]);
  if (n === 1) points.unshift([points[0][0] - step, points[0][1]]);
  const d = points.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join("");
  entry.line.setAttribute("d", d);
  entry.area.setAttribute("d", `${d}L${points.at(-1)[0].toFixed(1)} ${CHART.h}L${points[0][0].toFixed(1)} ${CHART.h}Z`);
  entry.chart.setAttribute(
    "aria-label",
    tn("quality.chartLabel", n, { min: formatMs(Math.min(...values)), max: formatMs(Math.max(...values)) }),
  );
  showQualityHover(entry);
}

/** Fadenkreuz und Wert unter dem Mauszeiger */
export function showQualityHover(entry) {
  const n = entry.samples.length;
  const i = entry.hover == null ? -1 : entry.hover - (QUALITY_SECONDS - n);
  const hide = i < 0 || i >= n;
  entry.cross.hidden = entry.dot.hidden = entry.tip.hidden = hide;
  if (hide) return;
  const q = entry.samples[i];
  const left = (entry.hover / (QUALITY_SECONDS - 1)) * 100;
  entry.cross.style.left = `${left}%`;
  entry.dot.style.left = `${left}%`;
  entry.dot.style.top = `${(qualityY(entry, q.rtt_ms) / CHART.h) * 100}%`;
  const ago = n - 1 - i;
  const parts = [
    ago ? t("quality.ago", { seconds: ago }) : t("quality.now"),
    formatMs(q.rtt_ms),
    t("quality.tipJitter", { jitter: formatMs(q.jitter_ms) }),
  ];
  if (q.loss_pct > 0) parts.push(t("quality.tipLoss", { loss: formatPct(q.loss_pct) }));
  entry.tip.textContent = parts.join(" · ");
  // Innerhalb des Diagramms halten
  const width = entry.chart.clientWidth;
  const half = entry.tip.offsetWidth / 2;
  const x = Math.max(half, Math.min(width - half, (left / 100) * width));
  entry.tip.style.left = `${x}px`;
}

export function onQuality(q) {
  const s = state.status;
  const connected =
    q.peer == null ? s.role === "client" && s.phase === "connected" : s.role === "server" && s.clients.some((c) => c.name === q.peer);
  if (!connected) return;
  const entry = qualityEntry(q.peer == null ? "server" : `client:${q.peer}`);
  entry.samples.push(q);
  if (entry.samples.length > QUALITY_SECONDS) entry.samples.shift();
  updateQuality(entry);
}

/** Verläufe beendeter Verbindungen verwerfen; eine neue Verbindung beginnt von vorn. */
export function pruneQualities() {
  const s = state.status;
  const keep = new Set();
  if (s.role === "server") for (const c of s.clients) keep.add(`client:${c.name}`);
  if (s.role === "client" && s.phase === "connected") keep.add("server");
  for (const key of qualities.keys()) if (!keep.has(key)) qualities.delete(key);
}
