// Rechte je Rechner

import { $, state } from "./core.js";
import { t } from "./i18n.js";
import { el } from "./util.js";
import { settingsChanged } from "./store.js";

/** Was sich je Rechner freigeben lässt; `key` wie in gui.json, `short` für Kurzfassungen */
export const SHARE_KINDS = [
  { key: "clipboard", label: "sharing.clipboard", short: "sharing.clipboard", hint: "sharing.clipboardHint" },
  { key: "files", label: "sharing.files", short: "sharing.filesShort", hint: "sharing.filesHint" },
  { key: "drag", label: "sharing.drag", short: "sharing.drag", hint: "sharing.dragHint" },
];
/** Aus Sicht des jeweiligen Rechners: „to-client“ heißt, er empfängt nur. */
export const SHARE_OPTIONS = [
  ["both", "sharing.both"],
  ["to-client", "sharing.receiveOnly"],
  ["from-client", "sharing.sendOnly"],
  ["off", "common.off"],
];
export const SHARE_SHORT = { "to-client": "sharing.receiveOnlyShort", "from-client": "sharing.sendOnlyShort", off: "sharing.offShort" };
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
  const parts = SHARE_KINDS.filter((k) => sharing[k.key] !== "both").map((k) =>
    t("sharing.limit", { kind: t(k.short), mode: t(SHARE_SHORT[sharing[k.key]]) }),
  );
  return parts.length ? parts.join(" · ") : null;
}

/** Je Art eine Zeile mit Auswahl, für einen Rechner oder die Vorgabe (`SHARE_DEFAULT`) */
export function shareControls(name) {
  const rows = SHARE_KINDS.map((kind) => {
    const label = t(kind.label);
    return el(
      "label",
      { class: "share-row" },
      el("span", { class: "share-kind" }, el("span", { text: label }), el("span", { class: "hint", text: t(kind.hint) })),
      el(
        "select",
        {
          "data-kind": kind.key,
          "aria-label": name === SHARE_DEFAULT ? t("sharing.kindForNew", { kind: label }) : t("sharing.kindFor", { kind: label, name }),
          onchange: (e) => setShare(name, kind.key, e.target.value),
        },
        ...SHARE_OPTIONS.map(([value, key]) => el("option", { value, text: t(key) })),
      ),
    );
  });
  const box = el("div", { class: "share-rows", "data-name": name }, ...rows);
  syncShareControls(box);
  return box;
}

/** Auswahlfelder auf den aktuellen Stand bringen; ein gerade bedientes bleibt, wie es ist. */
export function syncShareControls(box) {
  const sharing = sharingOf(box.dataset.name);
  for (const select of box.querySelectorAll("select")) {
    if (document.activeElement !== select) select.value = sharing[select.dataset.kind];
  }
}

/** Vorgabe für neue Rechner in den Einstellungen (Bereich „Server“) */
export function renderShareDefault() {
  const slot = $("#shareDefault");
  if (!slot.firstElementChild) slot.replaceChildren(...shareControls(SHARE_DEFAULT).children);
  slot.dataset.name = SHARE_DEFAULT;
  syncShareControls(slot);
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
}

/** Client-Ansicht: was der Server mit diesem Rechner nicht oder nur in eine Richtung teilt */
export function renderClientSharing(sharing) {
  const note = $("#clientSharing");
  const limits = sharing && sharingSummary(sharing);
  note.hidden = !limits;
  if (limits) note.replaceChildren(el("strong", { text: `${t("sharing.restrictedByServer")} ` }), limits);
}
