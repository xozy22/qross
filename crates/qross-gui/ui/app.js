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
  /** Zuletzt gezeigter Bereich der Einstellungen */
  settingsSection: "set-general",
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
  /** Vom System: Autostart, Update-Status, Datenordner (siehe get_prefs) */
  prefs: null,
  /** Benennbare Tasten [{ name, label }] und Beschriftung je Name */
  keyNames: null,
  keyLabels: new Map(),
  /** Rechner, dessen Umbelegung gerade angezeigt wird */
  remapPc: null,
  /** Laufende Tastenaufnahme (Fang-Taste oder Auswahlfeld) */
  recording: null,
  /** Ergebnis der LAN-Suche (null = noch nicht gesucht) */
  found: null,
  searching: false,
  searchError: null,
  /** Nur macOS: { accessibility, input_monitoring } (null = nicht geprüft) */
  permissions: null,
  /** Grafische Sitzung: windows, macos, x11, wayland, other */
  desktop: "windows",
  /** Nur Wayland: Stand der Desktop-Portale (siehe portal_state), null = unbekannt */
  portal: null,
  /** Profile je Ort ({ active, auto, fallback, list }) */
  profiles: null,
  /** Aktuelle Netzwerke, bevorzugtes zuerst (null = noch nicht abgefragt) */
  networks: null,
  /** Aufgeklappte Einträge unter „Rechte je Rechner“ (Name; "" = Vorgabe für neue Rechner) */
  sharingOpen: new Set(),
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
  arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  screen: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
  cursor: '<path d="M5 3l14 7.5-6.2 1.7L10 19z" fill="currentColor" stroke="none"/>',
  lock: '<rect x="5" y="11" width="14" height="9" rx="2" fill="currentColor" stroke="none"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
  game: '<path d="M7.5 7h9a5 5 0 0 1 4.9 6l-.7 3.4a2.2 2.2 0 0 1-3.7 1.1L14.6 15H9.4l-2.4 2.5a2.2 2.2 0 0 1-3.7-1.1L2.6 13a5 5 0 0 1 4.9-6z"/><path d="M8 9.5v3M6.5 11h3"/><path d="M15.5 10.5h.01M17.5 12h.01"/>',
  up: '<path d="M12 19V5M6 11l6-6 6 6"/>',
  shield: '<path d="M12 3l7 3v5.5c0 4.2-2.9 7.8-7 9.5-4.1-1.7-7-5.3-7-9.5V6z"/><path d="M9 12h6"/>',
  down: '<path d="M12 5v14M6 13l6 6 6-6"/>',
  pencil: '<path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>',
  trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>',
  wifi: '<path d="M2 8.5a15 15 0 0 1 20 0M5.5 12a10 10 0 0 1 13 0M9 15.5a5 5 0 0 1 6 0"/><path d="M12 19h.01"/>',
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
  setTimeout(() => node.remove(), action ? 12000 : kind === "error" || kind === "warn" ? 7000 : 3500);
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

async function askName(title, initial = "", text = "Verwende genau den Namen, mit dem sich der Rechner meldet. Das ist meist sein Rechnername.") {
  const input = el("input", { type: "text", value: initial, spellcheck: "false", autocomplete: "off", "aria-label": "Name" });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      $("#dialog").close("ok");
    }
  });
  const result = await showDialog({
    title,
    body: [el("p", { text }), input],
    actions: [{ label: "Abbrechen", value: "cancel", kind: "ghost" }, { label: "Übernehmen", value: "ok", kind: "primary" }],
  });
  const name = input.value.trim();
  return result === "ok" && name ? name : null;
}

/** „a“, „a“ und „b“, „a“, „b“ und „c“ */
function joinNames(names) {
  const quoted = names.map((n) => `„${n}“`);
  return quoted.length > 1 ? `${quoted.slice(0, -1).join(", ")} und ${quoted.at(-1)}` : (quoted[0] ?? "");
}

/**
 * Fragt nach, bevor ein Wechsel der Rolle (Knopf oder Profil) den laufenden Server ausschaltet
 * oder die Verbindung des Clients trennt. Liefert true, wenn es weitergehen soll.
 */
async function confirmRoleChange(role) {
  const s = state.status;
  if (!role || !running() || s.role === role) return true;
  if (s.role === "client" && s.phase !== "connected") return true;
  const toClient = s.role === "server";
  const names = toClient ? s.clients.map((c) => c.name) : [];
  const lose = names.length
    ? ` ${joinNames(names)} ${names.length === 1 ? "verliert" : "verlieren"} dann Maus und Tastatur dieses Rechners.`
    : "";
  const server = s.server_name ? `„${s.server_name}“` : "dem Server";
  const result = await showDialog({
    title: toClient ? "Server ausschalten?" : "Verbindung trennen?",
    body: toClient
      ? [
          el("p", { text: `Achtung: Der Server schaltet sich aus, wenn dieser Rechner zum Client wird.${lose}` }),
          el("p", { text: "Willst du wirklich zum Client wechseln?" }),
        ]
      : [
          el("p", { text: `Achtung: Die Verbindung zu ${server} wird getrennt, wenn dieser Rechner zum Server wird.` }),
          el("p", { text: "Willst du wirklich zum Server wechseln?" }),
        ],
    actions: [
      { label: "Abbrechen", value: "cancel", kind: "ghost" },
      { label: toClient ? "Server ausschalten" : "Verbindung trennen", value: "ok", kind: "danger" },
    ],
  });
  return result === "ok";
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
  status.dataset.locked = s.role === "server" && (s.locked || s.game_hold) ? "yes" : "no";

  let text = s.message || "Gestoppt";
  if (s.phase === "stopped") text = "Gestoppt";
  if (s.role === "server" && s.phase === "listening" && s.clients.length) {
    text = s.clients.length === 1 ? "1 Rechner verbunden" : `${s.clients.length} Rechner verbunden`;
  }
  if (s.role === "server" && s.locked) text += `, Cursor auf „${s.cursor_on}“ gesperrt`;
  else if (s.role === "server" && s.game_hold) text += `, Spielemodus hält den Cursor auf „${s.cursor_on}“`;
  $("#statusText").textContent = text;
  status.title = s.message || "";

  const toggle = $("#toggle");
  const isRunning = running();
  toggle.classList.toggle("running", isRunning);
  const label = role === "client" ? (isRunning ? "Trennen" : "Verbinden") : isRunning ? "Stoppen" : "Starten";
  $("#toggleText").textContent = label;
}

// ------------------------------------------------------------------ Layout-Editor

/** Monitore eines Rechners, sofern es mehrere sind (sonst null: Darstellung wie bisher) */
function multipleMonitors(screen) {
  return screen?.monitors?.length > 1 ? screen.monitors : null;
}

/** Zusatz für Beschriftungen, z. B. „ · 2 Monitore“ */
function monitorNote(screen) {
  const monitors = multipleMonitors(screen);
  return monitors ? ` · ${monitors.length} Monitore` : "";
}

/** Tooltip: Name, dann jeder Monitor von links nach rechts und die Gesamtauflösung */
function screenTitle(name, screen) {
  const monitors = multipleMonitors(screen);
  if (!monitors) return name;
  const sorted = [...monitors].sort((a, b) => a.x - b.x || a.y - b.y);
  return [name, ...sorted.map((m, i) => `Monitor ${i + 1}: ${m.width} × ${m.height}`), `Gesamt: ${screen.width} × ${screen.height}`].join("\n");
}

/** Maßstäbliche Skizze der Monitore hinter der Beschriftung des Kastens */
function monitorSketch(screen) {
  const monitors = multipleMonitors(screen);
  if (!monitors) return null;
  const left = Math.min(...monitors.map((m) => m.x));
  const top = Math.min(...monitors.map((m) => m.y));
  const width = Math.max(...monitors.map((m) => m.x + m.width)) - left;
  const height = Math.max(...monitors.map((m) => m.y + m.height)) - top;
  // Schmale Fuge, damit aneinanderstoßende Monitore einzeln erkennbar bleiben
  const gap = Math.max(width, height) * 0.012;
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", "monitors");
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.setAttribute("aria-hidden", "true");
  for (const m of monitors) {
    const rect = document.createElementNS(ns, "rect");
    rect.setAttribute("x", m.x - left + gap / 2);
    rect.setAttribute("y", m.y - top + gap / 2);
    rect.setAttribute("width", Math.max(m.width - gap, 1));
    rect.setAttribute("height", Math.max(m.height - gap, 1));
    rect.setAttribute("rx", gap);
    svg.append(rect);
  }
  return svg;
}

/** Hinweis über dem Layout, solange der Spielemodus den Cursor festhält */
function renderGameNote() {
  const note = $("#gameNote");
  const s = state.status;
  const game = s.role === "server" && !s.locked ? s.game_hold : null;
  note.hidden = !game;
  if (!game) return;
  const where = s.cursor_on === state.settings.server.name ? "Auf diesem Rechner" : `Auf „${s.cursor_on}“`;
  const what =
    game === "captured"
      ? `${where} hat ein Spiel den Zeiger eingefangen. Der Cursor bleibt dort, bis es ihn freigibt.`
      : `${where} läuft ein Programm im Vollbild. Der Cursor bleibt dort, bis es endet.`;
  const lock = state.settings.server.keys.lock;
  // Die Tastennamen („Rollen“) lädt sonst erst der Einstellungsbereich
  if (lock && !state.keyNames) loadKeyNames().then(renderGameNote);
  const release = lock
    ? ` ${hotkeyLabel(lock)} gibt ihn sofort frei.`
    : " Die Fang-Taste ist aus; Strg+Alt+Umschalt+Esc holt ihn zu diesem Rechner zurück.";
  note.replaceChildren(el("strong", { text: "Spielemodus: " }), what + release);
}

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
  // Spielemodus: festgehalten wie mit der Fang-Taste, nur mit anderem Grund
  const game = serverRunning && !locked && status.game_hold;
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
    // Gesamtfläche und Monitore: vom laufenden Server selbst und von verbundenen Clients
    let screen = isServer && serverRunning ? status.local_screen : null;
    if (isOnline) {
      const c = status.clients.find((x) => x.name === s.name);
      sub = `${c.width} × ${c.height}`;
      screen = c;
    }
    if (hasCursor && locked) sub = "Cursor gesperrt";
    if (hasCursor && game) sub = game === "captured" ? "Spiel hält den Zeiger" : "Vollbild, Cursor bleibt";
    const held = hasCursor && (locked || game);
    const marker = locked
      ? { title: "Cursor hier gesperrt (Rollen-Taste)", icon: "lock" }
      : game
        ? { title: "Spielemodus: Cursor bleibt hier, bis das Vollbild endet (Fang-Taste gibt ihn frei)", icon: "game" }
        : { title: "Cursor ist hier", icon: "cursor" };
    const classes = ["screen", isServer && "server", isOnline && "online", hasCursor && "has-cursor", held && "locked"]
      .filter(Boolean)
      .join(" ");
    // Eingeschränkte Rechte: kleines Schild, das zum Eintrag in der Seitenleiste führt
    const limits = isServer ? null : sharingSummary(sharingOf(s.name));
    const card = el(
      "div",
      {
        class: classes,
        tabindex: "0",
        role: "button",
        "data-name": s.name,
        "aria-label": `${s.name}, ${isServer ? "Server" : sub}${monitorNote(screen)}${limits ? ", Rechte eingeschränkt" : ""}`,
        style: `grid-column:${s.col + 1};grid-row:${s.row + 1}`,
        title: screenTitle(s.name, screen),
      },
      monitorSketch(screen),
      isServer ? el("span", { class: "tag", text: "Server" }) : null,
      hasCursor ? el("span", { class: "marker", title: marker.title }, icon(marker.icon)) : null,
      el("span", { class: "name", text: s.name }),
      el("span", { class: "sub", text: sub }),
      limits
        ? el(
            "button",
            {
              class: "share-badge",
              type: "button",
              title: `Eingeschränkt: ${limits}`,
              "aria-label": `Rechte von ${s.name} anzeigen`,
              tabindex: "-1",
              onpointerdown: (e) => e.stopPropagation(),
              onclick: (e) => {
                e.stopPropagation();
                openSharing(s.name);
              },
            },
            icon("shield"),
          )
        : null,
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
  renderClipLimit();
  renderAddress();
  renderSharing();
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
  // Neue Rechner bekommen die Vorgabe als eigene Rechte.
  server.sharing.clients[name] ??= { ...server.sharing.default };
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
  renameSwitchKey(oldName, name);
  renameSharing(oldName, name);
  renameKeyMode(oldName, name);
  state.focusName = name;
  await settingsChanged({ immediate: true });
  focusScreen(name);
}

