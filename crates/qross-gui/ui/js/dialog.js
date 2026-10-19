// Dialog

import { $, state } from "./core.js";
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

export async function askName(title, initial = "", text = "Verwende genau den Namen, mit dem sich der Rechner meldet. Das ist meist sein Rechnername.") {
  const input = el("input", { type: "text", value: initial, spellcheck: "false", autocomplete: "off", "aria-label": "Name" });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      $("#dialog").close("ok");
    }
  });
  const result = await showDialog({
    title,
    body: [el("p", { text }), input],
    actions: [{ label: "Abbrechen", value: "cancel", kind: "ghost" }, { label: "Übernehmen", value: "ok", kind: "primary" }],
  });
  const name = input.value.trim();
  return result === "ok" && name ? name : null;
}

/** „a“, „a“ und „b“, „a“, „b“ und „c“ */
export function joinNames(names) {
  const quoted = names.map((n) => `„${n}“`);
  return quoted.length > 1 ? `${quoted.slice(0, -1).join(", ")} und ${quoted.at(-1)}` : (quoted[0] ?? "");
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
  const lose = names.length
    ? ` ${joinNames(names)} ${names.length === 1 ? "verliert" : "verlieren"} dann Maus und Tastatur dieses Rechners.`
    : "";
  const server = s.server_name ? `„${s.server_name}“` : "dem Server";
  const result = await showDialog({
    title: toClient ? "Server ausschalten?" : "Verbindung trennen?",
    body: toClient
      ? [
          el("p", { text: `Achtung: Der Server schaltet sich aus, wenn dieser Rechner zum Client wird.${lose}` }),
          el("p", { text: "Willst du wirklich zum Client wechseln?" }),
        ]
      : [
          el("p", { text: `Achtung: Die Verbindung zu ${server} wird getrennt, wenn dieser Rechner zum Server wird.` }),
          el("p", { text: "Willst du wirklich zum Server wechseln?" }),
        ],
    actions: [
      { label: "Abbrechen", value: "cancel", kind: "ghost" },
      { label: toClient ? "Server ausschalten" : "Verbindung trennen", value: "ok", kind: "danger" },
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
