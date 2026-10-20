// Neuigkeiten

import { $, invoke, state } from "./core.js";
import { el, store, toast } from "./util.js";
import { locale, t } from "./i18n.js";
import { openSheet, setupSheet } from "./dialog.js";

export const NEWS_SEEN = "qross.newsSeen"; // zuletzt angesehene Version
export const NEWS_HINTED = "qross.newsHinted"; // Version, zu der der Update-Hinweis schon erschienen ist

/** Rubrik aus dem Changelog → Farbe des Tags (unbekannte Rubriken bleiben neutral). */
export const SECTION_TONES = {
  neu: "neu",
  new: "neu",
  added: "neu",
  verbessert: "verbessert",
  geändert: "verbessert",
  changed: "verbessert",
  improved: "verbessert",
  behoben: "behoben",
  fixed: "behoben",
  wichtig: "wichtig",
  important: "wichtig",
  sicherheit: "wichtig",
  security: "wichtig",
  entfernt: "wichtig",
  removed: "wichtig",
};

export function compareVersions(a, b) {
  const pa = String(a).split(".").map((n) => parseInt(n, 10) || 0);
  const pb = String(b).split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] || 0) - (pb[i] || 0);
    if (d) return d;
  }
  return 0;
}

/** „2026-10-18“ → „18. Oktober 2026“ bzw. „October 18, 2026“; anderes bleibt wie es ist. */
export function formatDate(iso) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso || "");
  if (!m) return iso || "";
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).toLocaleDateString(locale(), {
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}

/** Inline-Auszeichnung des Changelogs (**fett**, `code`) als DOM-Knoten – nie per innerHTML. */
export function inlineNodes(text) {
  const nodes = [];
  for (const part of text.split(/(`[^`]+`)/)) {
    if (part.length > 2 && part.startsWith("`") && part.endsWith("`")) {
      nodes.push(el("code", { text: part.slice(1, -1) }));
      continue;
    }
    for (const piece of part.split(/(\*\*[^*]+\*\*)/)) {
      if (piece.length > 4 && piece.startsWith("**") && piece.endsWith("**")) nodes.push(el("strong", { text: piece.slice(2, -2) }));
      else if (piece) nodes.push(document.createTextNode(piece));
    }
  }
  return nodes;
}

export function releaseNode(release) {
  const current = release.version === state.version;
  return el(
    "li",
    { class: `release${current ? " current" : ""}` },
    el("span", { class: "node", "aria-hidden": "true" }),
    el(
      "div",
      { class: "release-head" },
      el("h3", { text: release.version }),
      current ? el("span", { class: "installed", text: t("news.installed") }) : null,
      release.date ? el("time", { datetime: release.date, text: formatDate(release.date) }) : null,
    ),
    (release.intro || "").split(/\n{2,}/).filter(Boolean).map((p) => el("p", { class: "intro" }, inlineNodes(p))),
    release.sections.map((section) =>
      el(
        "section",
        { class: "group" },
        section.kind
          ? el("span", { class: `tag ${SECTION_TONES[section.kind.toLowerCase()] || "other"}`, text: section.kind })
          : null,
        el("ul", {}, section.items.map((item) => el("li", {}, inlineNodes(item)))),
      ),
    ),
  );
}

export async function openNews() {
  const dialog = $("#news");
  if (dialog.open) return;
  if (!state.releases) {
    try {
      state.releases = await invoke("changelog");
    } catch {
      state.releases = [];
    }
  }
  const list = $("#newsList");
  list.replaceChildren(
    ...(state.releases.length
      ? state.releases.map(releaseNode)
      : [el("li", { class: "news-empty", text: t("news.empty") })]),
  );
  openSheet(dialog, $("#newsClose"));
  list.scrollTop = 0;
  markNewsSeen();
}

export function markNewsSeen() {
  store(NEWS_SEEN, state.version);
  store(NEWS_HINTED, state.version);
  renderNewsChip(false);
}

export function renderNewsChip(unseen) {
  const chip = $("#newsChip");
  $("#versionText").textContent = `v${state.version}`;
  chip.classList.toggle("has-news", unseen);
  const version = state.version;
  chip.title = unseen ? t("news.chipNew", { version }) : t("news.show");
  chip.setAttribute("aria-label", t(unseen ? "news.chipLabelNew" : "news.chipLabel", { version }));
}

/** Nach einem Update: Punkt am Chip und einmal ein Hinweis. Beim allerersten Start nur die Version merken. */
export function checkForNews() {
  const seen = store(NEWS_SEEN);
  if (!seen) {
    store(NEWS_SEEN, state.version);
    store(NEWS_HINTED, state.version);
    renderNewsChip(false);
    return;
  }
  const unseen = compareVersions(state.version, seen) > 0;
  renderNewsChip(unseen);
  if (unseen && store(NEWS_HINTED) !== state.version) {
    store(NEWS_HINTED, state.version);
    toast(t("news.updated", { version: state.version }), "", { label: t("news.see"), run: openNews });
  }
}

export function setupNews() {
  $("#newsChip").addEventListener("click", openNews);
  setupSheet($("#news"), $("#newsClose"));
  checkForNews();
}