function removeScreen(name) {
  const server = state.settings.server;
  server.screens = server.screens.filter((s) => s.name !== name);
  delete server.sharing.clients[name];
  state.focusName = server.name;
  settingsChanged({ immediate: true });
  toast(`„${name}“ entfernt`);
}

function setServerName(name) {
  const server = state.settings.server;
  const own = server.screens.find((s) => s.name === server.name);
  if (own && !server.screens.some((s) => s !== own && s.name === name)) {
    renameSwitchKey(own.name, name);
    own.name = name;
  }
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

// ------------------------------------------------------------------ Rechte je Rechner

/** Was sich je Rechner freigeben lässt; `key` wie in gui.json, `short` für Kurzfassungen */
const SHARE_KINDS = [
  { key: "clipboard", label: "Zwischenablage", short: "Zwischenablage", hint: "Text, HTML, Bild" },
  { key: "files", label: "Kopierte Dateien", short: "Dateien", hint: "Kopieren, Einfügen" },
  { key: "drag", label: "Drag & Drop", short: "Drag & Drop", hint: "Dateien ziehen" },
];
/** Aus Sicht des jeweiligen Rechners: „to-client“ heißt, er empfängt nur. */
const SHARE_OPTIONS = [
  ["both", "Senden und empfangen"],
  ["to-client", "Nur empfangen"],
  ["from-client", "Nur senden"],
  ["off", "Aus"],
];
const SHARE_SHORT = { "to-client": "nur empfangen", "from-client": "nur senden", off: "aus" };
/** Eintrag für die Vorgabe (kein gültiger Rechnername) */
const SHARE_DEFAULT = "";

/** Clients in der Anordnung, ohne den Server selbst */
function sharingClients() {
  const server = state.settings.server;
  return server.screens.map((s) => s.name).filter((n) => n !== server.name);
}

/** Rechte eines Rechners; ohne eigenen Eintrag gilt die Vorgabe (so sieht es auch der Server) */
function sharingOf(name) {
  const rules = state.settings.server.sharing;
  return (name !== SHARE_DEFAULT && rules.clients[name]) || rules.default;
}

/** Kurzfassung, z. B. „Zwischenablage nur empfangen · Drag & Drop aus“; `null`, wenn alles erlaubt ist */
function sharingSummary(sharing) {
  const parts = SHARE_KINDS.filter((k) => sharing[k.key] !== "both").map((k) => `${k.short} ${SHARE_SHORT[sharing[k.key]]}`);
  return parts.length ? parts.join(" · ") : null;
}

function renderSharing() {
  const list = $("#sharingList");
  const names = [...sharingClients(), SHARE_DEFAULT];
  // Nur neu aufbauen, wenn sich die Rechner ändern; sonst bleiben Auswahl und Fokus erhalten.
  const key = JSON.stringify(names);
  if (list.dataset.names !== key) {
    list.dataset.names = key;
    list.replaceChildren(...names.map(sharingEntry));
  }
  for (const details of list.querySelectorAll(".share-entry")) {
    const name = details.dataset.name;
    const sharing = sharingOf(name);
    for (const select of details.querySelectorAll("select")) {
      if (document.activeElement !== select) select.value = sharing[select.dataset.kind];
    }
    const limits = sharingSummary(sharing);
    details.classList.toggle("limited", limits !== null);
    details.querySelector(".share-sum").textContent = limits ?? "Alles erlaubt";
  }
}

function sharingEntry(name) {
  const isDefault = name === SHARE_DEFAULT;
  const who = isDefault ? "neue Rechner" : name;
  const rows = SHARE_KINDS.map((kind) =>
    el(
      "label",
      { class: "share-row" },
      el("span", { class: "share-kind" }, el("span", { text: kind.label }), el("span", { class: "hint", text: kind.hint })),
      el(
        "select",
        { "data-kind": kind.key, "aria-label": `${kind.label} für ${who}`, onchange: (e) => setShare(name, kind.key, e.target.value) },
        ...SHARE_OPTIONS.map(([value, label]) => el("option", { value, text: label })),
      ),
    ),
  );
  const details = el(
    "details",
    {
      class: `share-entry${isDefault ? " default" : ""}`,
      "data-name": name,
      ontoggle: (e) => (e.target.open ? state.sharingOpen.add(name) : state.sharingOpen.delete(name)),
    },
    el(
      "summary",
      {},
      el("span", { class: "share-head" }, el("span", { class: "share-name", text: isDefault ? "Neue Rechner" : name }), el("span", { class: "share-sum" })),
    ),
    isDefault ? el("p", { class: "hint share-default-hint", text: "Vorgabe für Rechner, die neu in die Anordnung kommen. Eingetragene behalten ihre Rechte." }) : null,
    ...rows,
  );
  details.open = state.sharingOpen.has(name);
  return el("li", {}, details);
}

function setShare(name, kind, value) {
  const rules = state.settings.server.sharing;
  if (name === SHARE_DEFAULT) {
    // Eingetragene Rechner behalten, was sie bisher hatten.
    for (const client of sharingClients()) rules.clients[client] ??= { ...rules.default };
    rules.default = { ...rules.default, [kind]: value };
  } else {
    rules.clients[name] = { ...sharingOf(name), [kind]: value };
  }
  settingsChanged({ immediate: true });
}

function renameSharing(oldName, name) {
  const clients = state.settings.server.sharing.clients;
  if (clients[oldName]) clients[name] = clients[oldName];
  delete clients[oldName];
  if (state.sharingOpen.delete(oldName)) state.sharingOpen.add(name);
}

/** Vom Schild auf einer Karte: Eintrag aufklappen und zeigen */
function openSharing(name) {
  state.sharingOpen.add(name);
  renderSharing();
  const details = $(`#sharingList .share-entry[data-name="${CSS.escape(name)}"]`);
  if (!details) return;
  details.open = true;
  details.scrollIntoView({ block: "nearest", behavior: "smooth" });
  details.querySelector("summary").focus({ preventScroll: true });
}

/** Client-Ansicht: was der Server mit diesem Rechner nicht oder nur in eine Richtung teilt */
function renderClientSharing(sharing) {
  const note = $("#clientSharing");
  const limits = sharing && sharingSummary(sharing);
  note.hidden = !limits;
  if (limits) note.replaceChildren(el("strong", { text: "Vom Server eingeschränkt: " }), limits);
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
  renderClipLimit();

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
  renderClientSharing(phase === "connected" ? s.sharing : null);

  const qualitySlot = $("#clientQuality");
  qualitySlot.hidden = phase !== "connected";
  if (phase === "connected") {
    const quality = qualityEntry("server");
    if (qualitySlot.firstElementChild !== quality.pill) qualitySlot.replaceChildren(quality.pill, quality.panel);
  } else if (qualitySlot.firstElementChild) {
    qualitySlot.replaceChildren();
  }
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

// ------------------------------------------------------------------ Kopierte Dateien

const TOAST_KIND = { info: "", success: "success", warn: "warn", error: "error" };

function renderClipLimit() {
  for (const input of $$("[data-clip-limit]")) {
    if (document.activeElement !== input) input.value = state.settings.clipboard_files_mb;
  }
}

function setupClipboardFiles() {
  const mod = state.platform === "macos" ? "Cmd" : "Strg";
  for (const hint of $$("[data-clip-hint]")) {
    hint.textContent = `${mod}+C auf Dateien, drüben ${mod}+V. Größeres per Drag & Drop. 0 schaltet es aus.`;
  }
  for (const input of $$("[data-clip-limit]")) {
    input.addEventListener("input", () => {
      const value = Number(input.value);
      if (input.value === "" || !Number.isInteger(value) || value < 0 || value > 100000) return;
      state.settings.clipboard_files_mb = value;
      renderClipLimit();
      updatePreview();
      saveSoon();
    });
  }
}

// ------------------------------------------------------------------ Dateiübertragungen

/** Erst ab dieser Laufzeit erscheint eine Zeile; kürzere Übertragungen würden nur aufblitzen. */
const TRANSFER_SHOW_AFTER_MS = 500;
/** So lange bleibt eine beendete Zeile stehen */
const TRANSFER_LINGER_MS = { done: 5000, cancelled: 4000, failed: 9000 };
/** Angezeigte Übertragungen: id → { data, node, parts, final, timer }. Die Knoten bleiben bestehen
 *  und werden nur aktualisiert, damit bei vier Meldungen pro Sekunde nichts flackert. */
const transfers = new Map();

function transferRow(t) {
  const parts = {
    name: el("strong", { class: "tname" }),
    route: el("span", { class: "troute" }),
    pct: el("span", { class: "tpct" }),
    fill: el("span", { class: "tfill" }),
    outcome: el("span", { class: "toutcome" }),
    size: el("span", { class: "tsize" }),
    speed: el("span", { class: "tspeed" }),
  };
  parts.bar = el("div", { class: "tbar", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100" }, parts.fill);
  parts.cancel = el("button", {
    type: "button",
    class: "btn ghost small tcancel",
    text: "Abbrechen",
    "aria-label": `Übertragung von ${t.name} abbrechen`,
    onclick: () => cancelTransfer(t.id),
  });
  const node = el(
    "div",
    { class: "transfer", "data-direction": t.direction, "data-state": "running" },
    icon(t.direction === "sending" ? "up" : "down"),
    el(
      "div",
      { class: "tmain" },
      el("div", { class: "tline" }, parts.name, parts.route, parts.pct),
      parts.bar,
      el("div", { class: "tmeta" }, parts.outcome, parts.size, parts.speed),
    ),
    parts.cancel,
  );
  return { node, parts, final: false, timer: null };
}

function showPercent(entry, percent) {
  const whole = Math.floor(percent);
  entry.parts.pct.textContent = `${whole}${NBSP}%`;
  entry.parts.fill.style.width = `${percent}%`;
  entry.parts.bar.setAttribute("aria-valuenow", whole);
}

/** Meldung aus dem Backend (etwa 4×/s je Übertragung, zum Schluss mit Ergebnis) */
function onTransfer(t) {
  let entry = transfers.get(t.id);
  if (!entry) {
    // Kurze Übertragungen bekommen keine Zeile, ein Ende ohne Zeile braucht keine.
    if (t.state !== "running" || t.elapsed_ms < TRANSFER_SHOW_AFTER_MS) return;
    entry = transferRow(t);
    transfers.set(t.id, entry);
    $("#transfers").append(entry.node);
    $("#transfers").hidden = false;
  }
  // Schon beendet, etwa durch „Kopierte Dateien sind bereit“
  if (entry.final) return;
  entry.data = t;
  const { parts } = entry;
  parts.name.textContent = t.name;
  parts.name.title = t.name;
  parts.route.textContent = t.route;
  parts.size.textContent = t.size;
  parts.speed.textContent = t.speed;
  showPercent(entry, t.percent);
  if (t.state !== "running") finishTransfer(entry, t.outcome, t.state);
}

/** Ergebnis in die Zeile schreiben; sie verschwindet nach ein paar Sekunden. */
function finishTransfer(entry, text, state) {
  entry.final = true;
  entry.node.dataset.state = state;
  entry.parts.outcome.textContent = text || "";
  entry.parts.cancel.disabled = true;
  if (state === "done") showPercent(entry, 100);
  clearTimeout(entry.timer);
  entry.timer = setTimeout(() => removeTransfer(entry), TRANSFER_LINGER_MS[state] ?? 4000);
}

function removeTransfer(entry) {
  entry.node.classList.add("leaving");
  setTimeout(() => {
    if (transfers.get(entry.data.id) === entry) transfers.delete(entry.data.id);
    entry.node.remove();
    $("#transfers").hidden = transfers.size === 0;
  }, 200);
}

/** Gestoppt oder neu gestartet: Meldungen der alten Instanz kommen nicht mehr. */
function clearTransfers() {
  if (!transfers.size) return;
  for (const entry of transfers.values()) {
    clearTimeout(entry.timer);
    entry.node.remove();
  }
  transfers.clear();
  $("#transfers").hidden = true;
}

async function cancelTransfer(id) {
  const entry = transfers.get(id);
  if (entry) entry.parts.cancel.disabled = true;
  try {
    // Das Ende kommt als Meldung, auch wenn die Übertragung gerade von selbst fertig wurde.
    await invoke("cancel_transfer", { id });
  } catch (e) {
    toast(String(e), "error");
    if (entry && !entry.final) entry.parts.cancel.disabled = false;
  }
}

/** Zeile für kopierte Dateien, die gerade von `peer` ankommen (oder eben angekommen sind) */
function clipboardRow(peer) {
  return [...transfers.values()]
    .reverse()
    .find((e) => e.data.kind === "clipboard" && e.data.direction === "receiving" && e.data.peer === peer);
}

/** Hinweis zu kopierten Dateien: „bereit“ und Fehler landen in deren Zeile, sonst als Toast. */
function onClipboardFiles(message) {
  if (!message.text) return;
  const row = (message.event === "ready" || message.event === "failed") && clipboardRow(message.peer);
  if (row) finishTransfer(row, message.text, message.event === "ready" ? "done" : "failed");
  else toast(message.text, TOAST_KIND[message.tone] ?? "");
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

/**
 * Gemeinsames Verhalten der Dialoge „Neuigkeiten“ und „Einstellungen“: Esc und Klick auf den
 * Hintergrund schließen, der Fokus geht beim Schließen an den Auslöser zurück.
 */
function setupSheet(dialog, closeButton) {
  closeButton.addEventListener("click", () => dialog.close());
  // Der Inhalt füllt den Dialog aus: Ein Klick direkt auf das Dialog-Element ist der Hintergrund.
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
    dialog.returnFocus?.focus?.();
    dialog.returnFocus = null;
  });
}

function openSheet(dialog, focusTarget) {
  if (dialog.open) return false;
  dialog.returnFocus = document.activeElement;
  dialog.showModal();
  focusTarget.focus();
  return true;
}

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
  openSheet(dialog, $("#newsClose"));
  list.scrollTop = 0;
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
  $("#newsChip").addEventListener("click", openNews);
  setupSheet($("#news"), $("#newsClose"));
  checkForNews();
}

// ------------------------------------------------------------------ Einstellungen

async function refreshPrefs() {
  try {
    state.prefs = await invoke("get_prefs");
  } catch (e) {
    toast(String(e), "error");
  }
}

async function openSettings(section) {
  const dialog = $("#settings");
  if (!dialog.open) {
    await loadKeyNames();
    await refreshPrefs();
    renderSettings();
    // Ohne Ziel wieder dort, wo man zuletzt war (bis zum Neustart)
    showSection(section || state.settingsSection);
    openSheet(dialog, $("#settingsClose"));
    renderProfiles();
    watchNetworks(true);
  } else if (section) {
    showSection(section);
  }
}

/** Zeigt einen Bereich der Einstellungen als eigene Seite; die anderen sind ausgeblendet. */
function showSection(id) {
  if (!id || !$(`#${id}.settings-section`)) id = "set-general";
  state.settingsSection = id;
  for (const section of $$(".settings-section")) section.hidden = section.id !== id;
  for (const tab of $$(".settings-nav button")) {
    const active = tab.dataset.section === id;
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    // Schmales Fenster: Die Leiste liegt als Zeile oben und lässt sich schieben; den gewählten Bereich zeigen
    const nav = tab.parentElement;
    if (active && nav.scrollWidth > nav.clientWidth) {
      nav.scrollLeft = Math.max(0, Math.min(nav.scrollLeft, tab.offsetLeft - 12), tab.offsetLeft + tab.offsetWidth + 36 - nav.clientWidth);
    }
  }
  $("#settingsContent").scrollTop = 0;
}

/** Pfeiltasten, Pos1 und Ende wechseln zwischen den Bereichen (wie bei Registerkarten üblich). */
function onSettingsNavKey(e) {
  const tabs = $$(".settings-nav button");
  const i = tabs.indexOf(document.activeElement);
  const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
  const next = step ? (i + step + tabs.length) % tabs.length : { Home: 0, End: tabs.length - 1 }[e.key];
  if (i < 0 || next === undefined) return;
  e.preventDefault();
  tabs[next].focus();
  showSection(tabs[next].dataset.section);
}

function renderSettings() {
  const prefs = state.prefs;
  if (!prefs) return;
  renderKeys();
  const autostart = $("#prefAutostart");
  autostart.checked = prefs.autostart;
  autostart.disabled = !prefs.autostart_editable;
  autostart.closest(".setting").classList.toggle("disabled", !prefs.autostart_editable);
  // Grund, falls Qross sich nicht eintragen kann (Entwicklungsbuild, macOS-DMG, …), sonst wo der Eintrag liegt
  $("#autostartHint").textContent =
    prefs.autostart_note ?? `Qross startet bei der Anmeldung unsichtbar im Tray. Eintrag unter ${prefs.autostart_place}.`;
  $("#prefResume").checked = state.settings.resume_role;
  $("#prefAutoUpdate").checked = state.settings.auto_update;
  $("#prefGameMode").checked = state.settings.game_mode;
  $("#prefKeepSecrets").checked = !state.settings.clipboard_share_concealed;
  $("#aboutVersion").textContent = state.version;
  $("#aboutDataDir").textContent = prefs.data_dir;
  renderClipLimit();
  renderUpdateStatus();
}

/** „heute um 14:02“, „gestern um 9:15“, sonst „am 3. Oktober um 14:02“ */
function whenText(ms) {
  const d = new Date(ms);
  const time = d.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return `heute um ${time}`;
  if (d.toDateString() === yesterday.toDateString()) return `gestern um ${time}`;
  return `am ${d.toLocaleDateString("de-DE", { day: "numeric", month: "long" })} um ${time}`;
}

function updateStatusText(u) {
  const checked = u.checked_at ? `, geprüft ${whenText(u.checked_at)}` : "";
  switch (u.phase) {
    case "checking":
      return ["Suche nach Updates …", ""];
    case "current":
      return [`Qross ${u.current} ist aktuell${checked}.`, ""];
    case "downloading":
      return [`Update ${u.version} wird geladen … ${u.progress ?? 0} %`, "new"];
    case "waiting":
      return [`Update ${u.version} ist geladen und wird installiert, sobald Maus und Tastatur gerade nicht gebraucht werden.`, "new"];
    case "installing":
      return [`Update ${u.version} wird installiert, Qross startet gleich neu.`, "new"];
    case "available": {
      const prompt = u.bundle === "msi" ? " Die Installation fragt nach Administratorrechten."
        : u.bundle === "deb" || u.bundle === "rpm" ? " Die Installation fragt nach dem Passwort." : "";
      return [`Update ${u.version} ist verfügbar.${prompt}`, "new"];
    }
    case "error":
      return [`${u.message || "Prüfung fehlgeschlagen"}${u.checked_at ? `. Zuletzt erfolgreich geprüft ${whenText(u.checked_at)}.` : "."}`, "error"];
    default:
      return [u.automatic ? "Die erste Prüfung folgt kurz nach dem Start." : `Qross ${u.current}. Automatische Prüfung nur in installierten Versionen.`, ""];
  }
}

function renderUpdateStatus() {
  const u = state.prefs?.update;
  if (!u) return;
  const [text, tone] = updateStatusText(u);
  const line = $("#updateStatus");
  line.textContent = text;
  line.dataset.tone = tone;
  const busy = ["checking", "downloading", "installing"].includes(u.phase);
  $("#updateCheck").disabled = busy;
  $("#updateInstall").hidden = u.phase !== "available";
}

// ------------------------------------------------------------------ Tastatur

const KEY_GROUPS = [
  ["Buchstaben", /^Key[A-Z]$/],
  ["Ziffern", /^Digit\d$/],
  ["Zusatztasten", /^(Control|Shift|Alt|Meta)(Left|Right)$|^CapsLock$/],
  ["Funktionstasten", /^F\d+$/],
  ["Medien", /^(Audio|Media)/],
  ["Navigation", /^(Arrow|Home|End|Page|Insert|Delete)/],
  ["Ziffernblock", /^Num/],
  ["Weitere", /./],
];
const DEFAULT_LOCK = "ScrollLock";
const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);
const PRESETS = {
  "alt-meta": [["AltLeft", "MetaLeft"], ["MetaLeft", "AltLeft"], ["AltRight", "MetaRight"], ["MetaRight", "AltRight"]],
  "caps-ctrl": [["CapsLock", "ControlLeft"]],
};

async function loadKeyNames() {
  if (state.keyNames) return;
  try {
    state.keyNames = await invoke("key_names");
  } catch {
    state.keyNames = [];
  }
  state.keyLabels = new Map(state.keyNames.map((k) => [k.name, k.label]));
  const groups = KEY_GROUPS.map(([label, re]) => ({ label, re, keys: [] }));
  for (const key of state.keyNames) groups.find((g) => g.re.test(key.name)).keys.push(key);
  for (const select of [$("#addFrom"), $("#addTo")]) {
    select.replaceChildren(
      ...groups
        .filter((g) => g.keys.length)
        .map((g) => el("optgroup", { label: g.label }, g.keys.map((k) => el("option", { value: k.name, text: k.label })))),
    );
  }
  $("#addFrom").value = "CapsLock";
  $("#addTo").value = "ControlLeft";
}

const keyLabel = (name) => state.keyLabels.get(name) || name;

/** „Ctrl+Alt+KeyL“ → „Strg+Alt+L“ (Mac: Ctrl, Option, Cmd) */
function hotkeyLabel(text) {
  if (!text) return "Aus";
  const mac = state.platform === "macos";
  const names = mac ? { Ctrl: "Ctrl", Shift: "Umschalt", Alt: "Option", Meta: "Cmd" } : { Ctrl: "Strg", Shift: "Umschalt", Alt: "Alt", Meta: "Windows" };
  const parts = text.split("+");
  const key = parts.pop();
  return [...parts.map((m) => names[m] || m), keyLabel(key)].join("+");
}

function machinesForRemap() {
  const server = state.settings.server;
  return server.screens.map((s) => s.name).filter((n) => n !== server.name).sort((a, b) => a.localeCompare(b, "de"));
}

function renderKeys() {
  const keys = state.settings.server.keys;
  const isClient = state.settings.role === "client";
  $("#keysClientNote").hidden = !isClient;
  $("#keysBlock").disabled = isClient;

  const lock = $("#lockLabel");
  if (state.recording?.kind !== "lock") {
    lock.textContent = hotkeyLabel(keys.lock);
    lock.className = keys.lock ? "keycap" : "keycap off";
  }
  $("#lockDefault").disabled = keys.lock === DEFAULT_LOCK;
  $("#lockOff").disabled = !keys.lock;
  renderSwitchKeys();
  renderKeyModes();

  const media = $("#mediaMode");
  media.value = keys.media;
  const platform = state.platform;
  media.disabled = platform !== "windows";
  $("#mediaHint").textContent =
    platform === "macos" ? "Unter macOS bleiben Medientasten immer auf diesem Mac."
    : platform === "linux" ? "Unter Linux (X11) gehen Medientasten immer an den Rechner mit dem Cursor."
    : "Lautstärke, Wiedergabe/Pause, Weiter, Zurück, Stopp. „Immer auf diesem Rechner“ gibt es nur mit einem Windows-Server.";

  const machines = machinesForRemap();
  if (!machines.includes(state.remapPc)) state.remapPc = machines[0] ?? null;
  const pc = $("#remapPc");
  pc.replaceChildren(
    ...(machines.length
      ? machines.map((m) => el("option", { value: m, text: m }))
      : [el("option", { value: "", text: "Kein anderer Rechner im Layout" })]),
  );
  pc.value = state.remapPc ?? "";
  const hasPc = !!state.remapPc;
  pc.disabled = !hasPc;
  for (const b of $$(".remap-presets button, .remap-add button, .remap-add select")) b.disabled = !hasPc;

  const rules = (hasPc && keys.remap[state.remapPc]) || [];
  $("#remapList").replaceChildren(
    ...(rules.length
      ? rules.map((rule, i) =>
          el(
            "li",
            {},
            el("kbd", { class: "keycap", text: keyLabel(rule.from) }),
            el("span", { class: "arrow", "aria-hidden": "true", text: "→" }),
            el("kbd", { class: "keycap", text: keyLabel(rule.to) }),
            el(
              "button",
              {
                type: "button",
                class: "icon-btn small remove-rule",
                title: "Regel entfernen",
                "aria-label": `Regel ${keyLabel(rule.from)} → ${keyLabel(rule.to)} entfernen`,
                onclick: () => removeRule(i),
              },
              icon("x"),
            ),
          ),
        )
      : [el("li", { class: "empty", text: hasPc ? `Keine Umbelegung für „${state.remapPc}“.` : "Füge im Layout einen Rechner hinzu, um Tasten umzubelegen." })]),
  );
}

// Wechsel-Kürzel: je Richtung und je Rechner im Layout eines. Der Server prüft dieselben Regeln;
// hier werden doppelte Belegungen nur angezeigt.

const DIRECTIONS = [
  ["left", "Nach links", 180],
  ["right", "Nach rechts", 0],
  ["up", "Nach oben", -90],
  ["down", "Nach unten", 90],
];
/** Tasten, die ohne Zusatztaste zum Tippen gebraucht werden */
const TYPING_KEYS = /^(Key|Digit|Numpad|Arrow|Space$|Enter$|Tab$|Backspace$|Minus$|Equal$|Bracket|Semicolon$|Quote$|Backquote$|Comma$|Period$|Slash$|Backslash$|Intl|Home$|End$|Page|Delete$)/;

function switchKeys() {
  const keys = state.settings.server.keys;
  keys.switch ??= {};
  keys.switch.directions ??= {};
  keys.switch.screens ??= {};
  return keys.switch;
}

/** Vergleichbare Form, z. B. „Alt+Ctrl+KeyL“ und „Ctrl+Alt+KeyL“ gleich */
function hotkeyId(text) {
  if (!text) return "";
  const parts = text.split("+").map((p) => p.trim().toLowerCase());
  const key = parts.pop();
  return [...["ctrl", "shift", "alt", "meta"].filter((m) => parts.includes(m)), key].join("+");
}

function switchTargets() {
  const server = state.settings.server;
  const names = server.screens
    .map((s) => s.name)
    .sort((a, b) => (a === server.name ? -1 : b === server.name ? 1 : a.localeCompare(b, "de")));
  return [
    ...DIRECTIONS.map(([id, label, turn]) => ({ kind: "directions", id, label, turn })),
    ...names.map((name) => ({ kind: "screens", id: name, label: `Zu „${name}“`, name, isServer: name === server.name })),
  ];
}

/** Hinweis je Ziel: gleich wie die Fang-Taste, doppelt belegt oder ohne Zusatztaste */
function switchProblems(targets) {
  const keys = state.settings.server.keys;
  const all = switchKeys();
  const lock = hotkeyId(keys.lock);
  const byKey = new Map();
  for (const t of targets) {
    const id = hotkeyId(all[t.kind][t.id]);
    if (id) byKey.set(id, [...(byKey.get(id) || []), t]);
  }
  const problems = new Map();
  for (const t of targets) {
    const hotkey = all[t.kind][t.id];
    const id = hotkeyId(hotkey);
    if (!id) continue;
    const others = byKey.get(id).filter((o) => o !== t);
    if (id === lock) problems.set(t, { text: "Ist schon die Fang-Taste. Die Fang-Taste geht vor.", error: true });
    else if (others.length) problems.set(t, { text: `Auch belegt für: ${others.map((o) => o.label.replace(/^Zu /, "")).join(", ")}`, error: true });
    else if (!hotkey.includes("+") && TYPING_KEYS.test(hotkey)) problems.set(t, { text: "Ohne Zusatztaste kommt diese Taste in keinem Programm mehr an.", error: false });
  }
  return problems;
}

function renderSwitchKeys() {
  const all = switchKeys();
  const targets = switchTargets();
  const problems = switchProblems(targets);
  const recordingId = state.recording?.kind === "switch" ? state.recording.target : null;
  const focused = document.activeElement?.closest?.(".switch-list li")?.dataset.target;
  $("#switchHint").textContent =
    "Springt in die Mitte eines Rechners oder zum Nachbarn, dort an die gegenüberliegende Kante. Wirkt auf jedem Rechner und erreicht keine Programme. Hält die Fang-Taste den Cursor fest, wechselt er nicht." +
    (state.platform === "linux" ? " Belegt die Arbeitsfläche ein Kürzel schon (z. B. Strg+Alt+Pfeil), wirkt es unter X11 nur, solange der Cursor auf einem anderen Rechner ist." : "");

  const row = (t) => {
    const id = `${t.kind}:${t.id}`;
    const hotkey = all[t.kind][t.id] || "";
    const recording = recordingId === id;
    const problem = problems.get(t);
    const label = recording ? "Taste drücken …" : hotkey ? hotkeyLabel(hotkey) : "Nicht belegt";
    const capClass = ["keycap", recording ? "recording" : !hotkey && "off", problem?.error && "conflict"].filter(Boolean).join(" ");
    const arrow = t.kind === "directions" ? icon("arrow") : icon("screen");
    if (t.kind === "directions" && t.turn) arrow.style.transform = `rotate(${t.turn}deg)`;
    return el(
      "li",
      { "data-target": id, class: problem?.error ? "conflict" : "" },
      el(
        "span",
        { class: "switch-target" },
        arrow,
        el("span", { class: "name", text: t.label, title: t.label }),
        t.isServer ? el("span", { class: "server-tag", text: "Server" }) : null,
      ),
      el(
        "span",
        { class: "switch-key" },
        el("kbd", { class: capClass, text: label }),
        el("button", {
          type: "button",
          class: "btn small ghost record-switch",
          text: recording ? "Abbrechen" : hotkey ? "Ändern" : "Aufnehmen",
          "aria-label": `${recording ? "Aufnahme abbrechen" : "Kürzel aufnehmen"}: ${t.label}`,
          onclick: () => (recording ? stopRecording() : recordSwitch(t)),
        }),
        hotkey && !recording
          ? el(
              "button",
              {
                type: "button",
                class: "icon-btn small remove-key",
                title: "Kürzel entfernen",
                "aria-label": `Kürzel ${t.label} entfernen`,
                onclick: () => {
                  delete all[t.kind][t.id];
                  keysChanged();
                },
              },
              icon("x"),
            )
          : null,
      ),
      problem ? el("p", { class: `switch-problem${problem.error ? "" : " warn"}`, text: problem.text }) : null,
    );
  };
  $("#switchDirections").replaceChildren(...targets.filter((t) => t.kind === "directions").map(row));
  const screens = targets.filter((t) => t.kind === "screens");
  $("#switchScreens").replaceChildren(
    ...(screens.length ? screens.map(row) : [el("li", { class: "empty", text: "Noch kein Rechner im Layout." })]),
  );
  const keep = recordingId || focused;
  if (keep) $(`.switch-list li[data-target="${CSS.escape(keep)}"] .record-switch`)?.focus();
}

function recordSwitch(target) {
  startRecording({
    kind: "switch",
    target: `${target.kind}:${target.id}`,
    start() {
      $("#switchRecordHint").hidden = false;
      renderSwitchKeys();
    },
    end() {
      $("#switchRecordHint").hidden = true;
      renderSwitchKeys();
    },
    done(value) {
      switchKeys()[target.kind][target.id] = value;
      keysChanged();
    },
  });
}

// Tastenposition oder Zeichen je Rechner. Ob ein verbundener Rechner Zeichen eingeben kann, meldet
// der Server (Status `clients[].types_text`); sonst bekommt er trotz Einstellung Tastenpositionen.

function renderKeyModes() {
  const keys = state.settings.server.keys;
  keys.modes ??= {};
  const s = state.status;
  const clients = s?.role === "server" ? s.clients : [];
  const focused = document.activeElement?.closest?.("#modeList li")?.dataset.machine;
  $("#modeHint").textContent =
    "„Tastenposition“: die Taste an derselben Stelle, dort bestimmt das eigene Tastaturlayout das Zeichen – wie eine angeschlossene Tastatur, nötig für Spiele. " +
    "„Zeichen“: Dort kommt an, was du hier tippst, auch bei anderem Layout (z. B. hier Deutsch, dort US). Kürzel wie Strg+Z gehen nach dem Zeichen, Pfeile, F-Tasten und Co. bleiben Tasten." +
    (state.desktop === "wayland" ? " Unter Wayland kann dieser Rechner sein Tastaturlayout nicht lesen und schickt als Server immer Tastenpositionen." : "");
  const machines = machinesForRemap();
  $("#modeList").replaceChildren(
    ...(machines.length
      ? machines.map((machine) => {
          const mode = keys.modes[machine] === "chars" ? "chars" : "position";
          const client = clients.find((c) => c.name === machine);
          const note =
            mode === "chars" && client && !client.types_text
              ? `„${machine}“ kann keine Zeichen eingeben (Qross vor 0.7 oder Wayland) und bekommt Tastenpositionen.`
              : null;
          const select = el(
            "select",
            { "aria-label": `Tasten senden an ${machine}`, onchange: (e) => setKeyMode(machine, e.target.value) },
            el("option", { value: "position", text: "Tastenposition" }),
            el("option", { value: "chars", text: "Zeichen" }),
          );
          select.value = mode;
          return el(
            "li",
            { "data-machine": machine },
            el("span", { class: "switch-target" }, icon("screen"), el("span", { class: "name", text: machine, title: machine })),
            select,
            note ? el("span", { class: "hint mode-note", text: note }) : null,
          );
        })
      : [el("li", { class: "empty", text: "Füge im Layout einen Rechner hinzu, um das einzustellen." })]),
  );
  if (focused) [...$$("#modeList li")].find((li) => li.dataset.machine === focused)?.querySelector("select")?.focus();
}

function setKeyMode(machine, mode) {
  const modes = state.settings.server.keys.modes;
  if (mode === "chars") modes[machine] = "chars";
  else delete modes[machine];
  keysChanged();
}

/** Die Einstellung eines umbenannten Rechners wandert mit. */
function renameKeyMode(oldName, name) {
  const modes = (state.settings.server.keys.modes ??= {});
  if (oldName in modes) modes[name] = modes[oldName];
  delete modes[oldName];
}

/** Das Kürzel eines umbenannten Rechners wandert mit. */
function renameSwitchKey(oldName, name) {
  const screens = switchKeys().screens;
  if (oldName in screens) screens[name] = screens[oldName];
  delete screens[oldName];
}

function keysChanged() {
  renderKeys();
  settingsChanged();
}

function setRules(machine, change) {
  const remap = state.settings.server.keys.remap;
  const rules = change([...(remap[machine] || [])]);
  if (rules.length) remap[machine] = rules;
  else delete remap[machine];
  keysChanged();
}

/** Regel setzen; eine vorhandene Regel für dieselbe gedrückte Taste wird ersetzt. */
function addRules(pairs) {
  if (!state.remapPc) return;
  setRules(state.remapPc, (rules) => {
    for (const [from, to] of pairs) {
      const i = rules.findIndex((r) => r.from === from);
      if (from === to) {
        if (i >= 0) rules.splice(i, 1);
      } else if (i >= 0) rules[i] = { from, to };
      else rules.push({ from, to });
    }
    return rules;
  });
}

function removeRule(index) {
  setRules(state.remapPc, (rules) => {
    rules.splice(index, 1);
    return rules;
  });
}

/** Tastenaufnahme: fängt Tasten ab, bevor Dialog (Esc) oder Formularfelder sie sehen. */
function startRecording(recording) {
  stopRecording();
  state.recording = recording;
  recording.pressed = new Set();
  recording.keyDown = (e) => {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.code === "Escape") return stopRecording();
    if (!state.keyLabels.has(e.code)) return;
    recording.pressed.add(e.code);
    if (recording.kind === "select") return finishRecording(e.code);
    if (MODIFIER_CODES.has(e.code)) return; // Zusatztaste: bis zur eigentlichen Taste warten
    const mods = [["Ctrl", e.ctrlKey], ["Shift", e.shiftKey], ["Alt", e.altKey], ["Meta", e.metaKey]].filter(([, on]) => on).map(([m]) => m);
    finishRecording([...mods, e.code].join("+"));
  };
  recording.keyUp = (e) => {
    e.preventDefault();
    e.stopImmediatePropagation();
    // Eine Zusatztaste zählt allein, wenn sie ohne andere Taste losgelassen wird.
    const hotkey = recording.kind === "lock" || recording.kind === "switch";
    if (hotkey && MODIFIER_CODES.has(e.code) && recording.pressed.size === 1 && recording.pressed.has(e.code)) {
      return finishRecording(e.code);
    }
    recording.pressed.delete(e.code);
  };
  recording.cancel = (e) => e.preventDefault(); // Esc darf den Dialog während der Aufnahme nicht schließen
  window.addEventListener("keydown", recording.keyDown, true);
  window.addEventListener("keyup", recording.keyUp, true);
  $("#settings").addEventListener("cancel", recording.cancel);
  recording.start?.();
}

