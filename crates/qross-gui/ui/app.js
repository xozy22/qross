"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => [...document.querySelectorAll(sel)];

const DEFAULT_PORT = 24900;

const state = {
  settings: null,
  status: null,
  fingerprint: "",
  hostname: "",
  localIp: null,
  grid: [5, 3],
  /** Bereits per Dialog angezeigte Freigabe-Anfragen (Name + Fingerprint) */
  prompted: new Set(),
  dialogOpen: false,
  /** Während des Ziehens wird das Raster nicht neu aufgebaut. */
  dragging: false,
  renderPending: false,
  /** Bildschirm, der nach dem Neuaufbau wieder den Fokus bekommt */
  focusName: null,
  /** Betriebssystem dieses Rechners */
  platform: "windows",
  /** Version dieser App und die Neuigkeiten aus CHANGELOG.md (einmal geladen) */
  version: "",
  releases: null,
  /** Ergebnis der LAN-Suche (null = noch nicht gesucht) */
  found: null,
  searching: false,
  searchError: null,
};

const PLATFORMS = {
  windows: { label: "Windows", short: "Win" },
  macos: { label: "macOS", short: "Mac" },
  linux: { label: "Linux", short: "Lx" },
  other: { label: "anderes System", short: "?" },
};

// ------------------------------------------------------------------ Hilfen

const ICONS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  x: '<path d="M7 7l10 10M17 7L7 17"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
  cursor: '<path d="M5 3l14 7.5-6.2 1.7L10 19z" fill="currentColor" stroke="none"/>',
  lock: '<rect x="5" y="11" width="14" height="9" rx="2" fill="currentColor" stroke="none"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
};

function icon(name) {
  const span = document.createElement("span");
  span.style.display = "contents";
  span.innerHTML = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;
  return span.firstElementChild;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v !== false && v != null) node.setAttribute(k, v === true ? "" : v);
  }
  for (const child of children.flat()) {
    if (child != null && child !== false) node.append(child);
  }
  return node;
}

/** Kurzer Hinweis unten; mit `action` ({ label, run }) bekommt er einen Knopf und bleibt länger stehen. */
function toast(message, kind = "", action = null) {
  const node = el(
    "div",
    { class: `toast ${kind}` },
    el("span", { text: message }),
    action
      ? el("button", {
          type: "button",
          class: "toast-action",
          text: action.label,
          onclick: () => {
            node.remove();
            action.run();
          },
        })
      : null,
  );
  $("#toasts").append(node);
  setTimeout(() => node.remove(), action ? 12000 : kind === "error" ? 7000 : 3500);
}

function fingerprintNode(fp, big = false) {
  const node = el("code", { class: big ? "fingerprint big" : "fingerprint" });
  fillFingerprint(node, fp);
  return node;
}

function fillFingerprint(node, fp) {
  node.replaceChildren(...(fp || "").split(":").map((g) => el("span", { text: g })));
}

function debounce(fn, ms) {
  let timer;
  const wrapped = (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
  wrapped.flush = () => {
    clearTimeout(timer);
    return fn();
  };
  return wrapped;
}

function running() {
  return state.status && state.status.role != null;
}

function store(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, value);
  } catch {
    return null;
  }
  return null;
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(`„${text}“ kopiert`);
  } catch {
    toast("Kopieren ist hier nicht möglich. Bitte den Text markieren.", "error");
  }
}

// ------------------------------------------------------------------ Dialog

/** Zeigt einen Dialog; liefert den Wert des gedrückten Knopfes (oder null). */
function showDialog({ title, body, actions }) {
  const dialog = $("#dialog");
  $("#dialogTitle").textContent = title;
  $("#dialogBody").replaceChildren(...[body].flat());
  $("#dialogActions").replaceChildren(
    ...actions.map((a) => el("button", { class: `btn ${a.kind || ""}`, value: a.value, text: a.label })),
  );
  return new Promise((resolve) => {
    const done = () => {
      dialog.removeEventListener("close", done);
      state.dialogOpen = false;
      resolve(dialog.returnValue || null);
    };
    dialog.returnValue = "";
    dialog.addEventListener("close", done);
    state.dialogOpen = true;
    dialog.showModal();
    const input = dialog.querySelector("input");
    if (input) {
      input.focus();
      input.select();
    }
  });
}

