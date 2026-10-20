// Rechte (macOS)

import { $, invoke, state } from "./core.js";
import { el, toast } from "./util.js";
import { t } from "./i18n.js";

export const PERMISSIONS = {
  accessibility: {
    title: "permissions.accessibility",
    text: "permissions.accessibilityText",
  },
  input_monitoring: {
    title: "permissions.inputMonitoring",
    text: "permissions.inputMonitoringText",
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
        el("span", { class: "perm-text" }, el("strong", { text: t(PERMISSIONS[key].title) }), el("span", { class: "hint", text: t(PERMISSIONS[key].text) })),
        el("button", { type: "button", class: "btn small", text: t("perm.grant"), onclick: () => openPermission(key) }),
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
  if (wasMissing && !missingPermissions().length) toast(t("permissions.granted"), "success");
}

export async function openPermission(key) {
  try {
    await invoke("open_permission_settings", { permission: key });
  } catch (e) {
    toast(t("permissions.openFailed", { error: e }), "error");
  }
}