function finishRecording(value) {
  const recording = state.recording;
  stopRecording();
  recording?.done(value);
}

function stopRecording() {
  const recording = state.recording;
  if (!recording) return;
  state.recording = null;
  window.removeEventListener("keydown", recording.keyDown, true);
  window.removeEventListener("keyup", recording.keyUp, true);
  $("#settings").removeEventListener("cancel", recording.cancel);
  recording.end?.();
}

function recordLock() {
  const label = $("#lockLabel");
  const button = $("#lockChange");
  startRecording({
    kind: "lock",
    start() {
      label.textContent = "Taste drücken …";
      label.className = "keycap recording";
      button.textContent = "Abbrechen";
      $("#lockRecordHint").hidden = false;
    },
    end() {
      button.textContent = "Ändern";
      $("#lockRecordHint").hidden = true;
      renderKeys();
    },
    done(value) {
      state.settings.server.keys.lock = value;
      keysChanged();
    },
  });
}

function recordInto(selectId, button) {
  const select = $(`#${selectId}`);
  startRecording({
    kind: "select",
    start() {
      button.textContent = "Taste drücken …";
      button.classList.add("recording");
    },
    end() {
      button.textContent = "Aufnehmen";
      button.classList.remove("recording");
    },
    done(code) {
      select.value = code;
    },
  });
}

