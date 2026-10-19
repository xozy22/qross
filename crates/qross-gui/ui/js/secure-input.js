// Sichere Tastatureingabe (macOS)

import { $, state } from "./core.js";
import { el } from "./util.js";

/** Hinweis kommt fertig aus dem Backend, solange er gilt (an und ein Client verbunden). */
export function renderSecureInput() {
  const hint = state.status?.secure_input;
  // Gleicher Platz wie der Rechte-Hinweis; ohne Rechte erfasst der Server ohnehin nichts.
  const box = $("#secureInput");
  box.hidden = !hint || !$("#permissions").hidden;
  if (box.hidden) return;
  $("#secureTitle").textContent = hint.title;
  $("#secureText").textContent = hint.text;
  $("#secureSteps").replaceChildren(...hint.steps.map((step) => el("li", { text: step })));
}
