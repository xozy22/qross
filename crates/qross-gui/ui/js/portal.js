// Wayland (Desktop-Portale)

import { $, $$, invoke, state } from "./core.js";
import { el, running, toast } from "./util.js";
import { t } from "./i18n.js";
import { startFlow } from "./start.js";

export let portalRequest = null;

/** Stand der Portale neu holen (höchstens eine Anfrage zugleich) */
export function refreshPortal() {
  if (state.desktop !== "wayland" || portalRequest) return;
  portalRequest = invoke("portal_state")
    .then((portal) => {
      state.portal = portal;
      renderPortal();
    })
    .catch(() => {})
    .finally(() => {
      portalRequest = null;
    });
}

/** Hinweis für die eingestellte Rolle: { title, text, retry, info } oder null */
export function portalHint() {
  const p = state.portal;
  if (state.desktop !== "wayland" || !p) return null;
  const server = state.settings.role === "server";
  const problem = server ? p.capture : p.injector;
  if (problem?.kind === "denied") {
    return {
      title: t("portal.deniedTitle"),
      text: server ? t("portal.deniedCapture") : t("portal.deniedControl"),
      retry: true,
    };
  }
  if (server && p.input_capture == null) {
    return {
      title: t("portal.noCaptureTitle"),
      text: t("portal.noCapture"),
    };
  }
  if (!server && !(p.remote_desktop >= 2)) {
    return {
      title: t("portal.limitedTitle"),
      text: t("portal.limited"),
    };
  }
  if (problem?.kind === "missing") return { title: t("portal.missingTitle"), text: problem.text };
  if (problem?.kind === "failed") {
    return { title: t("portal.failedTitle"), text: t("portal.failed", { error: problem.text }), retry: !running() };
  }
  const remembered = server ? p.remembered_capture : p.remembered_injector;
  const s = state.status;
  if (s?.phase === "starting" && s.role === state.settings.role && !remembered) {
    return {
      title: t("portal.confirmTitle"),
      text: server ? t("portal.confirmCapture") : t("portal.confirmControl"),
      info: true,
    };
  }
  return null;
}

export function renderPortal() {
  const hint = portalHint();
  const box = $("#portalHint");
  box.hidden = !hint;
  if (hint) {
    $("#portalTitle").textContent = hint.title;
    $("#portalText").textContent = hint.text;
    box.classList.toggle("info", !!hint.info);
    box.setAttribute("role", hint.info ? "status" : "alert");
    const retry = el("button", { type: "button", class: "btn small", text: t("portal.retry"), onclick: () => startFlow() });
    $("#portalActions").replaceChildren(...(hint.retry ? [retry] : []));
  }
  // Einstellungen → Info
  const p = state.portal;
  for (const node of $$("[data-wayland]")) node.hidden = !p;
  if (p) {
    const injector = !!p.remembered_injector;
    const capture = !!p.remembered_capture;
    $("#portalRemembered").textContent =
      injector && capture ? t("portal.rememberedBoth")
      : injector ? t("portal.rememberedInjector")
      : capture ? t("portal.rememberedCapture")
      : t("portal.rememberedNone");
    $("#portalForget").hidden = !injector && !capture;
  }
}

export async function forgetPortal() {
  try {
    await invoke("forget_portal_permissions");
    toast(t("portal.forgotten"), "success");
  } catch (e) {
    toast(t("portal.forgetFailed", { error: e }), "error");
  }
  refreshPortal();
}