function setupKeys() {
  $("#lockChange").addEventListener("click", () => (state.recording?.kind === "lock" ? stopRecording() : recordLock()));
  $("#lockDefault").addEventListener("click", () => {
    state.settings.server.keys.lock = DEFAULT_LOCK;
    keysChanged();
  });
  $("#lockOff").addEventListener("click", () => {
    state.settings.server.keys.lock = "";
    keysChanged();
  });
  $("#mediaMode").addEventListener("change", (e) => {
    state.settings.server.keys.media = e.target.value;
    keysChanged();
  });
  $("#remapPc").addEventListener("change", (e) => {
    state.remapPc = e.target.value;
    renderKeys();
  });
  for (const b of $$("[data-preset]")) b.addEventListener("click", () => addRules(PRESETS[b.dataset.preset]));
  for (const b of $$("[data-record]")) {
    b.addEventListener("click", () => (state.recording?.kind === "select" ? stopRecording() : recordInto(b.dataset.record, b)));
  }
  $("#addRule").addEventListener("click", () => {
    const from = $("#addFrom").value;
    const to = $("#addTo").value;
    if (from === to) return toast("Gedrückte und gesendete Taste sind gleich.", "error");
    addRules([[from, to]]);
  });
  $("#settings").addEventListener("close", stopRecording);
}