async function askName(title, initial = "") {
  const input = el("input", { type: "text", value: initial, spellcheck: "false", autocomplete: "off", "aria-label": "Name" });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      $("#dialog").close("ok");
    }
  });
  const result = await showDialog({
    title,
    body: [el("p", { text: "Verwende genau den Namen, mit dem sich der Rechner meldet. Das ist meist sein Rechnername." }), input],
    actions: [{ label: "Abbrechen", value: "cancel", kind: "ghost" }, { label: "Übernehmen", value: "ok", kind: "primary" }],
  });
  const name = input.value.trim();
  return result === "ok" && name ? name : null;
}

// ------------------------------------------------------------------ Einstellungen

const saveNow = async () => {
  try {
    await invoke("save_settings", { settings: state.settings });
  } catch (e) {
    toast(String(e), "error");
  }
};
const saveSoon = debounce(saveNow, 450);

async function settingsChanged({ immediate = false } = {}) {
  renderServer();
  updatePreview();
  if (immediate) await saveSoon.flush();
  else saveSoon();
}

async function updatePreview() {
  const preview = await invoke("preview_layout", { settings: state.settings });
  $("#tomlPreview").textContent = preview.toml;
  const note = $("#layoutNote");
  if (preview.error) {
    note.hidden = false;
    note.className = "note error";
    note.textContent = preview.error;
  } else if (preview.isolated.length) {
    note.hidden = false;
    note.className = "note";
    const names = preview.isolated.map((n) => `„${n}“`).join(", ");
    note.textContent = `${names} berührt keinen anderen Bildschirm. Schieb ihn direkt daneben, sonst kann der Cursor ihn nicht erreichen.`;
  } else {
    note.hidden = true;
  }
}

// ------------------------------------------------------------------ Kopfzeile

function renderTopbar() {
  const role = state.settings.role;
  for (const b of $$(".segmented button")) b.setAttribute("aria-checked", String(b.dataset.role === role));
  $("#serverView").hidden = role !== "server";
  $("#clientView").hidden = role !== "client";

  const s = state.status;
  const status = $("#status");
  status.dataset.phase = s.phase;
  status.dataset.clients = s.clients.length ? "yes" : "no";
  status.dataset.locked = s.role === "server" && s.locked ? "yes" : "no";

  let text = s.message || "Gestoppt";
  if (s.phase === "stopped") text = "Gestoppt";
  if (s.role === "server" && s.phase === "listening" && s.clients.length) {
    text = s.clients.length === 1 ? "1 Rechner verbunden" : `${s.clients.length} Rechner verbunden`;
  }
  if (s.role === "server" && s.locked) text += `, Cursor auf „${s.cursor_on}“ gesperrt`;
  $("#statusText").textContent = text;
  status.title = s.message || "";

  const toggle = $("#toggle");
  const isRunning = running();
  toggle.classList.toggle("running", isRunning);
  const label = role === "client" ? (isRunning ? "Trennen" : "Verbinden") : isRunning ? "Stoppen" : "Starten";
  $("#toggleText").textContent = label;
}

// ------------------------------------------------------------------ Layout-Editor

