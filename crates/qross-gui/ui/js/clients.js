// Seitenleiste des Servers: Anfragen, Rechner der Anordnung und Details des ausgewählten Rechners

import { $, invoke, state } from "./core.js";
import { el, fingerprintNode, icon, toast } from "./util.js";
import { showDialog } from "./dialog.js";
import { settingsChanged } from "./store.js";
import { addScreenNamed, markSelected, monitorNote, multipleMonitors, removeScreen, renameScreen, selectedName } from "./layout.js";
import { qualityEntry, updateQuality } from "./quality.js";
import { shareControls, sharingOf, sharingSummary, syncShareControls } from "./sharing.js";
import { openSettings } from "./settings.js";

/** Reiter der Details eines Clients */
export const INSPECTOR_TABS = [
  ["connection", "Verbindung"],
  ["rights", "Rechte"],
  ["info", "Info"],
];

export function renderClients() {
  renderRequests();
  renderComputerList();
  renderInspector();
}

/** Zeigt die Details eines Rechners, wahlweise gleich in einem bestimmten Reiter. */
export function selectComputer(name, tab) {
  state.selected = name;
  state.selectedByUser = true;
  if (tab) state.inspectorTab = tab;
  markSelected();
  renderClients();
}

// ------------------------------------------------------------ Anfragen

function requestNode(p) {
  const what =
    p.trust === "unknown"
      ? "möchte sich verbinden"
      : p.trust === "mismatch"
        ? "meldet einen anderen Fingerprint"
        : p.in_layout
          ? `abgewiesen: ${p.reason}`
          : "ist freigegeben, fehlt aber in der Anordnung";
  // Schon freigegeben, nur nicht in der Anordnung: gleich aufnehmen; sonst erst den Fingerprint vergleichen
  const direct = p.trust === "known" && !p.in_layout;
  return el(
    "div",
    { class: `request ${p.trust}`, "data-name": p.name },
    icon("ask"),
    el("span", { class: "what", title: `${p.name} (${p.addr}) ${what}` }, el("strong", { text: p.name }), el("span", { text: what })),
    el("button", {
      type: "button",
      class: "btn small",
      text: direct ? "Aufnehmen" : "Prüfen",
      title: direct ? approveLabel(p) : "Fingerprint vergleichen und freigeben",
      onclick: () => (direct ? approve(p) : approvalDialog(p)),
    }),
    el(
      "button",
      { type: "button", class: "icon-btn small", title: "Ignorieren", "aria-label": `Anfrage von ${p.name} ignorieren`, onclick: () => invoke("dismiss_client", { name: p.name }) },
      icon("x"),
    ),
  );
}