function setupSettings() {
  setupKeys();
  $("#settingsBtn").addEventListener("click", () => openSettings());
  setupSheet($("#settings"), $("#settingsClose"));
  for (const b of $$(".settings-nav button")) b.addEventListener("click", () => showSection(b.dataset.section));
  $(".settings-nav").addEventListener("keydown", onSettingsNavKey);

  $("#prefAutostart").addEventListener("change", async (e) => {
    try {
      state.prefs.autostart = await invoke("set_autostart", { enabled: e.target.checked });
    } catch (err) {
      toast(String(err), "error");
      await refreshPrefs();
    }
    renderSettings();
  });
  $("#prefResume").addEventListener("change", (e) => {
    state.settings.resume_role = e.target.checked;
    saveSoon.flush();
  });
  $("#prefAutoUpdate").addEventListener("change", (e) => {
    state.settings.auto_update = e.target.checked;
    saveSoon.flush();
  });
  $("#prefGameMode").addEventListener("change", (e) => {
    state.settings.game_mode = e.target.checked;
    saveSoon.flush();
  });
  $("#prefKeepSecrets").addEventListener("change", (e) => {
    state.settings.clipboard_share_concealed = !e.target.checked;
    updatePreview();
    saveSoon.flush();
  });
  $("#updateCheck").addEventListener("click", () => invoke("check_updates"));
  $("#updateInstall").addEventListener("click", () => invoke("install_update"));
  $("#openDataDir").addEventListener("click", () => invoke("open_data_dir").catch((e) => toast(String(e), "error")));
  $("#aboutProject").addEventListener("click", () => invoke("open_project_page").catch((e) => toast(String(e), "error")));
  $("#aboutNews").addEventListener("click", () => {
    $("#settings").close();
    openNews();
  });
}

// ------------------------------------------------------------------ Profile je Ort

/** Abfrage der Netzwerke, solange die Einstellungen offen sind */
const NETWORK_REFRESH_MS = 5000;
let networkTimer = null;

/** Dieselbe Regel wie `NetworkMark::matches`: Kennen beide die MAC des Gateways, zählt nur sie. */
function netMatches(stored, current) {
  if (!stored.gateway || !current) return false;
  if (stored.gateway_mac && current.gateway_mac) return stored.gateway_mac.toLowerCase() === current.gateway_mac.toLowerCase();
  const sameSubnet = !stored.subnet || !current.subnet || stored.subnet === current.subnet;
  return stored.gateway === current.gateway && sameSubnet;
}

function activeProfile() {
  return state.profiles?.list.find((p) => p.id === state.profiles.active) ?? null;
}

/** „Server · 3 Bildschirme“ bzw. „Client · verbindet mit …“; beim aktiven Profil wie gerade eingestellt */
function profileSummary(profile) {
  const place = profile.id === state.profiles.active ? state.settings : profile.place;
  if (place.role === "client") {
    const address = place.client.address.trim();
    return address ? `Client · verbindet mit ${address}` : "Client · noch ohne Server-Adresse";
  }
  const count = place.server.screens.length;
  return count === 1 ? "Server · nur dieser Rechner" : `Server · ${count} Bildschirme`;
}

