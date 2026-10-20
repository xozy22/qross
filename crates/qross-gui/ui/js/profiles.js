// Profile je Ort

import { $, invoke, state } from "./core.js";
import { el, icon, toast } from "./util.js";
import { t, tn, tx } from "./i18n.js";
import { askName, confirmRoleChange, showDialog } from "./dialog.js";
import { saveSoon } from "./store.js";
import { startFlow } from "./start.js";
import { openSettings } from "./settings.js";

/** Abfrage der Netzwerke, solange die Einstellungen offen sind */
export const NETWORK_REFRESH_MS = 5000;
export let networkTimer = null;

/** Dieselbe Regel wie `NetworkMark::matches`: Kennen beide die MAC des Gateways, zählt nur sie. */
export function netMatches(stored, current) {
  if (!stored.gateway || !current) return false;
  if (stored.gateway_mac && current.gateway_mac) return stored.gateway_mac.toLowerCase() === current.gateway_mac.toLowerCase();
  const sameSubnet = !stored.subnet || !current.subnet || stored.subnet === current.subnet;
  return stored.gateway === current.gateway && sameSubnet;
}

export function activeProfile() {
  return state.profiles?.list.find((p) => p.id === state.profiles.active) ?? null;
}

/** „Server · 3 Bildschirme“ bzw. „Client · verbindet mit …“; beim aktiven Profil wie gerade eingestellt */
export function profileSummary(profile) {
  const place = profile.id === state.profiles.active ? state.settings : profile.place;
  if (place.role === "client") {
    const address = place.client.address.trim();
    return address ? t("profiles.clientTo", { address }) : t("profiles.clientNoAddress");
  }
  return tn("profiles.serverScreens", place.server.screens.length);
}

export function netDetails(n) {
  const mac = n.gateway_mac ? t("profiles.mac", { mac: n.gateway_mac }) : t("profiles.macUnknown");
  return [
    t("profiles.gateway", { gateway: n.gateway }),
    mac,
    n.subnet && t("profiles.subnet", { subnet: n.subnet }),
    n.interface && t("profiles.via", { interface: n.interface }),
  ]
    .filter(Boolean)
    .join(" · ");
}

/** Auswahl in der Kopfzeile */
export function renderProfilePick() {
  const profiles = state.profiles;
  if (!profiles) return;
  const select = $("#profileSelect");
  select.replaceChildren(
    ...profiles.list.map((p) => el("option", { value: p.id, text: p.name })),
    el("option", { value: "", disabled: true, text: "──────────" }),
    el("option", { value: "manage", text: t("profiles.manage") }),
  );
  select.value = profiles.active;
  const active = activeProfile();
  const auto = profiles.auto && profiles.list.some((p) => p.auto && p.networks.length);
  $("#profilePick").title = active
    ? t(auto ? "profiles.pickTitleAuto" : "profiles.pickTitle", { name: active.name, summary: profileSummary(active) })
    : t("topbar.profile");
}

/** Wechselt das Profil nach Rückfrage, falls es die Rolle wechselt, während Qross läuft. */
export async function pickProfile(id) {
  if (!id || id === state.profiles.active) return;
  const role = state.profiles.list.find((p) => p.id === id)?.place.role;
  if (await confirmRoleChange(role)) await activateProfile(id);
}

/** Wechselt das Profil; als Client fragt danach womöglich der Fingerprint-Dialog. */
export async function activateProfile(id) {
  await saveSoon.flush();
  try {
    const probe = await invoke("activate_profile", { id });
    if (probe) await startFlow();
  } catch (e) {
    toast(String(e), "error");
  }
}

export async function editProfiles(edit) {
  try {
    const result = await invoke("edit_profiles", { edit });
    state.profiles = result.profiles;
    if (result.note) toast(result.note);
    renderProfilePick();
    renderProfiles();
    return true;
  } catch (e) {
    toast(String(e), "error");
    renderProfiles();
    return false;
  }
}

export async function refreshNetworks() {
  try {
    state.networks = await invoke("current_networks");
  } catch {
    state.networks = [];
  }
  renderProfiles();
}

export function watchNetworks(on) {
  clearInterval(networkTimer);
  networkTimer = on ? setInterval(refreshNetworks, NETWORK_REFRESH_MS) : null;
  if (on) refreshNetworks();
}

export function renderNetNow() {
  const box = $("#netNow");
  const nets = state.networks;
  if (nets == null) {
    box.replaceChildren(el("span", { class: "hint", text: t("profiles.netDetecting") }));
    return;
  }
  if (!nets.length) {
    box.replaceChildren(icon("wifi"), el("span", { class: "net-text", text: t("profiles.netNone") }));
    return;
  }
  const [first, ...others] = nets;
  const owner = state.profiles.list.find((p) => p.networks.some((n) => netMatches(n, first)));
  box.replaceChildren(
    icon("wifi"),
    el(
      "span",
      { class: "net-text" },
      el("span", { class: "net-title" }, ...tx("profiles.netCurrent", { network: el("strong", { text: first.label }) })),
      el("span", { class: "hint", text: netDetails(first) }),
      el("span", { class: "hint", text: owner ? t("profiles.netOwner", { name: owner.name }) : t("profiles.netNoOwner") }),
      others.length
        ? el("span", { class: "hint", text: t("profiles.netOthers", { networks: others.map((n) => `${n.label} (${n.interface})`).join(", ") }) })
        : null,
    ),
  );
}

