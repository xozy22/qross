// Rechte (macOS)

import { $, invoke, state } from "./core.js";
import { el, toast } from "./util.js";

export const PERMISSIONS = {
  accessibility: {
    title: "Bedienungshilfen",
    text: "Damit Qross Maus und Tastatur steuern kann, als Client wie als Server.",
  },
  input_monitoring: {
    title: "Eingabeüberwachung",
    text: "Damit Qross als Server Maus und Tastatur erfassen kann. macOS bietet danach an, Qross neu zu öffnen.",
    serverOnly: true,
  },
};
export let permissionTimer = null;

export function missingPermissions() {
  const granted = state.permissions;
  if (!granted) return [];
  return Object.keys(PERMISSIONS).filter(
    (key) => !granted[key] && !(PERMISSIONS[key].serverOnly && state.settings.role !== "server"),
  );
}

export function renderPermissions() {
  const missing = missingPermissions();
  $("#permissions").hidden = !missing.length;
  $("#permList").replaceChildren(
    ...missing.map((key) =>
      el(
        "li",
        { class: "perm-item" },
        el("span", { class: "perm-text" }, el("strong", { text: PERMISSIONS[key].title }), el("span", { class: "hint", text: PERMISSIONS[key].text })),
        el("button", { type: "button", class: "btn small", text: "Freigeben …", onclick: () => openPermission(key) }),
      ),
    ),
  );
  // Die Freigabe wirkt ohne Neustart; solange etwas fehlt, regelmäßig nachsehen.
  if (missing.length && !permissionTimer) permissionTimer = setInterval(checkPermissions, 2000);
  if (!missing.length && permissionTimer) {
    clearInterval(permissionTimer);
    permissionTimer = null;
  }
}

export async function checkPermissions() {
  const wasMissing = missingPermissions().length > 0;
  try {
    state.permissions = await invoke("permissions");
  } catch {
    return;
  }
  renderPermissions();
  if (wasMissing && !missingPermissions().length) toast("macOS hat Qross freigegeben.", "success");
}

export async function openPermission(key) {
  try {
    await invoke("open_permission_settings", { permission: key });
  } catch (e) {
    toast(`Systemeinstellungen nicht geöffnet: ${e}`, "error");
  }
}