function netDetails(n) {
  const mac = n.gateway_mac ? `MAC ${n.gateway_mac}` : "MAC unbekannt";
  return [`Gateway ${n.gateway}`, mac, n.subnet && `Subnetz ${n.subnet}`, n.interface && `über ${n.interface}`].filter(Boolean).join(" · ");
}

/** Auswahl in der Kopfzeile */
function renderProfilePick() {
  const profiles = state.profiles;
  if (!profiles) return;
  const select = $("#profileSelect");
  select.replaceChildren(
    ...profiles.list.map((p) => el("option", { value: p.id, text: p.name })),
    el("option", { value: "", disabled: true, text: "──────────" }),
    el("option", { value: "manage", text: "Profile verwalten …" }),
  );
  select.value = profiles.active;
  const active = activeProfile();
  const auto = profiles.auto && profiles.list.some((p) => p.auto && p.networks.length);
  $("#profilePick").title = active
    ? `Profil „${active.name}“: ${profileSummary(active)}${auto ? ". Wird nach Netzwerk gewählt." : ""}`
    : "Profil";
}

/** Wechselt das Profil nach Rückfrage, falls es die Rolle wechselt, während Qross läuft. */
async function pickProfile(id) {
  if (!id || id === state.profiles.active) return;
  const role = state.profiles.list.find((p) => p.id === id)?.place.role;
  if (await confirmRoleChange(role)) await activateProfile(id);
}

/** Wechselt das Profil; als Client fragt danach womöglich der Fingerprint-Dialog. */
async function activateProfile(id) {
  await saveSoon.flush();
  try {
    const probe = await invoke("activate_profile", { id });
    if (probe) await startFlow();
  } catch (e) {
    toast(String(e), "error");
  }
}

async function editProfiles(edit) {
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

async function refreshNetworks() {
  try {
    state.networks = await invoke("current_networks");
  } catch {
    state.networks = [];
  }
  renderProfiles();
}

function watchNetworks(on) {
  clearInterval(networkTimer);
  networkTimer = on ? setInterval(refreshNetworks, NETWORK_REFRESH_MS) : null;
  if (on) refreshNetworks();
}

function renderNetNow() {
  const box = $("#netNow");
  const nets = state.networks;
  if (nets == null) {
    box.replaceChildren(el("span", { class: "hint", text: "Netzwerk wird ermittelt …" }));
    return;
  }
  if (!nets.length) {
    box.replaceChildren(icon("wifi"), el("span", { class: "net-text", text: "Kein Netzwerk mit Router erkannt. Ohne Netzwerk bleibt das Profil, wie es ist." }));
    return;
  }
  const [first, ...others] = nets;
  const owner = state.profiles.list.find((p) => p.networks.some((n) => netMatches(n, first)));
  box.replaceChildren(
    icon("wifi"),
    el(
      "span",
      { class: "net-text" },
      el("span", { class: "net-title" }, "Aktuelles Netzwerk: ", el("strong", { text: first.label })),
      el("span", { class: "hint", text: netDetails(first) }),
      el("span", { class: "hint", text: owner ? `Gehört zum Profil „${owner.name}“.` : "Noch keinem Profil zugeordnet." }),
      others.length ? el("span", { class: "hint", text: `Außerdem: ${others.map((n) => `${n.label} (${n.interface})`).join(", ")}` }) : null,
    ),
  );
}

function profileCard(profile) {
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
          title: "Zuordnung entfernen",
          "aria-label": `Netzwerk „${n.label}“ von „${name}“ entfernen`,
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
      title: current ? `${current.label}: ${netDetails(current)}` : "Kein Netzwerk erkannt",
      onclick: () => editProfiles({ action: "add_network", id: profile.id, network: current }),
    },
    icon("plus"),
    "Aktuelles Netzwerk übernehmen",
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
        active ? el("span", { class: "profile-badge", text: "aktiv" }) : null,
        profiles.fallback === profile.id
          ? el("span", { class: "profile-badge other", text: "andere Netzwerke", title: "Gilt, wenn kein Profil zum Netzwerk passt" })
          : null,
      ),
      el(
        "span",
        { class: "profile-actions" },
        active ? null : el("button", { type: "button", class: "btn small", text: "Aktivieren", onclick: () => activateProfile(profile.id) }),
        action("Umbenennen", "pencil", () => renameProfile(profile)),
        action("Duplizieren", "copy", () => editProfiles({ action: "duplicate", id: profile.id })),
        action("Löschen", "trash", () => removeProfile(profile), {
          disabled: active,
          title: active ? "Das aktive Profil lässt sich nicht löschen" : "Löschen",
        }),
      ),
      el("span", { class: "profile-summary", text: profileSummary(profile) }),
    ),
    el(
      "div",
      { class: "profile-nets" },
      chips.length ? chips : el("span", { class: "hint", text: "Kein Netzwerk zugeordnet, nur von Hand wählbar." }),
      hasCurrent ? null : take,
    ),
    profile.networks.length
      ? el(
          "label",
          { class: "switch profile-auto" },
          auto,
          el("span", { class: "track" }, el("span", { class: "thumb" })),
          el("span", { text: "In diesen Netzwerken automatisch aktivieren" }),
        )
      : null,
  );
}

function renderProfiles() {
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
    el("option", { value: "", text: "Aktuelles Profil behalten" }),
    ...profiles.list.map((p) => el("option", { value: p.id, text: `„${p.name}“ aktivieren` })),
  );
  fallback.value = profiles.fallback;
  $("#netHint").hidden = state.platform !== "macos";
}

const PROFILE_NAME_HINT = "Zum Beispiel der Ort, an dem dieses Profil gilt.";

async function newProfile() {
  const text = `${PROFILE_NAME_HINT} Es beginnt mit diesem Rechner allein als Server. Um die aktuellen Werte zu übernehmen, ein Profil duplizieren.`;
  const name = await askName("Neues Profil", "", text);
  if (name) await editProfiles({ action: "create", name });
}

async function renameProfile(profile) {
  const name = await askName("Profil umbenennen", profile.name, PROFILE_NAME_HINT);
  if (name && name !== profile.name) await editProfiles({ action: "rename", id: profile.id, name });
}

async function removeProfile(profile) {
  const result = await showDialog({
    title: `Profil „${profile.name}“ löschen?`,
    body: el("p", { text: "Anordnung, Adressen und zugeordnete Netzwerke dieses Profils gehen verloren." }),
    actions: [
      { label: "Abbrechen", value: "cancel", kind: "ghost" },
      { label: "Löschen", value: "remove", kind: "danger" },
    ],
  });
  if (result === "remove") await editProfiles({ action: "remove", id: profile.id });
}

function setupProfiles() {
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

// ------------------------------------------------------------------ Verbindungsqualität

/** Länge des Verlaufs in Sekunden (ein Messpunkt pro Sekunde) */
const QUALITY_SECONDS = 60;
/** Stufe → gefüllte Balken; die Farbe kommt per CSS über `data-level`. */
const QUALITY_BARS = { very_good: 4, good: 3, fair: 2, poor: 1 };
/** Einzelwerte, die die Stufe bestimmen (Felder von `rating`) */
const QUALITY_METRICS = { rtt: "Latenz", jitter: "Jitter", loss: "Verlust" };
const QUALITY_LIMITS =
  "Sehr gut: unter 10 ms Latenz, 3 ms Jitter und 0,5 % Verlust. Gut: unter 30 ms, 10 ms und 2 %. Mäßig: unter 80 ms, 25 ms und 5 %. Es zählt der schlechteste Wert.";
/** Je Verbindung ("server" bzw. "client:Name"): Messpunkte der letzten Minute und die Anzeige dazu */
const qualities = new Map();
const SVG_NS = "http://www.w3.org/2000/svg";
const NBSP = " ";
const CHART = { w: 240, h: 48 };
let qualityIds = 0;

function svgEl(tag, attrs = {}) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
  return node;
}

