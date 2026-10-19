// Clients und Freigaben

import { $, invoke, state } from "./core.js";
import { el, fingerprintNode, toast } from "./util.js";
import { showDialog } from "./dialog.js";
import { settingsChanged } from "./store.js";
import { addScreenNamed, monitorNote, multipleMonitors, screenTitle } from "./layout.js";
import { qualityEntry } from "./quality.js";

export function renderClients() {
  const s = state.status;
  const serverRunning = s.role === "server";
  const list = $("#clientList");
  const clients = serverRunning ? s.clients : [];
  if (!clients.length) {
    const text = serverRunning
      ? "Noch niemand verbunden. Starte Qross auf dem anderen Rechner als Client und trag dort die Adresse unten ein."
      : "Starte den Server, damit sich andere Rechner verbinden können.";
    list.replaceChildren(el("li", { class: "empty", text }));
  } else {
    // Die Qualitätsanzeigen werden wiederverwendet; ein Fokus darin bleibt erhalten.
    const focused = document.activeElement?.closest(".qpill, .qpanel") ? document.activeElement : null;
    list.replaceChildren(
      ...clients.map((c) => {
        const quality = qualityEntry(`client:${c.name}`);
        return el(
          "li",
          {},
          el("span", { class: "dot" }),
          el("div", {}, el("strong", { text: c.name }), quality.pill, s.cursor_on === c.name ? el("div", { class: "here", text: "Cursor ist hier" }) : null),
          el("span", { class: "meta", title: multipleMonitors(c) && screenTitle(c.name, c) }, `${c.width} × ${c.height}${monitorNote(c)}`, el("br"), c.addr),
          quality.panel,
        );
      }),
    );
    if (focused?.isConnected && document.activeElement !== focused) focused.focus({ preventScroll: true });
  }

  const pending = serverRunning ? s.pending : [];
  $("#pendingCard").hidden = pending.length === 0;
  $("#pendingList").replaceChildren(...pending.map(pendingNode));

  // Neue unbekannte Clients zusätzlich als Dialog melden (je Name und Fingerprint einmal).
  if (!state.dialogOpen) {
    const fresh = pending.find((p) => p.trust !== "known" && !state.prompted.has(p.name + p.fingerprint));
    if (fresh) {
      state.prompted.add(fresh.name + fresh.fingerprint);
      approvalDialog(fresh);
    }
  }
}

export function pendingText(p) {
  if (p.trust === "unknown") return "Vergleiche den Fingerprint mit der Anzeige auf diesem Rechner, bevor du ihn freigibst.";
  if (p.trust === "mismatch") return "Meldet sich mit einem anderen Fingerprint als bisher. Gib ihn nur frei, wenn Qross dort neu eingerichtet wurde.";
  if (!p.in_layout) return "Ist freigegeben, steht aber noch nicht in der Anordnung.";
  return `Abgewiesen: ${p.reason}`;
}

export function approveLabel(p) {
  if (p.trust === "known") return "In Anordnung aufnehmen";
  if (p.trust === "mismatch") return "Neuen Fingerprint freigeben";
  return p.in_layout ? "Freigeben" : "Freigeben und aufnehmen";
}

export function pendingNode(p) {
  return el(
    "div",
    { class: `pending ${p.trust}` },
    el("div", { class: "who" }, el("strong", { text: p.name }), el("span", { text: p.addr })),
    el("p", { class: "text", text: pendingText(p) }),
    p.trust === "known" ? null : fingerprintNode(p.fingerprint),
    el(
      "div",
      { class: "actions" },
      el("button", { class: `btn small ${p.trust === "mismatch" ? "danger" : "accept"}`, type: "button", text: approveLabel(p), onclick: () => approve(p) }),
      el("button", { class: "btn small ghost", type: "button", text: "Ignorieren", onclick: () => invoke("dismiss_client", { name: p.name }) }),
    ),
  );
}

export async function approvalDialog(p) {
  const body = [el("p", {}, `„${p.name}“ (${p.addr}) möchte Maus und Tastatur dieses Rechners mitbenutzen. ${pendingText(p)}`)];
  if (p.trust === "mismatch" && p.expected) {
    body.push(el("div", { class: "warnbox" }, "Bisher freigegeben:", fingerprintNode(p.expected)));
  }
  body.push(el("div", { class: "fp-block" }, el("span", { class: "label", text: "Fingerprint des Rechners" }), fingerprintNode(p.fingerprint, true)));
  const result = await showDialog({
    title: p.trust === "mismatch" ? `Fingerprint von „${p.name}“ hat sich geändert` : `„${p.name}“ möchte sich verbinden`,
    body,
    actions: [
      { label: "Später", value: "later", kind: "ghost" },
      { label: "Ignorieren", value: "ignore", kind: "ghost" },
      { label: approveLabel(p), value: "approve", kind: p.trust === "mismatch" ? "danger" : "accept" },
    ],
  });
  if (result === "approve") approve(p);
  else if (result === "ignore") invoke("dismiss_client", { name: p.name });
}

export async function approve(p) {
  try {
    if (p.trust !== "known") await invoke("approve_client", { name: p.name, fingerprint: p.fingerprint });
    if (!p.in_layout && addScreenNamed(p.name)) await settingsChanged({ immediate: true });
    if (p.trust === "known") invoke("dismiss_client", { name: p.name });
    toast(`„${p.name}“ freigegeben. Er verbindet sich beim nächsten Versuch.`);
  } catch (e) {
    toast(String(e), "error");
  }
}