function renderRequests() {
  const s = state.status;
  const pending = s.role === "server" ? s.pending : [];
  $("#pendingList").replaceChildren(...pending.map(requestNode));

  // Neue unbekannte Clients einmal als Dialog melden (je Name und Fingerprint); die Zeile bleibt als Erinnerung.
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

// ------------------------------------------------------------ Rechnerliste

function renderComputerList() {
  const server = state.settings.server;
  const s = state.status;
  const running = s.role === "server";
  const online = new Map(running ? s.clients.map((c) => [c.name, c]) : []);
  const names = server.screens.map((x) => x.name).filter((n) => n !== server.name);
  names.sort((a, b) => Number(online.has(b)) - Number(online.has(a)) || a.localeCompare(b, "de"));
  const list = $("#computerList");
  if (!names.length) {
    list.replaceChildren(el("li", { class: "empty", text: "Noch kein anderer Rechner. Nimm einen mit „Bildschirm“ auf oder starte Qross dort als Client." }));
    return;
  }
  const selected = selectedName();
  const focused = document.activeElement?.closest?.(".computer")?.dataset.name;
  list.replaceChildren(
    ...names.map((name) => {
      const client = online.get(name);
      const cursorHere = running && s.cursor_on === name;
      const held = cursorHere && (s.locked || s.game_hold);
      const limits = sharingSummary(sharingOf(name));
      return el(
        "li",
        {},
        el(
          "button",
          { type: "button", class: `computer${client ? " online" : ""}`, "data-name": name, "aria-current": String(name === selected), onclick: () => selectComputer(name) },
          el("span", { class: "dot" }),
          el("span", { class: "name", text: name, title: name }),
          cursorHere
            ? el(
                "span",
                { class: `flag${held ? " held" : ""}`, title: s.locked ? "Cursor hier gesperrt" : held ? "Spielemodus hält den Cursor hier" : "Cursor ist hier" },
                icon(s.locked ? "lock" : held ? "game" : "cursor"),
              )
            : null,
          limits ? el("span", { class: "flag limited", title: `Rechte eingeschränkt: ${limits}` }, icon("shield")) : null,
          client ? qualityEntry(`client:${name}`).pill : el("span", { class: "off", text: running ? "nicht verbunden" : "" }),
        ),
      );
    }),
  );
  if (focused) list.querySelector(`.computer[data-name="${CSS.escape(focused)}"]`)?.focus({ preventScroll: true });
}

// ------------------------------------------------------------ Details

/** Was den Aufbau der Details bestimmt; ändert es sich, wird neu aufgebaut, sonst nur aktualisiert. */
let inspectorKey = "";

function renderInspector() {
  const box = $("#inspector");
  const server = state.settings.server;
  const s = state.status;
  const running = s.role === "server";
  const name = selectedName();
  if (!name) {
    inspectorKey = "";
    box.replaceChildren();
    return;
  }
  const isServer = name === server.name;
  const client = running && !isServer ? s.clients.find((c) => c.name === name) : null;
  const tab = isServer ? "info" : state.inspectorTab;
  const key = JSON.stringify([name, isServer, Boolean(client), tab, running]);

  if (key !== inspectorKey || !box.firstElementChild) {
    inspectorKey = key;
    const tabFocused = Boolean(document.activeElement?.closest?.("#inspector .tabs"));
    const body = el("div", { class: "insp-body", role: isServer ? null : "tabpanel", "aria-label": isServer ? null : tabLabel(tab) });
    const head = el("div", { class: "insp-head" }, el("strong", { text: name, title: name }), el("span", { class: "insp-sub" }));
    box.replaceChildren(...(isServer ? [head, body] : [head, tabsNode(tab), body]));
    fillBody(body, name, tab, client, isServer);
    if (tabFocused) box.querySelector('.tabs [aria-selected="true"]')?.focus();
  } else {
    const body = box.querySelector(".insp-body");
    const rights = body.querySelector(".share-rows");
    if (rights) syncShareControls(rights);
    // Info: Liste neu, Knöpfe darunter bleiben (samt Fokus)
    const list = body.querySelector(".insp-list");
    if (list) list.replaceWith(infoList(name, client, isServer));
  }
  box.querySelector(".insp-sub").textContent = subtitle(name, client, isServer, running);
}

function tabLabel(tab) {
  return INSPECTOR_TABS.find(([id]) => id === tab)?.[1] ?? "";
}

function tabsNode(active) {
  const select = (id) => {
    state.inspectorTab = id;
    renderInspector();
  };
  const onKey = (e) => {
    const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const ids = INSPECTOR_TABS.map(([id]) => id);
    select(ids[(ids.indexOf(state.inspectorTab) + step + ids.length) % ids.length]);
    $('#inspector .tabs [aria-selected="true"]')?.focus();
  };
  return el(
    "div",
    { class: "tabs", role: "tablist", "aria-label": "Details" },
    ...INSPECTOR_TABS.map(([id, label]) =>
      el("button", {
        type: "button",
        role: "tab",
        "aria-selected": String(id === active),
        tabindex: id === active ? "0" : "-1",
        text: label,
        onclick: () => select(id),
        onkeydown: onKey,
      }),
    ),
  );
}

function subtitle(name, client, isServer, running) {
  if (isServer) return running ? "Dieser Rechner · Server läuft" : "Dieser Rechner · Server gestoppt";
  if (!running) return "Server gestoppt";
  if (!client) return "nicht verbunden";
  const cursor = state.status.cursor_on === name ? " · Cursor ist hier" : "";
  return `verbunden${monitorNote(client)}${cursor}`;
}

function fillBody(body, name, tab, client, isServer) {
  const running = state.status.role === "server";
  if (isServer) {
    body.append(
      infoList(name, null, true),
      el(
        "div",
        { class: "insp-actions" },
        el("button", { type: "button", class: "btn small ghost", onclick: () => renameScreen(name) }, icon("pencil"), "Umbenennen"),
        el("button", { type: "button", class: "btn small ghost", onclick: () => openSettings("set-server") }, icon("gear"), "Server-Einstellungen"),
      ),
    );
    return;
  }
  if (tab === "connection") {
    if (!client) {
      const text = running
        ? `„${name}“ ist nicht verbunden. Sobald Qross dort als Client mit der Adresse unten läuft, stehen hier Latenz, Verlauf und die Verzögerung der Eingabe.`
        : "Der Server ist gestoppt. Nach dem Start stehen hier für jeden verbundenen Rechner Latenz, Verlauf und die Verzögerung der Eingabe.";
      body.append(el("p", { class: "insp-note", text }));
      return;
    }
    const entry = qualityEntry(`client:${name}`);
    body.append(entry.panel);
    updateQuality(entry);
    return;
  }
  if (tab === "rights") {
    body.append(
      shareControls(name),
      el("p", { class: "hint", text: "Gilt sofort, auch bei bestehender Verbindung. Was ein Rechner sendet, erreicht über den Server auch die anderen." }),
    );
    return;
  }
  body.append(
    infoList(name, client, false),
    el(
      "div",
      { class: "insp-actions" },
      el("button", { type: "button", class: "btn small ghost", onclick: () => renameScreen(name) }, icon("pencil"), "Umbenennen"),
      el("button", { type: "button", class: "btn small ghost danger", onclick: () => removeScreen(name) }, icon("trash"), "Entfernen"),
    ),
  );
}

/** Monitore von links nach rechts, z. B. „2560 × 1440, 1920 × 1080“ */
function monitorsText(monitors) {
  return [...monitors]
    .sort((a, b) => a.x - b.x || a.y - b.y)
    .map((m) => `${m.width} × ${m.height}`)
    .join(", ");
}

function infoList(name, client, isServer) {
  const s = state.status;
  const running = s.role === "server";
  const rows = [];
  if (isServer) {
    const screen = running ? s.local_screen : null;
    rows.push(["Fläche", screen ? `${screen.width} × ${screen.height}` : "–"]);
    const monitors = multipleMonitors(screen);
    if (monitors) rows.push(["Monitore", monitorsText(monitors)]);
    rows.push(["Port", String(state.settings.server.port)]);
    // Die Adressen stehen darunter in der Seitenleiste.
    rows.push(["Fingerprint", fingerprintNode(state.fingerprint)]);
  } else {
    rows.push(["Status", client ? (s.cursor_on === name ? "verbunden, Cursor ist hier" : "verbunden") : running ? "nicht verbunden" : "Server gestoppt"]);
    if (client) {
      rows.push(["Fläche", `${client.width} × ${client.height}`]);
      const monitors = multipleMonitors(client);
      if (monitors) rows.push(["Monitore", monitorsText(monitors)]);
      rows.push(["Adresse", client.addr]);
    }
    const chars = state.settings.server.keys.modes?.[name] === "chars";
    const typesText = client ? client.types_text !== false : true;
    rows.push(["Tasten", chars ? (typesText ? "als Zeichen" : "als Tastenposition (kann keine Zeichen eingeben)") : "als Tastenposition"]);
  }
  return el("dl", { class: "insp-list" }, ...rows.flatMap(([k, v]) => [el("dt", { text: k }), el("dd", {}, v)]));
}