function decimal(value, digits) {
  return value.toLocaleString("de-DE", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

function formatMs(ms) {
  if (ms < 0.1) return `<${NBSP}0,1${NBSP}ms`;
  return `${decimal(ms, ms < 10 ? 1 : 0)}${NBSP}ms`;
}

function formatPct(pct) {
  if (pct === 0) return `0${NBSP}%`;
  if (pct < 0.1) return `<${NBSP}0,1${NBSP}%`;
  return `${decimal(pct, pct < 10 ? 1 : 0)}${NBSP}%`;
}

/** Bytes pro Sekunde, in denselben Einheiten wie das Protokoll (1 KB = 1000 B) */
function formatRate(bytes) {
  if (bytes < 1000) return `${bytes}${NBSP}B/s`;
  const units = ["KB/s", "MB/s", "GB/s"];
  let value = bytes / 1000;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return `${decimal(value, value < 10 ? 1 : 0)}${NBSP}${units[unit]}`;
}

/** Obergrenze der Skala (6, 8, 10, 15, 20, 30 … ms), mit etwas Luft über dem höchsten Wert */
function chartMax(values) {
  const target = Math.max(5, ...values) * 1.15;
  const power = 10 ** Math.floor(Math.log10(target));
  return [1, 1.5, 2, 3, 4, 5, 6, 8, 10].map((m) => m * power).find((v) => v >= target);
}

/** Pfeil für gesendet (hoch) bzw. empfangen (runter); Unicode-Pfeile erscheinen je nach Schrift farbig. */
function rateArrow(up) {
  const node = svgEl("svg", { class: "qarrow", viewBox: "0 0 12 12", "aria-hidden": "true" });
  node.append(svgEl("path", { d: up ? "M6 10.5V2M2.5 5.5L6 2l3.5 3.5" : "M6 1.5V10M2.5 6.5L6 10l3.5-3.5" }));
  return node;
}

function qualityBars() {
  const node = svgEl("svg", { class: "qbars", viewBox: "0 0 16 14", "aria-hidden": "true" });
  for (let i = 0; i < 4; i++) node.append(svgEl("rect", { x: i * 4 + 0.5, y: 10 - i * 3, width: 3, height: 4 + i * 3, rx: 1 }));
  return node;
}

/** Anzeige einer Verbindung: kompakte Plakette (Stufe + ms) und aufklappbare Details. Die Knoten
 *  bleiben bestehen und werden nur aktualisiert, damit nichts flackert und Aufgeklapptes offen bleibt. */
function qualityEntry(key) {
  let entry = qualities.get(key);
  if (entry) return entry;
  const id = `quality-${++qualityIds}`;
  entry = {
    samples: [],
    /** Spalte (0–59) unter dem Mauszeiger im Verlauf */
    hover: null,
    max: 10,
    pillBars: qualityBars(),
    pillText: el("span", { class: "qms" }),
    headBars: qualityBars(),
    level: el("strong", { class: "qlevel", title: QUALITY_LIMITS }),
    reason: el("span", { class: "qreason" }),
    scale: el("span", { class: "qscale" }),
    line: svgEl("path", { class: "qline" }),
    area: svgEl("path", { class: "qarea" }),
    cross: el("div", { class: "qcross", hidden: true }),
    dot: el("div", { class: "qdot", hidden: true }),
    tip: el("div", { class: "qtip", hidden: true }),
    values: {},
    /** Letztes Ergebnis der Messung der Eingabekette (Ereignis `latency`) */
    latency: null,
  };
  const plot = svgEl("svg", { viewBox: `0 0 ${CHART.w} ${CHART.h}`, preserveAspectRatio: "none", "aria-hidden": "true" });
  plot.append(
    svgEl("line", { class: "qgrid", x1: 0, y1: 0.5, x2: CHART.w, y2: 0.5 }),
    svgEl("line", { class: "qgrid base", x1: 0, y1: CHART.h - 0.5, x2: CHART.w, y2: CHART.h - 0.5 }),
    entry.area,
    entry.line,
  );
  entry.chart = el("div", { class: "qchart", role: "img" }, plot, entry.scale, entry.cross, entry.dot, entry.tip);
  entry.chart.addEventListener("pointermove", (e) => {
    const rect = entry.chart.getBoundingClientRect();
    const column = Math.round(((e.clientX - rect.left) / rect.width) * (QUALITY_SECONDS - 1));
    entry.hover = Math.max(0, Math.min(QUALITY_SECONDS - 1, column));
    showQualityHover(entry);
  });
  entry.chart.addEventListener("pointerleave", () => {
    entry.hover = null;
    showQualityHover(entry);
  });

  const stat = (name, label) => {
    entry.values[name] = el("span", { class: "v" });
    entry.values[`${name}Sub`] = el("span", { class: "s" });
    return el("div", { class: `qstat ${name}` }, el("span", { class: "k", text: label }), entry.values[name], entry.values[`${name}Sub`]);
  };
  entry.values.rate = el("span");
  entry.values.rateSub = el("span");
  const rate = el(
    "div",
    { class: "qstat rate" },
    el("span", { class: "k", text: "Durchsatz" }),
    el("span", { class: "v", title: "gesendet" }, rateArrow(true), entry.values.rate),
    el("span", { class: "v", title: "empfangen" }, rateArrow(false), entry.values.rateSub),
  );
  entry.panel = el(
    "div",
    { class: "qpanel", id, hidden: true },
    el("div", { class: "qhead" }, entry.headBars, entry.level, entry.reason),
    el("div", {}, entry.chart, el("div", { class: "qaxis" }, el("span", { text: "Latenz der letzten 60 s" }), el("span", { text: "jetzt" }))),
    el("div", { class: "qstats" }, stat("rtt", "Latenz"), stat("jitter", "Jitter"), stat("loss", "Verlust"), rate),
    el("p", { class: "hint", text: "Aus den Bestätigungen der QUIC-Verbindung, ohne zusätzlichen Netzverkehr. Der Verlust gilt für die Senderichtung dieses Rechners." }),
    chainBlock(entry, key.startsWith("client:") ? key.slice("client:".length) : null),
  );
  entry.pill = el("button", { type: "button", class: "qpill", "aria-expanded": "false", "aria-controls": id }, entry.pillBars, entry.pillText);
  entry.pill.addEventListener("click", () => {
    entry.panel.hidden = !entry.panel.hidden;
    entry.pill.setAttribute("aria-expanded", String(!entry.panel.hidden));
    updateQuality(entry);
  });
  qualities.set(key, entry);
  updateQuality(entry);
  return entry;
}

function updateQuality(entry) {
  const last = entry.samples.at(-1);
  const bars = last ? QUALITY_BARS[last.level] ?? 0 : 0;
  for (const node of [entry.pill, entry.panel]) node.dataset.level = last ? last.level : "none";
  for (const svg of [entry.pillBars, entry.headBars]) {
    [...svg.children].forEach((rect, i) => rect.classList.toggle("on", i < bars));
  }
  const open = !entry.panel.hidden;
  entry.pillText.textContent = last ? formatMs(last.rtt_ms) : "misst …";
  entry.pill.title = last ? `Verbindung ${last.label}: ${open ? "Details ausblenden" : "Details anzeigen"}` : "Die Verbindungsqualität wird gemessen";
  entry.pill.setAttribute("aria-label", last ? `Verbindung ${last.label}, Latenz ${formatMs(last.rtt_ms)}` : "Verbindungsqualität wird gemessen");
  if (!open) return;

  const v = entry.values;
  entry.level.textContent = last ? last.label : "Wird gemessen …";
  // Welche Werte die Stufe bestimmen, z. B. „wegen Latenz und Verlust“
  const reasons =
    last && last.level !== "very_good"
      ? Object.keys(QUALITY_METRICS).filter((k) => last.rating?.[k] === last.level).map((k) => QUALITY_METRICS[k])
      : [];
  entry.reason.textContent = reasons.length ? `wegen ${reasons.join(", ").replace(/, ([^,]*)$/, " und $1")}` : "";
  v.rtt.textContent = last ? formatMs(last.rtt_ms) : "–";
  v.rttSub.textContent = "hin und zurück";
  v.jitter.textContent = last ? formatMs(last.jitter_ms) : "–";
  v.jitterSub.textContent = "Schwankung, 10 s";
  v.loss.textContent = last ? formatPct(last.loss_pct) : "–";
  v.lossSub.textContent = last && last.loss_window_s ? `über ${last.loss_window_s} s` : "";
  v.rate.textContent = last ? formatRate(last.tx_bytes_per_s) : "–";
  v.rateSub.textContent = last ? formatRate(last.rx_bytes_per_s) : "–";
  drawQualityChart(entry);
  updateLatency(entry);
}

function qualityY(entry, ms) {
  return CHART.h - 1 - (Math.min(ms, entry.max) / entry.max) * (CHART.h - 4);
}

function drawQualityChart(entry) {
  const values = entry.samples.map((q) => q.rtt_ms);
  const n = values.length;
  entry.max = chartMax(values);
  entry.scale.textContent = `${decimal(entry.max, 0)}${NBSP}ms`;
  if (!n) {
    entry.line.setAttribute("d", "");
    entry.area.setAttribute("d", "");
    entry.chart.setAttribute("aria-label", "Noch kein Verlauf");
    return;
  }
  const step = CHART.w / (QUALITY_SECONDS - 1);
  const points = values.map((ms, i) => [(QUALITY_SECONDS - n + i) * step, qualityY(entry, ms)]);
  if (n === 1) points.unshift([points[0][0] - step, points[0][1]]);
  const d = points.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`).join("");
  entry.line.setAttribute("d", d);
  entry.area.setAttribute("d", `${d}L${points.at(-1)[0].toFixed(1)} ${CHART.h}L${points[0][0].toFixed(1)} ${CHART.h}Z`);
  entry.chart.setAttribute(
    "aria-label",
    `Latenz der letzten ${n} Sekunden: zwischen ${formatMs(Math.min(...values))} und ${formatMs(Math.max(...values))}`,
  );
  showQualityHover(entry);
}

/** Fadenkreuz und Wert unter dem Mauszeiger */
function showQualityHover(entry) {
  const n = entry.samples.length;
  const i = entry.hover == null ? -1 : entry.hover - (QUALITY_SECONDS - n);
  const hide = i < 0 || i >= n;
  entry.cross.hidden = entry.dot.hidden = entry.tip.hidden = hide;
  if (hide) return;
  const q = entry.samples[i];
  const left = (entry.hover / (QUALITY_SECONDS - 1)) * 100;
  entry.cross.style.left = `${left}%`;
  entry.dot.style.left = `${left}%`;
  entry.dot.style.top = `${(qualityY(entry, q.rtt_ms) / CHART.h) * 100}%`;
  const ago = n - 1 - i;
  const parts = [ago ? `vor ${ago}${NBSP}s` : "jetzt", formatMs(q.rtt_ms), `Jitter ${formatMs(q.jitter_ms)}`];
  if (q.loss_pct > 0) parts.push(`Verlust ${formatPct(q.loss_pct)}`);
  entry.tip.textContent = parts.join(" · ");
  // Innerhalb des Diagramms halten
  const width = entry.chart.clientWidth;
  const half = entry.tip.offsetWidth / 2;
  const x = Math.max(half, Math.min(width - half, (left / 100) * width));
  entry.tip.style.left = `${x}px`;
}

function onQuality(q) {
  const s = state.status;
  const connected =
    q.peer == null ? s.role === "client" && s.phase === "connected" : s.role === "server" && s.clients.some((c) => c.name === q.peer);
  if (!connected) return;
  const entry = qualityEntry(q.peer == null ? "server" : `client:${q.peer}`);
  entry.samples.push(q);
  if (entry.samples.length > QUALITY_SECONDS) entry.samples.shift();
  updateQuality(entry);
}

// ------------------------------------------------------------------ Verzögerung der Eingabekette

/** Teile der Kette in der Reihenfolge, in der eine Eingabe sie durchläuft */
const CHAIN_PARTS = [
  { key: "server", label: "Server", detail: "Erfassung bis Versand" },
  { key: "network", label: "Netz", detail: "Versand bis Empfang beim Client" },
  { key: "client", label: "Client", detail: "Empfang bis eingespeist" },
];
const CHAIN_KINDS = [
  { key: "mouse", label: "Maus", none: "noch keine Mausbewegung", what: "Mausbewegungen, jede 32." },
  { key: "keys", label: "Tasten", none: "noch kein Tastendruck", what: "Tastendrücke und Klicks" },
];
const CHAIN_HELP = [
  "Wie lange eine Eingabe braucht: vom Abholen aus der Erfassung am Server, bis das Einspeisen am Client erledigt ist. Groß: Median, klein: 95. Perzentil der letzten Minute.",
  "Gemessen an Stichproben (jede 32. Mausbewegung, jeder Tastendruck und Klick), ohne zusätzliche Daten auf dem Eingabeweg.",
  "Server: Erfassung bis Versand. Netz: bis der Client die Eingabe gelesen hat. Client: bis sie eingespeist ist. Der Balken zeigt die Anteile am Median.",
  "Server und Client messen mit ihrer eigenen Uhr. Den Netzanteil schätzt Qross wie NTP aus den Rückmeldungen des Clients: Er stimmt, wenn Hin- und Rückweg gleich lang dauern, und liegt sonst höchstens um die angegebene Unsicherheit daneben.",
  "Nicht enthalten: die Zeit im Betriebssystem vor dem Hook und nach dem Einspeisen bis zum Bildschirm.",
].join("\n\n");

/** Mikrosekunden als Millisekunden, fein genug für Loopback (0,04) */
function chainNumber(us) {
  const ms = us / 1000;
  if (ms < 0.01) return `<${NBSP}0,01`;
  return decimal(ms, ms < 1 ? 2 : ms < 10 ? 1 : 0);
}

function chainMs(us) {
  return `${chainNumber(us)}${NBSP}ms`;
}

/** Abschnitt „Verzögerung der Eingabe“ im aufgeklappten Panel einer Verbindung */
function chainBlock(entry, peer) {
  const block = { peer, rows: {} };
  block.accuracy = el("p", { class: "qchain-note" });
  block.empty = el("p", { class: "qchain-empty" });
  block.note = el("p", { class: "qchain-note", hidden: true });
  const rows = CHAIN_KINDS.map((kind) => {
    const row = {
      value: el("span", { class: "v" }),
      p95: el("span", { class: "s" }),
      segs: Object.fromEntries(CHAIN_PARTS.map((p) => [p.key, el("i", { class: p.key })])),
      parts: Object.fromEntries(CHAIN_PARTS.map((p) => [p.key, el("span", { class: p.key })])),
    };
    row.bar = el("div", { class: "qchain-bar", "aria-hidden": "true" }, ...Object.values(row.segs));
    row.node = el(
      "div",
      { class: "qchain-row", tabindex: "0" },
      el("span", { class: "k", text: kind.label }),
      el("div", { class: "line" }, row.value, row.p95),
      row.bar,
      el("div", { class: "qchain-parts" }, ...Object.values(row.parts)),
    );
    block.rows[kind.key] = row;
    return row.node;
  });
  block.list = el("div", { class: "qchain-rows" }, ...rows);
  block.node = el(
    "div",
    { class: "qchain" },
    el(
      "div",
      { class: "qchain-head" },
      el("span", { text: "Verzögerung der Eingabe" }),
      el("span", { class: "qinfo", tabindex: "0", role: "img", "aria-label": CHAIN_HELP, title: CHAIN_HELP, text: "?" }),
    ),
    block.empty,
    block.list,
    block.accuracy,
    block.note,
  );
  entry.chain = block;
  return block.node;
}

function updateLatency(entry) {
  const block = entry.chain;
  const latency = entry.latency;
  const any = latency && (latency.mouse || latency.keys);
  // Am Client erst, wenn der Server misst (ältere Server melden nichts); am Server immer.
  block.node.hidden = !latency && !block.peer;
  block.empty.hidden = Boolean(any);
  block.list.hidden = !any;
  block.empty.textContent = block.peer
    ? `Noch keine Eingaben gemessen. Sobald der Cursor auf „${block.peer}“ ist, misst Qross nebenbei mit.`
    : "Noch keine Eingaben gemessen.";
  const reports = !latency || latency.client_reports;
  block.note.hidden = !any || reports;
  block.note.textContent = `„${block.peer}“ meldet Netz- und Client-Anteil nicht (ältere Qross-Version); zu sehen ist nur der Server-Anteil.`;
  const uncertain = any && latency.uncertainty_us != null;
  block.accuracy.hidden = !uncertain;
  block.accuracy.textContent = uncertain ? `Netzanteil auf ±${NBSP}${chainMs(latency.uncertainty_us)} genau (Uhrenvergleich wie NTP)` : "";
  if (!any) return;
  for (const kind of CHAIN_KINDS) updateChainRow(block.rows[kind.key], kind, latency[kind.key], latency.uncertainty_us);
}

function updateChainRow(row, kind, chain, uncertainty) {
  const full = Boolean(chain?.total);
  if (!chain) {
    row.value.textContent = "–";
    row.p95.textContent = kind.none;
  } else if (full) {
    row.value.textContent = chainMs(chain.total.p50_us);
    row.p95.textContent = `95${NBSP}%: ${chainMs(chain.total.p95_us)}`;
  } else {
    row.value.textContent = chainMs(chain.server.p50_us);
    row.p95.textContent = `95${NBSP}%: ${chainMs(chain.server.p95_us)}, nur Server`;
  }
  row.bar.hidden = !chain;
  for (const part of CHAIN_PARTS) {
    const spread = chain?.[part.key];
    const seg = row.segs[part.key];
    seg.hidden = !spread;
    if (spread) seg.style.flexGrow = String(Math.max(spread.p50_us, 1));
    row.parts[part.key].hidden = !spread;
    row.parts[part.key].textContent = spread ? `${part.label} ${chainNumber(spread.p50_us)}` : "";
  }
  if (!chain) {
    row.node.title = `${kind.label}: ${kind.none}`;
    row.node.setAttribute("aria-label", row.node.title);
    return;
  }
  const lines = [`${kind.label} (${kind.what}), letzte Minute: ${full ? chain.reported : chain.samples} Stichproben`];
  if (full) lines.push(`Gesamt: Median ${chainMs(chain.total.p50_us)}, 95${NBSP}% ${chainMs(chain.total.p95_us)}`);
  for (const part of CHAIN_PARTS) {
    const spread = chain[part.key];
    if (!spread) continue;
    const plusMinus = part.key === "network" && uncertainty != null ? ` (±${NBSP}${chainMs(uncertainty)})` : "";
    lines.push(`${part.label}, ${part.detail}: ${chainMs(spread.p50_us)} / ${chainMs(spread.p95_us)}${plusMinus}`);
  }
  if (!full) lines.push("Netz und Client: meldet erst ein neueres Qross auf dem Client");
  row.node.title = lines.join("\n");
  row.node.setAttribute("aria-label", lines.join(". "));
}

function onLatency(latency) {
  const s = state.status;
  const peer = latency.peer;
  const connected =
    peer == null ? s.role === "client" && s.phase === "connected" : s.role === "server" && s.clients.some((c) => c.name === peer);
  if (!connected) return;
  const entry = qualityEntry(peer == null ? "server" : `client:${peer}`);
  entry.latency = latency;
  if (!entry.panel.hidden) updateLatency(entry);
}

/** Verläufe beendeter Verbindungen verwerfen; eine neue Verbindung beginnt von vorn. */
function pruneQualities() {
  const s = state.status;
  const keep = new Set();
  if (s.role === "server") for (const c of s.clients) keep.add(`client:${c.name}`);
  if (s.role === "client" && s.phase === "connected") keep.add("server");
  for (const key of qualities.keys()) if (!keep.has(key)) qualities.delete(key);
}

// ------------------------------------------------------------------ Rechte (macOS)

const PERMISSIONS = {
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
let permissionTimer = null;

function missingPermissions() {
  const granted = state.permissions;
  if (!granted) return [];
  return Object.keys(PERMISSIONS).filter(
    (key) => !granted[key] && !(PERMISSIONS[key].serverOnly && state.settings.role !== "server"),
  );
}

function renderPermissions() {
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

async function checkPermissions() {
  const wasMissing = missingPermissions().length > 0;
  try {
    state.permissions = await invoke("permissions");
  } catch {
    return;
  }
  renderPermissions();
  if (wasMissing && !missingPermissions().length) toast("macOS hat Qross freigegeben.", "success");
}

async function openPermission(key) {
  try {
    await invoke("open_permission_settings", { permission: key });
  } catch (e) {
    toast(`Systemeinstellungen nicht geöffnet: ${e}`, "error");
  }
}

// ------------------------------------------------------------------ Wayland (Desktop-Portale)

let portalRequest = null;

/** Stand der Portale neu holen (höchstens eine Anfrage zugleich) */
function refreshPortal() {
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
function portalHint() {
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

function renderPortal() {
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

async function forgetPortal() {
  try {
    await invoke("forget_portal_permissions");
    toast("Beim nächsten Start fragt das System wieder nach der Freigabe.", "success");
  } catch (e) {
    toast(`Nicht gelöscht: ${e}`, "error");
  }
  refreshPortal();
}

// ------------------------------------------------------------------ Gesamt

function render() {
  pruneQualities();
  renderPermissions();
  renderPortal();
  renderTopbar();
  renderProfilePick();
  renderGameNote();
  renderServer();
  renderClients();
  renderClient();
  // Verbundene Rechner melden, ob sie Zeichen eingeben können
  if ($("#settings").open && state.settings) renderKeyModes();
}

async function init() {
  // Vor get_state anmelden: Wechselt das Profil schon beim Start (Netzwerk), steckt das entweder im
  // Abbild oder kommt als Ereignis. Bis zum Abbild gibt es nichts zu aktualisieren.
  await listen("settings", (e) => {
    if (!state.settings) return;
    state.settings = e.payload;
    render();
    updatePreview();
    renderProfiles();
  });
  await listen("profiles", (e) => {
    if (!state.profiles) return;
    state.profiles = e.payload;
    renderProfilePick();
    renderProfiles();
  });
  const snap = await invoke("get_state");
  Object.assign(state, {
    settings: snap.settings,
    profiles: snap.profiles,
    status: snap.status,
    fingerprint: snap.fingerprint,
    hostname: snap.hostname,
    localIp: snap.local_ip,
    grid: snap.grid,
    platform: snap.platform,
    desktop: snap.desktop ?? state.desktop,
    version: snap.version,
  });
  for (const node of $$("[data-own-fingerprint]")) fillFingerprint(node, snap.fingerprint);
  for (const node of $$("[data-data-dir]")) node.textContent = snap.data_dir;
  $("#clientName").placeholder = snap.hostname;
  appendLogs(snap.logs);
  setLogOpen(store("qross.logOpen") === "1");
  if (state.platform === "macos") {
    state.permissions = await invoke("permissions");
    // Zurück aus den Systemeinstellungen: sofort neu prüfen
    window.addEventListener("focus", checkPermissions);
  }
  render();
  updatePreview();
  refreshPortal();
  $("#portalForget").addEventListener("click", forgetPortal);

  await listen("status", (e) => {
    const before = state.status;
    state.status = e.payload;
    if (!running() || state.status.phase === "starting") clearTransfers();
    render();
    // Unter Wayland nach jedem Phasenwechsel (Start, Fehler, verbunden) die Portale neu ansehen
    if (before?.phase !== e.payload.phase || before?.role !== e.payload.role) refreshPortal();
  });
  await listen("log", (e) => appendLogs([e.payload]));
  await listen("quality", (e) => onQuality(e.payload));
  await listen("latency", (e) => onLatency(e.payload));
  await listen("transfer", (e) => onTransfer(e.payload));
  await listen("start-from-tray", () => {
    if (!running()) startFlow();
  });
  // Profil aus dem Tray, das die Rolle wechseln würde: erst hier nachfragen
  await listen("confirm-profile", (e) => pickProfile(e.payload));
  await listen("show-news", () => openNews());
  await listen("show-settings", (e) => openSettings(e.payload || undefined));
  await listen("update-status", (e) => {
    if (state.prefs) state.prefs.update = e.payload;
    renderUpdateStatus();
  });
  // Hinweise aus dem Backend (z. B. „Update verfügbar“); Fehler kommen nie als Hinweis.
  await listen("app-notice", (e) => {
    const { tone, text, action } = e.payload;
    const button = action === "install-update" ? { label: "Jetzt aktualisieren", run: () => invoke("install_update") } : null;
    toast(text, TOAST_KIND[tone] ?? "", button);
  });
  setupSettings();
  setupProfiles();
  // Kopierte Dateien: dezente Hinweise im Fenster (Texte kommen fertig aus dem Backend)
  await listen("clipboard-files", (e) => onClipboardFiles(e.payload));
  setupClipboardFiles();
  setupNews();

  for (const b of $$(".segmented button")) {
    b.addEventListener("click", async () => {
      if (state.settings.role === b.dataset.role) return;
      if (!(await confirmRoleChange(b.dataset.role))) return;
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