function renderServer() {
  if (state.dragging) {
    state.renderPending = true;
    return;
  }
  const server = state.settings.server;
  const status = state.status;
  const serverRunning = status.role === "server";
  const online = new Set(serverRunning ? status.clients.map((c) => c.name) : []);
  const cursorOn = serverRunning ? status.cursor_on : null;
  const locked = serverRunning && status.locked;
  const [cols, rows] = state.grid;
  const grid = $("#grid");
  grid.style.setProperty("--cols", cols);
  grid.style.setProperty("--rows", rows);

  const at = (c, r) => server.screens.find((s) => s.col === c && s.row === r);
  const live = (s) => serverRunning && (s.name === server.name || online.has(s.name));

  const hadFocus = document.activeElement?.classList.contains("screen") && grid.contains(document.activeElement);
  const nodes = [];
  for (let row = 0; row < rows; row++) {
    for (let col = 0; col < cols; col++) {
      if (at(col, row)) {
        nodes.push(el("div", { class: "slot", "data-col": col, "data-row": row, style: `grid-column:${col + 1};grid-row:${row + 1}` }));
      } else {
        nodes.push(
          el(
            "button",
            {
              type: "button",
              class: "slot",
              "data-col": col,
              "data-row": row,
              style: `grid-column:${col + 1};grid-row:${row + 1}`,
              "aria-label": "Bildschirm hier hinzufügen",
              title: "Bildschirm hier hinzufügen",
              onclick: () => addScreen(col, row),
            },
            icon("plus"),
          ),
        );
      }
    }
  }

  for (const s of server.screens) {
    const right = at(s.col + 1, s.row);
    if (right) {
      const cls = `link h${live(s) && live(right) ? " live" : ""}`;
      nodes.push(el("div", { class: cls, style: `grid-column:${s.col + 1} / span 2;grid-row:${s.row + 1}` }));
    }
    const below = at(s.col, s.row + 1);
    if (below) {
      const cls = `link v${live(s) && live(below) ? " live" : ""}`;
      nodes.push(el("div", { class: cls, style: `grid-column:${s.col + 1};grid-row:${s.row + 1} / span 2` }));
    }
  }

  for (const s of server.screens) {
    const isServer = s.name === server.name;
    const isOnline = online.has(s.name);
    const hasCursor = cursorOn === s.name;
    let sub = isServer ? "dieser Rechner" : "nicht verbunden";
    if (isOnline) {
      const c = status.clients.find((x) => x.name === s.name);
      sub = `${c.width} × ${c.height}`;
    }
    if (hasCursor && locked) sub = "Cursor gesperrt";
    const classes = ["screen", isServer && "server", isOnline && "online", hasCursor && "has-cursor", hasCursor && locked && "locked"]
      .filter(Boolean)
      .join(" ");
    const card = el(
      "div",
      {
        class: classes,
        tabindex: "0",
        role: "button",
        "data-name": s.name,
        "aria-label": `${s.name}, ${isServer ? "Server" : sub}`,
        style: `grid-column:${s.col + 1};grid-row:${s.row + 1}`,
        title: s.name,
      },
      isServer ? el("span", { class: "tag", text: "Server" }) : null,
      hasCursor ? el("span", { class: "marker", title: locked ? "Cursor hier gesperrt (Rollen-Taste)" : "Cursor ist hier" }, icon(locked ? "lock" : "cursor")) : null,
      el("span", { class: "name", text: s.name }),
      el("span", { class: "sub", text: sub }),
      isServer
        ? null
        : el(
            "button",
            {
              class: "remove",
              type: "button",
              title: "Entfernen",
              "aria-label": `${s.name} entfernen`,
              tabindex: "-1",
              onpointerdown: (e) => e.stopPropagation(),
              onclick: (e) => {
                e.stopPropagation();
                removeScreen(s.name);
              },
            },
            icon("x"),
          ),
    );
    card.addEventListener("pointerdown", (e) => startDrag(e, card, s));
    card.addEventListener("dblclick", () => renameScreen(s.name));
    card.addEventListener("keydown", (e) => screenKey(e, s));
    card.addEventListener("focus", () => (state.focusName = s.name));
    nodes.push(card);
  }
  grid.replaceChildren(...nodes);

  if (hadFocus && state.focusName) {
    grid.querySelector(`.screen[data-name="${CSS.escape(state.focusName)}"]`)?.focus();
  }

  const name = $("#serverName");
  if (document.activeElement !== name) name.value = server.name;
  const port = $("#serverPort");
  if (document.activeElement !== port) port.value = server.port;
  const corner = $("#cornerSize");
  if (document.activeElement !== corner) corner.value = server.corner_size;
  const delay = $("#switchDelay");
  if (document.activeElement !== delay) delay.value = server.switch_delay_ms;
  $("#discovery").checked = server.discovery;
  renderAddress();
}

function renderAddress() {
  const port = state.settings.server.port;
  const suffix = port === DEFAULT_PORT ? "" : `:${port}`;
  const values = [state.hostname, state.localIp].filter(Boolean).map((v) => v + suffix);
  $("#addressValues").replaceChildren(
    ...values.map((v) =>
      el("span", { class: "chip" }, v, el("button", { type: "button", title: "Kopieren", "aria-label": `${v} kopieren`, onclick: () => copy(v) }, icon("copy"))),
    ),
  );
}

/** Tastatur im Editor: Pfeile verschieben, Entf entfernt, Enter/F2 benennt um. */
function screenKey(e, screen) {
  const moves = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  if (moves[e.key]) {
    e.preventDefault();
    const [dc, dr] = moves[e.key];
    moveScreen(screen, screen.col + dc, screen.row + dr);
  } else if ((e.key === "Delete" || e.key === "Backspace") && screen.name !== state.settings.server.name) {
    e.preventDefault();
    removeScreen(screen.name);
  } else if (e.key === "F2" || e.key === "Enter") {
    e.preventDefault();
    renameScreen(screen.name);
  }
}

