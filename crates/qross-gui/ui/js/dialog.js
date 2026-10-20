// Dialog

import { $, state } from "./core.js";
import { formatList, t, tn } from "./i18n.js";
import { el, running } from "./util.js";

/** Zeigt einen Dialog; liefert den Wert des gedrückten Knopfes (oder null). */
export function showDialog({ title, body, actions }) {
  const dialog = $("#dialog");
  $("#dialogTitle").textContent = title;
  $("#dialogBody").replaceChildren(...[body].flat());
  $("#dialogActions").replaceChildren(
    ...actions.map((a) => el("button", { class: `btn ${a.kind || ""}`, value: a.value, text: a.label })),
  );
  return new Promise((resolve) => {
    const done = () => {
      dialog.removeEventListener("close", done);
      state.dialogOpen = false;
      resolve(dialog.returnValue || null);
    };
    dialog.returnValue = "";
    dialog.addEventListener("close", done);
    state.dialogOpen = true;
    dialog.showModal();
    const input = dialog.querySelector("input");
    if (input) {
      input.focus();
      input.select();
    }
  });
}

export async function askName(title, initial = "", text = t("dialog.nameHint")) {
  const input = el("input", { type: "text", value: initial, spellcheck: "false", autocomplete: "off", "aria-label": t("dialog.name") });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      $("#dialog").close("ok");
    }
  });
  const result = await showDialog({
    title,
    body: [el("p", { text }), input],
    actions: [{ label: t("common.cancel"), value: "cancel", kind: "ghost" }, { label: t("dialog.apply"), value: "ok", kind: "primary" }],
  });
  const name = input.value.trim();
  return result === "ok" && name ? name : null;
}

/** „a“, „a“ und „b“, „a“, „b“ und „c“ */
export function joinNames(names) {
  return formatList(names.map((name) => t("dialog.quotedName", { name })));
}

/**
 * Fragt nach, bevor ein Wechsel der Rolle (Knopf oder Profil) den laufenden Server ausschaltet
 * oder die Verbindung des Clients trennt. Liefert true, wenn es weitergehen soll.
 */
export async function confirmRoleChange(role) {
  const s = state.status;
  if (!role || !running() || s.role === role) return true;
  if (s.role === "client" && s.phase !== "connected") return true;
  const toClient = s.role === "server";
  const names = toClient ? s.clients.map((c) => c.name) : [];
  const lose = names.length ? ` ${tn("dialog.clientsLose", names.length, { names: joinNames(names) })}` : "";
  const result = await showDialog({
    title: toClient ? t("dialog.stopServerTitle") : t("dialog.disconnectTitle"),
    body: toClient
      ? [
          el("p", { text: `${t("dialog.toClient")}${lose}` }),
          el("p", { text: t("dialog.toClientQuestion") }),
        ]
      : [
          el("p", { text: s.server_name ? t("dialog.toServerNamed", { server: s.server_name }) : t("dialog.toServer") }),
          el("p", { text: t("dialog.toServerQuestion") }),
        ],
    actions: [
      { label: t("common.cancel"), value: "cancel", kind: "ghost" },
      { label: toClient ? t("dialog.stopServer") : t("dialog.disconnect"), value: "ok", kind: "danger" },
    ],
  });
  return result === "ok";
}

/**
 * Gemeinsames Verhalten der Dialoge „Neuigkeiten“ und „Einstellungen“: Esc und Klick auf den
 * Hintergrund schließen, der Fokus geht beim Schließen an den Auslöser zurück.
 */
export function setupSheet(dialog, closeButton) {
  closeButton.addEventListener("click", () => dialog.close());
  // Der Inhalt füllt den Dialog aus: Ein Klick direkt auf das Dialog-Element ist der Hintergrund.
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
  // Esc schließt auch dann, wenn der native Abbruch des Dialogs nicht greift.
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      dialog.close();
    }
  });
  dialog.addEventListener("close", () => {
    dialog.returnFocus?.focus?.();
    dialog.returnFocus = null;
  });
}

export function openSheet(dialog, focusTarget) {
  if (dialog.open) return false;
  dialog.returnFocus = document.activeElement;
  dialog.showModal();
  focusTarget.focus();
  return true;
}
