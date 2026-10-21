// Fenster mit Administratorrechten (Windows als Server)

import { $, state } from "./core.js";

/** Hinweis kommt fertig aus dem Backend, solange ein solches Fenster im Weg ist und der Server läuft. */
export function renderElevated() {
  const hint = state.status?.elevated;
  const box = $("#elevated");
  box.hidden = !hint;
  if (box.hidden) return;
  $("#elevatedTitle").textContent = hint.title;
  $("#elevatedText").textContent = hint.text;
}