function moveScreen(screen, col, row) {
  const [cols, rows] = state.grid;
  if (col < 0 || row < 0 || col >= cols || row >= rows) return;
  if (screen.col === col && screen.row === row) return;
  const other = state.settings.server.screens.find((s) => s.col === col && s.row === row);
  if (other) {
    // Belegtes Feld: Plätze tauschen
    other.col = screen.col;
    other.row = screen.row;
  }
  screen.col = col;
  screen.row = row;
  state.focusName = screen.name;
  settingsChanged({ immediate: true }).then(() => focusScreen(screen.name));
  focusScreen(screen.name);
}

function focusScreen(name) {
  $(`#grid .screen[data-name="${CSS.escape(name)}"]`)?.focus();
}

function startDrag(e, card, screen) {
  if (e.button !== 0) return;
  e.preventDefault();
  card.focus({ preventScroll: true });
  const grid = $("#grid");
  const start = { x: e.clientX, y: e.clientY };
  let target = null;
  let moved = false;
  try {
    card.setPointerCapture(e.pointerId);
  } catch {
    // synthetische Ereignisse (Tests) haben keinen aktiven Zeiger
  }

  const slotAt = (x, y) => document.elementsFromPoint(x, y).find((n) => n.classList.contains("slot"));
  const onMove = (ev) => {
    const dx = ev.clientX - start.x;
    const dy = ev.clientY - start.y;
    if (!moved && Math.hypot(dx, dy) < 4) return;
    if (!moved) {
      moved = true;
      state.dragging = true;
      card.classList.add("drag");
      grid.classList.add("dragging");
    }
    card.style.transform = `translate(${dx}px, ${dy}px)`;
    const slot = slotAt(ev.clientX, ev.clientY);
    if (slot !== target) {
      target?.classList.remove("target");
      target = slot;
      target?.classList.add("target");
    }
  };
  const onUp = () => {
    card.removeEventListener("pointermove", onMove);
    card.removeEventListener("pointerup", onUp);
    card.removeEventListener("pointercancel", onUp);
    state.dragging = false;
    grid.classList.remove("dragging");
    card.classList.remove("drag");
    card.style.transform = "";
    if (moved && target) {
      moveScreen(screen, Number(target.dataset.col), Number(target.dataset.row));
    }
    if (state.renderPending || !moved || !target) {
      state.renderPending = false;
      renderServer();
    }
  };
  card.addEventListener("pointermove", onMove);
  card.addEventListener("pointerup", onUp);
  card.addEventListener("pointercancel", onUp);
}

/** Erstes freies Feld neben dem Server (rechts, links, oben, unten), sonst irgendwo. */
function freeCell() {
  const server = state.settings.server;
  const [cols, rows] = state.grid;
  const home = server.screens.find((s) => s.name === server.name) || { col: Math.floor(cols / 2), row: Math.floor(rows / 2) };
  const free = (c, r) => c >= 0 && r >= 0 && c < cols && r < rows && !server.screens.some((s) => s.col === c && s.row === r);
  const near = [[1, 0], [-1, 0], [0, -1], [0, 1]].map(([dc, dr]) => [home.col + dc, home.row + dr]);
  for (const [c, r] of near) if (free(c, r)) return [c, r];
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) if (free(c, r)) return [c, r];
  return null;
}

function addScreenNamed(name, cell = freeCell()) {
  const server = state.settings.server;
  if (server.screens.some((s) => s.name === name)) return true;
  if (!cell) {
    toast("Im Raster ist kein Feld mehr frei. Entferne zuerst einen Bildschirm.", "error");
    return false;
  }
  server.screens.push({ name, col: cell[0], row: cell[1] });
  return true;
}

async function addScreen(col, row) {
  const name = await askName("Bildschirm hinzufügen");
  if (!name) return;
  if (state.settings.server.screens.some((s) => s.name === name)) {
    toast(`„${name}“ ist schon in der Anordnung.`, "error");
    return;
  }
  const cell = col == null ? freeCell() : [col, row];
  if (addScreenNamed(name, cell)) {
    state.focusName = name;
    await settingsChanged({ immediate: true });
    focusScreen(name);
  }
}

