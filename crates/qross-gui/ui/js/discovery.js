// Server-Suche im LAN

import { $, PLATFORMS, invoke, state } from "./core.js";
import { t, tn } from "./i18n.js";
import { el, fingerprintNode, running, toast } from "./util.js";
import { showDialog } from "./dialog.js";
import { saveSoon } from "./store.js";
import { renderClient } from "./client-view.js";
import { startFlow } from "./start.js";

export async function searchServers() {
  if (state.searching) return;
  state.searching = true;
  state.searchError = null;
  renderFinder();
  try {
    state.found = await invoke("discover_servers");
  } catch (e) {
    state.searchError = String(e);
    state.found = [];
  } finally {
    state.searching = false;
    // Auch die Karte des eingestellten Servers: Name und System kommen aus der Suche
    renderClient();
  }
}

export function renderFinder() {
  $(".finder").classList.toggle("searching", state.searching);
  $("#searchServers").disabled = state.searching;
  $("#searchServers span").textContent = state.searching ? t("discovery.searching") : t("client.searchAgain");
  const s = state.status;
  const connected = s.role === "client" && s.phase === "connected";
  $("#finderTitle").textContent = connected ? t("discovery.chooseOther") : t("discovery.title");
  const count = state.found?.length ?? 0;
  $("#finderCount").textContent = state.searching
    ? t("discovery.countSearching")
    : state.found == null
      ? ""
      : tn("discovery.found", count);
  const list = $("#serverList");
  const found = state.found;
  if (found == null || (state.searching && !found.length)) {
    list.replaceChildren(el("li", { class: "empty", text: t("discovery.searchingList") }));
    return;
  }
  if (state.searchError) {
    list.replaceChildren(el("li", { class: "empty", text: t("discovery.failed", { error: state.searchError }) }));
    return;
  }
  if (!found.length) {
    list.replaceChildren(el("li", { class: "empty", text: t("discovery.none") }));
    return;
  }
  const current = s.role === "client" ? state.settings.client.address.trim() : null;
  list.replaceChildren(
    ...found.map((srv) => {
      const os = PLATFORMS[srv.platform] || PLATFORMS.other;
      const details = [el("span", { text: `${srv.address} (${os.label})` })];
      if (srv.trust === "mismatch") {
        details.push(el("span", { class: "warn", text: t("discovery.fingerprintMismatch") }));
      } else if (srv.swaps_modifiers) {
        details.push(el("span", { class: "swap", text: t("discovery.swapsModifiers") }));
      }
      const isCurrent = current === srv.address;
      return el(
        "li",
        {},
        el(
          "button",
          {
            type: "button",
            class: `server-row${isCurrent ? " current" : ""}`,
            title: t("discovery.connectTo", { name: srv.name }),
            onclick: () => connectTo(srv),
          },
          el("span", { class: "os", text: os.short }),
          el("span", { class: "info" }, el("strong", { text: srv.name }), details),
          el("span", {
            class: "go",
            text: isCurrent ? (s.phase === "connected" ? t("discovery.connected") : t("discovery.connecting")) : t("discovery.connect"),
          }),
        ),
      );
    }),
  );
}

export async function connectTo(srv) {
  if (running()) {
    if (state.settings.client.address.trim() === srv.address) return;
    await invoke("stop");
  }
  state.settings.client.address = srv.address;
  $("#clientAddress").value = srv.address;
  renderClient();
  await startFlow();
}

export async function serverConfirmDialog(probe) {
  const mismatch = !!probe.expected;
  const body = [];
  if (mismatch) {
    body.push(
      el("div", { class: "warnbox" }, t("discovery.mismatchWarning")),
      el("div", { class: "fp-block" }, el("span", { class: "label", text: t("discovery.previouslySaved") }), fingerprintNode(probe.expected)),
    );
  } else {
    body.push(el("p", { text: t("discovery.firstContact", { address: probe.address, addr: probe.addr }) }));
  }
  body.push(
    el("div", { class: "fp-block" }, el("span", { class: "label", text: t("discovery.serverFingerprint") }), fingerprintNode(probe.fingerprint, true)),
  );
  const result = await showDialog({
    title: mismatch ? t("discovery.fingerprintChanged") : t("discovery.confirmTitle"),
    body,
    actions: [
      { label: t("common.cancel"), value: "cancel", kind: "ghost" },
      { label: mismatch ? t("discovery.acceptNew") : t("discovery.trustConnect"), value: "trust", kind: mismatch ? "danger" : "accept" },
    ],
  });
  if (result !== "trust") return false;
  await invoke("trust_server", { address: probe.address, fingerprint: probe.fingerprint });
  return true;
}

export async function checkServer() {
  await saveSoon.flush();
  const address = state.settings.client.address.trim();
  if (!address) {
    $("#finderBox").open = true;
    $("#clientAddress").focus();
    return toast(t("discovery.addressFirst"), "error");
  }
  const button = $("#checkServer");
  button.disabled = true;
  try {
    const probe = await invoke("probe_server", { address });
    if (probe.known) toast(t("discovery.fingerprintMatches"));
    else if (await serverConfirmDialog(probe)) toast(t("discovery.fingerprintSaved"));
  } catch (e) {
    toast(String(e), "error");
  } finally {
    button.disabled = false;
  }
}