export function profileCard(profile) {
  const profiles = state.profiles;
  const active = profile.id === profiles.active;
  const current = state.networks?.[0];
  const hasCurrent = profile.networks.some((n) => netMatches(n, current));
  const name = profile.name;
  const action = (label, iconName, onclick, attrs = {}) =>
    el("button", { type: "button", class: "icon-btn small", title: label, "aria-label": `${label}: ${name}`, onclick, ...attrs }, icon(iconName));

  const chips = profile.networks.map((n, index) =>
    el(
      "span",
      { class: `chip net-chip${netMatches(n, current) ? " here" : ""}`, title: netDetails(n) },
      icon("wifi"),
      el("span", { class: "net-label", text: n.label }),
      el(
        "button",
        {
          type: "button",
          title: t("profiles.unassign"),
          "aria-label": t("profiles.unassignLabel", { network: n.label, name }),
          onclick: () => editProfiles({ action: "remove_network", id: profile.id, index }),
        },
        icon("x"),
      ),
    ),
  );
  const take = el(
    "button",
    {
      type: "button",
      class: "btn small ghost",
      disabled: !current,
      title: current ? `${current.label}: ${netDetails(current)}` : t("profiles.noNetwork"),
      onclick: () => editProfiles({ action: "add_network", id: profile.id, network: current }),
    },
    icon("plus"),
    t("profiles.addNetwork"),
  );

  const auto = el("input", {
    type: "checkbox",
    role: "switch",
    onchange: (e) => editProfiles({ action: "set_auto", id: profile.id, auto: e.target.checked }),
  });
  auto.checked = profile.auto;

  return el(
    "li",
    { class: `profile-card${active ? " active" : ""}`, "data-id": profile.id },
    el(
      "div",
      { class: "profile-head" },
      el(
        "span",
        { class: "profile-title" },
        el("span", { class: "profile-name", text: name }),
        active ? el("span", { class: "profile-badge", text: t("profiles.active") }) : null,
        profiles.fallback === profile.id
          ? el("span", { class: "profile-badge other", text: t("profiles.otherNetworks"), title: t("profiles.otherNetworksHint") })
          : null,
      ),
      el(
        "span",
        { class: "profile-actions" },
        active ? null : el("button", { type: "button", class: "btn small", text: t("profiles.activate"), onclick: () => activateProfile(profile.id) }),
        action(t("common.rename"), "pencil", () => renameProfile(profile)),
        action(t("profiles.duplicate"), "copy", () => editProfiles({ action: "duplicate", id: profile.id })),
        action(t("profiles.delete"), "trash", () => removeProfile(profile), {
          disabled: active,
          title: active ? t("profiles.deleteActive") : t("profiles.delete"),
        }),
      ),
      el("span", { class: "profile-summary", text: profileSummary(profile) }),
    ),
    el(
      "div",
      { class: "profile-nets" },
      chips.length ? chips : el("span", { class: "hint", text: t("profiles.noNetworks") }),
      hasCurrent ? null : take,
    ),
    profile.networks.length
      ? el(
          "label",
          { class: "switch profile-auto" },
          auto,
          el("span", { class: "track" }, el("span", { class: "thumb" })),
          el("span", { text: t("profiles.autoActivate") }),
        )
      : null,
  );
}

export function renderProfiles() {
  const profiles = state.profiles;
  if (!profiles || !$("#settings").open) return;
  $("#profilesAuto").checked = profiles.auto;
  renderNetNow();
  // Fokus nach dem Neuaufbau an dieselbe Stelle zurück (z. B. nach dem Umschalten der Automatik)
  const focused = document.activeElement?.closest?.(".profile-card");
  const focusId = focused?.dataset.id;
  const focusIndex = focused ? [...focused.querySelectorAll("button, input")].indexOf(document.activeElement) : -1;
  $("#profileList").replaceChildren(...profiles.list.map(profileCard));
  if (focusId) {
    const card = $(`#profileList [data-id="${CSS.escape(focusId)}"]`);
    const target = card && [...card.querySelectorAll("button, input")][focusIndex];
    (target && !target.disabled ? target : card?.querySelector("button"))?.focus();
  }
  const fallback = $("#profileFallback");
  fallback.replaceChildren(
    el("option", { value: "", text: t("profiles.keepCurrent") }),
    ...profiles.list.map((p) => el("option", { value: p.id, text: t("profiles.activateNamed", { name: p.name }) })),
  );
  fallback.value = profiles.fallback;
  $("#netHint").hidden = state.platform !== "macos";
}

export async function newProfile() {
  const name = await askName(t("settings.profiles.new"), "", t("profiles.newHint"));
  if (name) await editProfiles({ action: "create", name });
}

export async function renameProfile(profile) {
  const name = await askName(t("profiles.renameTitle"), profile.name, t("profiles.nameHint"));
  if (name && name !== profile.name) await editProfiles({ action: "rename", id: profile.id, name });
}

export async function removeProfile(profile) {
  const result = await showDialog({
    title: t("profiles.deleteTitle", { name: profile.name }),
    body: el("p", { text: t("profiles.deleteBody") }),
    actions: [
      { label: t("common.cancel"), value: "cancel", kind: "ghost" },
      { label: t("profiles.delete"), value: "remove", kind: "danger" },
    ],
  });
  if (result === "remove") await editProfiles({ action: "remove", id: profile.id });
}

export function setupProfiles() {
  $("#profileSelect").addEventListener("change", (e) => {
    const id = e.target.value;
    e.target.value = state.profiles.active;
    if (id === "manage") openSettings("set-profiles");
    else pickProfile(id);
  });
  $("#profilesAuto").addEventListener("change", (e) => editProfiles({ action: "set_automatic", enabled: e.target.checked }));
  $("#profileFallback").addEventListener("change", (e) => editProfiles({ action: "set_fallback", id: e.target.value }));
  $("#profileNew").addEventListener("click", newProfile);
  $("#settings").addEventListener("close", () => watchNetworks(false));
}