async function renameScreen(oldName) {
  const server = state.settings.server;
  const name = await askName(oldName === server.name ? "Server umbenennen" : "Bildschirm umbenennen", oldName);
  if (!name || name === oldName) return focusScreen(oldName);
  if (server.screens.some((s) => s.name === name)) {
    toast(`„${name}“ ist schon in der Anordnung.`, "error");
    return;
  }
  server.screens.find((s) => s.name === oldName).name = name;
  if (server.name === oldName) server.name = name;
  state.focusName = name;
  await settingsChanged({ immediate: true });
  focusScreen(name);
}

function removeScreen(name) {
  const server = state.settings.server;
  server.screens = server.screens.filter((s) => s.name !== name);
  state.focusName = server.name;
  settingsChanged({ immediate: true });
  toast(`„${name}“ entfernt`);
}

function setServerName(name) {
  const server = state.settings.server;
  const own = server.screens.find((s) => s.name === server.name);
  if (own && !server.screens.some((s) => s !== own && s.name === name)) own.name = name;
  server.name = name;
  settingsChanged();
}

// ------------------------------------------------------------------ Clients und Freigaben

function renderClients() {
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
    list.replaceChildren(
      ...clients.map((c) =>
        el(
          "li",
          {},
          el("span", { class: "dot" }),
          el("div", {}, el("strong", { text: c.name }), s.cursor_on === c.name ? el("div", { class: "here", text: "Cursor ist hier" }) : null),
          el("span", { class: "meta" }, `${c.width} × ${c.height}`, el("br"), c.addr),
        ),
      ),
    );
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

function pendingText(p) {
  if (p.trust === "unknown") return "Vergleiche den Fingerprint mit der Anzeige auf diesem Rechner, bevor du ihn freigibst.";
  if (p.trust === "mismatch") return "Meldet sich mit einem anderen Fingerprint als bisher. Gib ihn nur frei, wenn Qross dort neu eingerichtet wurde.";
  if (!p.in_layout) return "Ist freigegeben, steht aber noch nicht in der Anordnung.";
  return `Abgewiesen: ${p.reason}`;
}

function approveLabel(p) {
  if (p.trust === "known") return "In Anordnung aufnehmen";
  if (p.trust === "mismatch") return "Neuen Fingerprint freigeben";
  return p.in_layout ? "Freigeben" : "Freigeben und aufnehmen";
}

function pendingNode(p) {
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

async function approvalDialog(p) {
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

async function approve(p) {
  try {
    if (p.trust !== "known") await invoke("approve_client", { name: p.name, fingerprint: p.fingerprint });
    if (!p.in_layout && addScreenNamed(p.name)) await settingsChanged({ immediate: true });
    if (p.trust === "known") invoke("dismiss_client", { name: p.name });
    toast(`„${p.name}“ freigegeben. Er verbindet sich beim nächsten Versuch.`);
  } catch (e) {
    toast(String(e), "error");
  }
}

// ------------------------------------------------------------------ Client-Ansicht

function renderClient() {
  const c = state.settings.client;
  const s = state.status;
  for (const [id, key] of [["#clientAddress", "address"], ["#clientName", "name"]]) {
    const input = $(id);
    if (document.activeElement !== input) input.value = c[key];
  }
  $("#clientDryRun").checked = c.dry_run;

  const isClient = s.role === "client";
  let phase = "stopped";
  if (isClient) phase = s.phase === "connected" ? "connected" : "connecting";
  else if (s.phase === "error") phase = "error";

  const visual = $("#linkVisual");
  visual.dataset.state = phase;
  visual.dataset.cursor = isClient && s.cursor_here ? "here" : "away";
  $("#visualServer").textContent = (isClient && s.server_name) || c.address || "Server";
  $("#visualClient").textContent = c.name || state.hostname || "Dieser Rechner";

  const hero = $(".client-hero");
  hero.dataset.phase = phase;
  const headline = {
    stopped: "Nicht verbunden",
    connecting: "Suche den Server …",
    connected: `Verbunden mit „${s.server_name}“`,
    error: "Verbindung fehlgeschlagen",
  }[phase];
  $("#clientHeadline").textContent = headline;

  let message = "Wähl einen gefundenen Server oder gib seine Adresse ein.";
  if (phase === "connecting") message = s.message;
  if (phase === "connected") {
    message = s.cursor_here
      ? "Der Cursor ist gerade auf diesem Rechner."
      : "Schieb die Maus am Server über den Bildschirmrand, um hierher zu wechseln.";
  }
  if (phase === "error") message = s.message;
  $("#clientMessage").textContent = message;
  renderFinder();
}

// ------------------------------------------------------------------ Server-Suche im LAN

async function searchServers() {
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
    renderFinder();
  }
}

function renderFinder() {
  $(".finder").classList.toggle("searching", state.searching);
  $("#searchServers").disabled = state.searching;
  $("#searchServers span").textContent = state.searching ? "Suche läuft …" : "Erneut suchen";
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
  const s = state.status;
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

async function connectTo(srv) {
  if (running()) {
    if (state.settings.client.address.trim() === srv.address) return;
    await invoke("stop");
  }
  state.settings.client.address = srv.address;
  $("#clientAddress").value = srv.address;
  renderClient();
  await startFlow();
}

async function serverConfirmDialog(probe) {
  const mismatch = !!probe.expected;
  const body = [];
  if (mismatch) {
    body.push(
      el("div", { class: "warnbox" }, "Der Server meldet einen anderen Fingerprint als beim letzten Mal. Das passiert, wenn Qross dort neu eingerichtet wurde, kann aber auch auf einen Angriff hindeuten."),
      el("div", { class: "fp-block" }, el("span", { class: "label", text: "Bisher gespeichert" }), fingerprintNode(probe.expected)),
    );
  } else {
    body.push(el("p", { text: `Erster Kontakt mit ${probe.address} (${probe.addr}). Vergleiche den Fingerprint mit „Dieser Server“ in Qross auf dem Server.` }));
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

async function checkServer() {
  await saveSoon.flush();
  const address = state.settings.client.address.trim();
  if (!address) {
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

// ------------------------------------------------------------------ Start/Stopp

async function toggle() {
  const button = $("#toggle");
  button.disabled = true;
  try {
    if (running()) await invoke("stop");
    else await startFlow();
  } finally {
    button.disabled = false;
  }
}

async function startFlow() {
  await saveSoon.flush();
  if (state.settings.role === "client" && !state.settings.client.address.trim()) {
    $("#clientAddress").focus();
    toast("Gib zuerst die Adresse des Servers ein.", "error");
    return;
  }
  try {
    let probe = await invoke("start");
    while (probe) {
      if (!(await serverConfirmDialog(probe))) return;
      probe = await invoke("start");
    }
  } catch (e) {
    toast(String(e), "error");
  }
}

// ------------------------------------------------------------------ Protokoll

const LEVELS = { TRACE: 0, DEBUG: 0, INFO: 1, WARN: 2, ERROR: 2 };

function logLine(line) {
  const t = new Date(line.ts).toLocaleTimeString("de-DE");
  const node = el("div", { class: line.level, "data-rank": LEVELS[line.level] ?? 1 }, el("span", { class: "t", text: t }), el("span", { class: "l", text: line.level }), line.message);
  node.hidden = Number(node.dataset.rank) < Number($("#logLevel").value);
  return node;
}

function appendLogs(lines) {
  if (!lines.length) return;
  const log = $("#log");
  const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 8;
  log.append(...lines.map(logLine));
  while (log.childElementCount > 1500) log.firstElementChild.remove();
  if (atBottom) log.scrollTop = log.scrollHeight;
  const last = lines[lines.length - 1];
  const lastNode = $("#logLast");
  lastNode.textContent = last.message;
  lastNode.className = `log-last ${last.level}`;
}

function filterLogs() {
  const min = Number($("#logLevel").value);
  for (const node of $("#log").children) node.hidden = Number(node.dataset.rank) < min;
  $("#log").scrollTop = $("#log").scrollHeight;
}

function setLogOpen(open) {
  $("#logPanel").dataset.open = String(open);
  $("#logToggle").setAttribute("aria-expanded", String(open));
  store("qross.logOpen", open ? "1" : "0");
  if (open) $("#log").scrollTop = $("#log").scrollHeight;
}

// ------------------------------------------------------------------ Neuigkeiten

const NEWS_SEEN = "qross.newsSeen"; // zuletzt angesehene Version
const NEWS_HINTED = "qross.newsHinted"; // Version, zu der der Update-Hinweis schon erschienen ist

/** Rubrik aus dem Changelog → Farbe des Tags (unbekannte Rubriken bleiben neutral). */
const SECTION_TONES = {
  neu: "neu",
  added: "neu",
  verbessert: "verbessert",
  geändert: "verbessert",
  changed: "verbessert",
  behoben: "behoben",
  fixed: "behoben",
  wichtig: "wichtig",
  sicherheit: "wichtig",
  security: "wichtig",
  entfernt: "wichtig",
  removed: "wichtig",
};

let newsReturnFocus = null;

function compareVersions(a, b) {
  const pa = String(a).split(".").map((n) => parseInt(n, 10) || 0);
  const pb = String(b).split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] || 0) - (pb[i] || 0);
    if (d) return d;
  }
  return 0;
}

/** „2026-10-18“ → „18. Oktober 2026“; anderes bleibt wie es ist. */
function formatDate(iso) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso || "");
  if (!m) return iso || "";
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).toLocaleDateString("de-DE", {
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}

/** Inline-Auszeichnung des Changelogs (**fett**, `code`) als DOM-Knoten – nie per innerHTML. */
function inlineNodes(text) {
  const nodes = [];
  for (const part of text.split(/(`[^`]+`)/)) {
    if (part.length > 2 && part.startsWith("`") && part.endsWith("`")) {
      nodes.push(el("code", { text: part.slice(1, -1) }));
      continue;
    }
    for (const piece of part.split(/(\*\*[^*]+\*\*)/)) {
      if (piece.length > 4 && piece.startsWith("**") && piece.endsWith("**")) nodes.push(el("strong", { text: piece.slice(2, -2) }));
      else if (piece) nodes.push(document.createTextNode(piece));
    }
  }
  return nodes;
}

function releaseNode(release) {
  const current = release.version === state.version;
  return el(
    "li",
    { class: `release${current ? " current" : ""}` },
    el("span", { class: "node", "aria-hidden": "true" }),
    el(
      "div",
      { class: "release-head" },
      el("h3", { text: release.version }),
      current ? el("span", { class: "installed", text: "installiert" }) : null,
      release.date ? el("time", { datetime: release.date, text: formatDate(release.date) }) : null,
    ),
    (release.intro || "").split(/\n{2,}/).filter(Boolean).map((p) => el("p", { class: "intro" }, inlineNodes(p))),
    release.sections.map((section) =>
      el(
        "section",
        { class: "group" },
        section.kind
          ? el("span", { class: `tag ${SECTION_TONES[section.kind.toLowerCase()] || "other"}`, text: section.kind })
          : null,
        el("ul", {}, section.items.map((item) => el("li", {}, inlineNodes(item)))),
      ),
    ),
  );
}

async function openNews() {
  const dialog = $("#news");
  if (dialog.open) return;
  if (!state.releases) {
    try {
      state.releases = await invoke("changelog");
    } catch {
      state.releases = [];
    }
  }
  const list = $("#newsList");
  list.replaceChildren(
    ...(state.releases.length
      ? state.releases.map(releaseNode)
      : [el("li", { class: "news-empty", text: "Für diese Version sind keine Neuigkeiten hinterlegt." })]),
  );
  newsReturnFocus = document.activeElement;
  dialog.showModal();
  list.scrollTop = 0;
  $("#newsClose").focus();
  markNewsSeen();
}

function markNewsSeen() {
  store(NEWS_SEEN, state.version);
  store(NEWS_HINTED, state.version);
  renderNewsChip(false);
}

function renderNewsChip(unseen) {
  const chip = $("#newsChip");
  $("#versionText").textContent = `v${state.version}`;
  chip.classList.toggle("has-news", unseen);
  chip.title = unseen ? `Neu in Version ${state.version}: Neuigkeiten ansehen` : "Neuigkeiten anzeigen";
  chip.setAttribute("aria-label", unseen ? `Version ${state.version}, neu – Neuigkeiten ansehen` : `Version ${state.version} – Neuigkeiten anzeigen`);
}

/** Nach einem Update: Punkt am Chip und einmal ein Hinweis. Beim allerersten Start nur die Version merken. */
function checkForNews() {
  const seen = store(NEWS_SEEN);
  if (!seen) {
    store(NEWS_SEEN, state.version);
    store(NEWS_HINTED, state.version);
    renderNewsChip(false);
    return;
  }
  const unseen = compareVersions(state.version, seen) > 0;
  renderNewsChip(unseen);
  if (unseen && store(NEWS_HINTED) !== state.version) {
    store(NEWS_HINTED, state.version);
    toast(`Qross wurde auf ${state.version} aktualisiert.`, "", { label: "Neuigkeiten ansehen", run: openNews });
  }
}

function setupNews() {
  const dialog = $("#news");
  $("#newsChip").addEventListener("click", openNews);
  $("#newsClose").addEventListener("click", () => dialog.close());
  // Klick auf den abgedunkelten Hintergrund schließt (der Inhalt füllt den Dialog vollständig aus).
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) dialog.close();
  });
  // Esc schließt auch dann, wenn der native Abbruch des Dialogs nicht greift.
  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      dialog.close();
    }
  });
  dialog.addEventListener("close", () => {
    newsReturnFocus?.focus?.();
    newsReturnFocus = null;
  });
  checkForNews();
}

// ------------------------------------------------------------------ Gesamt

function render() {
  renderTopbar();
  renderServer();
  renderClients();
  renderClient();
}

async function init() {
  const snap = await invoke("get_state");
  Object.assign(state, {
    settings: snap.settings,
    status: snap.status,
    fingerprint: snap.fingerprint,
    hostname: snap.hostname,
    localIp: snap.local_ip,
    grid: snap.grid,
    platform: snap.platform,
    version: snap.version,
  });
  for (const node of $$("[data-own-fingerprint]")) fillFingerprint(node, snap.fingerprint);
  for (const node of $$("[data-data-dir]")) node.textContent = snap.data_dir;
  $("#clientName").placeholder = snap.hostname;
  appendLogs(snap.logs);
  setLogOpen(store("qross.logOpen") === "1");
  render();
  updatePreview();

  await listen("status", (e) => {
    state.status = e.payload;
    render();
  });
  await listen("log", (e) => appendLogs([e.payload]));
  await listen("start-from-tray", () => {
    if (!running()) startFlow();
  });
  await listen("show-news", () => openNews());
  setupNews();

  for (const b of $$(".segmented button")) {
    b.addEventListener("click", async () => {
      if (state.settings.role === b.dataset.role) return;
      state.settings.role = b.dataset.role;
      render();
      if (b.dataset.role === "client" && !running()) searchServers();
      await saveSoon.flush();
    });
  }
  $("#toggle").addEventListener("click", toggle);
  $("#addScreen").addEventListener("click", () => addScreen());
  $("#serverName").addEventListener("input", (e) => setServerName(e.target.value.trim()));
  $("#serverPort").addEventListener("input", (e) => {
    const port = Number(e.target.value);
    if (port >= 1 && port <= 65535) {
      state.settings.server.port = port;
      settingsChanged();
    }
  });
  $("#clientAddress").addEventListener("input", (e) => {
    state.settings.client.address = e.target.value;
    saveSoon();
    renderClient();
  });
  $("#clientName").addEventListener("input", (e) => {
    state.settings.client.name = e.target.value.trim();
    saveSoon();
    renderClient();
  });
  $("#clientDryRun").addEventListener("change", (e) => {
    state.settings.client.dry_run = e.target.checked;
    saveSoon.flush();
  });
  $("#clientAddress").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !running()) startFlow();
  });
  $("#checkServer").addEventListener("click", checkServer);
  $("#searchServers").addEventListener("click", searchServers);
  const numberSetting = (id, key, max) =>
    $(id).addEventListener("input", (e) => {
      const value = Number(e.target.value);
      if (e.target.value !== "" && Number.isInteger(value) && value >= 0 && value <= max) {
        state.settings.server[key] = value;
        settingsChanged();
      }
    });
  numberSetting("#cornerSize", "corner_size", 400);
  numberSetting("#switchDelay", "switch_delay_ms", 3000);
  $("#discovery").addEventListener("change", (e) => {
    state.settings.server.discovery = e.target.checked;
    settingsChanged({ immediate: true });
  });
  if (state.settings.role === "client" && !running()) searchServers();
  $("#logLevel").addEventListener("change", filterLogs);
  $("#logClear").addEventListener("click", () => {
    $("#log").replaceChildren();
    $("#logLast").textContent = "";
  });
  $("#logToggle").addEventListener("click", () => setLogOpen($("#logPanel").dataset.open !== "true"));
}

init().catch((e) => {
  document.body.prepend(el("div", { class: "note error", text: `Die Oberfläche konnte nicht geladen werden: ${e}` }));
});
