// Wayland (Desktop-Portale)

import { $, $$, invoke, state } from "./core.js";
import { el, running, toast } from "./util.js";
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
  const verb = server ? "erfassen" : "steuern";
  const problem = server ? p.capture : p.injector;
  if (problem?.kind === "denied") {
    return {
      title: "Wayland: Freigabe abgelehnt",
      text: `Ohne Freigabe kann Qross Maus und Tastatur nicht ${verb}. Starte erneut und erlaube den Zugriff im Dialog des Systems.`,
      retry: true,
    };
  }
  if (server && p.input_capture == null) {
    return {
      title: "Wayland: Dieser Desktop lässt das Erfassen nicht zu",
      text: "Es fehlt das Desktop-Portal „InputCapture“ (ab GNOME 45, KDE Plasma 6.1, Hyprland 0.56). Unter Sway und anderen wlroots-Desktops läuft der Server nur in einer X11-Sitzung.",
    };
  }
  if (!server && !(p.remote_desktop >= 2)) {
    return {
      title: "Wayland: Einspeisen nur eingeschränkt",
      text: "Es fehlt das Desktop-Portal „RemoteDesktop“ mit libei (ab GNOME 45, KDE Plasma 6.1). Qross speist dann über XWayland ein; das erreicht je nach Desktop nur X11-Programme.",
    };
  }
  if (problem?.kind === "missing") return { title: "Wayland: Desktop-Portal fehlt", text: problem.text };
  if (problem?.kind === "failed") {
    return { title: "Wayland: Problem mit dem Desktop-Portal", text: `${problem.text}. Qross versucht es weiter.`, retry: !running() };
  }
  const remembered = server ? p.remembered_capture : p.remembered_injector;
  const s = state.status;
  if (s?.phase === "starting" && s.role === state.settings.role && !remembered) {
    return {
      title: "Wayland: Bitte die Freigabe bestätigen",
      text: `Das System fragt gleich, ob Qross Maus und Tastatur ${verb} darf. Erlaube es im Dialog des Systems.`,
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
    const retry = el("button", { type: "button", class: "btn small", text: "Erneut versuchen", onclick: () => startFlow() });
    $("#portalActions").replaceChildren(...(hint.retry ? [retry] : []));
  }
  // Einstellungen → Info
  const p = state.portal;
  for (const node of $$("[data-wayland]")) node.hidden = !p;
  if (p) {
    const kept = [p.remembered_injector && "Einspeisen", p.remembered_capture && "Erfassen"].filter(Boolean);
    $("#portalRemembered").textContent = kept.length ? `gemerkt: ${kept.join(", ")}` : "keine gemerkt";
    $("#portalForget").hidden = !kept.length;
  }
}

export async function forgetPortal() {
  try {
    await invoke("forget_portal_permissions");
    toast("Beim nächsten Start fragt das System wieder nach der Freigabe.", "success");
  } catch (e) {
    toast(`Nicht gelöscht: ${e}`, "error");
  }
  refreshPortal();
}
