// Rechte je Rechner

import { $, state } from "./core.js";
import { el } from "./util.js";
import { settingsChanged } from "./store.js";

/** Was sich je Rechner freigeben lässt; `key` wie in gui.json, `short` für Kurzfassungen */
export const SHARE_KINDS = [
  { key: "clipboard", label: "Zwischenablage", short: "Zwischenablage", hint: "Text, HTML, Bild" },
  { key: "files", label: "Kopierte Dateien", short: "Dateien", hint: "Kopieren, Einfügen" },
  { key: "drag", label: "Drag & Drop", short: "Drag & Drop", hint: "Dateien ziehen" },
];
/** Aus Sicht des jeweiligen Rechners: „to-client“ heißt, er empfängt nur. */
export const SHARE_OPTIONS = [
  ["both", "Senden und empfangen"],
  ["to-client", "Nur empfangen"],
  ["from-client", "Nur senden"],
  ["off", "Aus"],
];
export const SHARE_SHORT = { "to-client": "nur empfangen", "from-client": "nur senden", off: "aus" };
/** Eintrag für die Vorgabe (kein gültiger Rechnername) */
export const SHARE_DEFAULT = "";

/** Clients in der Anordnung, ohne den Server selbst */
export function sharingClients() {
  const server = state.settings.server;
  return server.screens.map((s) => s.name).filter((n) => n !== server.name);
}

/** Rechte eines Rechners; ohne eigenen Eintrag gilt die Vorgabe (so sieht es auch der Server) */
export function sharingOf(name) {
  const rules = state.settings.server.sharing;
  return (name !== SHARE_DEFAULT && rules.clients[name]) || rules.default;
}

/** Kurzfassung, z. B. „Zwischenablage nur empfangen · Drag & Drop aus“; `null`, wenn alles erlaubt ist */
export function sharingSummary(sharing) {
  const parts = SHARE_KINDS.filter((k) => sharing[k.key] !== "both").map((k) => `${k.short} ${SHARE_SHORT[sharing[k.key]]}`);
  return parts.length ? parts.join(" · ") : null;
}

export function renderSharing() {
  const list = $("#sharingList");
  const names = [...sharingClients(), SHARE_DEFAULT];
  // Nur neu aufbauen, wenn sich die Rechner ändern; sonst bleiben Auswahl und Fokus erhalten.
  const key = JSON.stringify(names);
  if (list.dataset.names !== key) {
    list.dataset.names = key;
    list.replaceChildren(...names.map(sharingEntry));
  }
  for (const details of list.querySelectorAll(".share-entry")) {
    const name = details.dataset.name;
    const sharing = sharingOf(name);
    for (const select of details.querySelectorAll("select")) {
      if (document.activeElement !== select) select.value = sharing[select.dataset.kind];
    }
    const limits = sharingSummary(sharing);
    details.classList.toggle("limited", limits !== null);
    details.querySelector(".share-sum").textContent = limits ?? "Alles erlaubt";
  }
}

export function sharingEntry(name) {
  const isDefault = name === SHARE_DEFAULT;
  const who = isDefault ? "neue Rechner" : name;
  const rows = SHARE_KINDS.map((kind) =>
    el(
      "label",
      { class: "share-row" },
      el("span", { class: "share-kind" }, el("span", { text: kind.label }), el("span", { class: "hint", text: kind.hint })),
      el(
        "select",
        { "data-kind": kind.key, "aria-label": `${kind.label} für ${who}`, onchange: (e) => setShare(name, kind.key, e.target.value) },
        ...SHARE_OPTIONS.map(([value, label]) => el("option", { value, text: label })),
      ),
    ),
  );
  const details = el(
    "details",
    {
      class: `share-entry${isDefault ? " default" : ""}`,
      "data-name": name,
      ontoggle: (e) => (e.target.open ? state.sharingOpen.add(name) : state.sharingOpen.delete(name)),
    },
    el(
      "summary",
      {},
      el("span", { class: "share-head" }, el("span", { class: "share-name", text: isDefault ? "Neue Rechner" : name }), el("span", { class: "share-sum" })),
    ),
    isDefault ? el("p", { class: "hint share-default-hint", text: "Vorgabe für Rechner, die neu in die Anordnung kommen. Eingetragene behalten ihre Rechte." }) : null,
    ...rows,
  );
  details.open = state.sharingOpen.has(name);
  return el("li", {}, details);
}

export function setShare(name, kind, value) {
  const rules = state.settings.server.sharing;
  if (name === SHARE_DEFAULT) {
    // Eingetragene Rechner behalten, was sie bisher hatten.
    for (const client of sharingClients()) rules.clients[client] ??= { ...rules.default };
    rules.default = { ...rules.default, [kind]: value };
  } else {
    rules.clients[name] = { ...sharingOf(name), [kind]: value };
  }
  settingsChanged({ immediate: true });
}

export function renameSharing(oldName, name) {
  const clients = state.settings.server.sharing.clients;
  if (clients[oldName]) clients[name] = clients[oldName];
  delete clients[oldName];
  if (state.sharingOpen.delete(oldName)) state.sharingOpen.add(name);
}

/** Vom Schild auf einer Karte: Eintrag aufklappen und zeigen */
export function openSharing(name) {
  state.sharingOpen.add(name);
  renderSharing();
  const details = $(`#sharingList .share-entry[data-name="${CSS.escape(name)}"]`);
  if (!details) return;
  details.open = true;
  details.scrollIntoView({ block: "nearest", behavior: "smooth" });
  details.querySelector("summary").focus({ preventScroll: true });
}

/** Client-Ansicht: was der Server mit diesem Rechner nicht oder nur in eine Richtung teilt */
export function renderClientSharing(sharing) {
  const note = $("#clientSharing");
  const limits = sharing && sharingSummary(sharing);
  note.hidden = !limits;
  if (limits) note.replaceChildren(el("strong", { text: "Vom Server eingeschränkt: " }), limits);
}
