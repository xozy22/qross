// Server-Suche im LAN

import { $, PLATFORMS, invoke, state } from "./core.js";
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
  $("#searchServers span").textContent = state.searching ? "Suche läuft …" : "Erneut suchen";
  const s = state.status;
  const connected = s.role === "client" && s.phase === "connected";
  $("#finderTitle").textContent = connected ? "Anderen Server wählen" : "Server im Netzwerk";
  const count = state.found?.length ?? 0;
  $("#finderCount").textContent = state.searching
    ? "sucht …"
    : state.found == null
      ? ""
      : count === 1
        ? "1 gefunden"
        : `${count} gefunden`;
  const list = $("#serverList");
  const found = state.found;
  if (found == null || (state.searching && !found.length)) {
    list.replaceChildren(el("li", { class: "empty", text: "Suche nach Qross-Servern im Netzwerk …" }));
    return;
  }
  if (state.searchError) {
    list.replaceChildren(el("li", { class: "empty", text: `Die Suche ist fehlgeschlagen: ${state.searchError}` }));
    return;
  }
  if (!found.length) {
    list.replaceChildren(
      el("li", { class: "empty", text: "Kein Server gefunden. Läuft Qross auf dem anderen Rechner als Server? Sonst trag seine Adresse unten ein." }),
    );
    return;
  }
  const current = s.role === "client" ? state.settings.client.address.trim() : null;
  list.replaceChildren(
    ...found.map((srv) => {
      const os = PLATFORMS[srv.platform] || PLATFORMS.other;
      const details = [el("span", { text: `${srv.address} (${os.label})` })];
      if (srv.trust === "mismatch") {
        details.push(el("span", { class: "warn", text: "Fingerprint weicht vom gespeicherten ab" }));
      } else if (srv.swaps_modifiers) {
        details.push(el("span", { class: "swap", text: "Strg und Cmd werden automatisch getauscht" }));
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
            title: `Mit ${srv.name} verbinden`,
            onclick: () => connectTo(srv),
          },
          el("span", { class: "os", text: os.short }),
          el("span", { class: "info" }, el("strong", { text: srv.name }), details),
          el("span", { class: "go", text: isCurrent ? (s.phase === "connected" ? "Verbunden" : "Verbindet …") : "Verbinden" }),
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
      el("div", { class: "warnbox" }, "Der Server meldet einen anderen Fingerprint als beim letzten Mal. Das passiert, wenn Qross dort neu eingerichtet wurde, kann aber auch auf einen Angriff hindeuten."),
      el("div", { class: "fp-block" }, el("span", { class: "label", text: "Bisher gespeichert" }), fingerprintNode(probe.expected)),
    );
  } else {
    body.push(
      el("p", {
        text: `Erster Kontakt mit ${probe.address} (${probe.addr}). Vergleiche den Fingerprint mit der Anzeige in Qross auf dem Server: Klick dort in der Anordnung auf den Server oder öffne Einstellungen → Server.`,
      }),
    );
  }
  body.push(el("div", { class: "fp-block" }, el("span", { class: "label", text: "Fingerprint des Servers" }), fingerprintNode(probe.fingerprint, true)));
  const result = await showDialog({
    title: mismatch ? "Fingerprint des Servers hat sich geändert" : "Server bestätigen",
    body,
    actions: [
      { label: "Abbrechen", value: "cancel", kind: "ghost" },
      { label: mismatch ? "Neuen Fingerprint übernehmen" : "Vertrauen und verbinden", value: "trust", kind: mismatch ? "danger" : "accept" },
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
    return toast("Gib zuerst die Adresse des Servers ein.", "error");
  }
  const button = $("#checkServer");
  button.disabled = true;
  try {
    const probe = await invoke("probe_server", { address });
    if (probe.known) toast("Der Fingerprint stimmt mit dem gespeicherten überein.");
    else if (await serverConfirmDialog(probe)) toast("Fingerprint gespeichert");
  } catch (e) {
    toast(String(e), "error");
  } finally {
    button.disabled = false;
  }
}
