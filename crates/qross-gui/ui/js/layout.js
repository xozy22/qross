// Layout-Editor

import { $, DEFAULT_PORT, state } from "./core.js";
import { copy, el, icon, toast } from "./util.js";
import { askName } from "./dialog.js";
import { settingsChanged } from "./store.js";
import { openSharing, renameSharing, renderSharing, sharingOf, sharingSummary } from "./sharing.js";
import { renderClipLimit } from "./clipboard-files.js";
import { hotkeyLabel, loadKeyNames, renameKeyMode, renameSwitchKey } from "./keys.js";

/** Monitore eines Rechners, sofern es mehrere sind (sonst null: Darstellung wie bisher) */
export function multipleMonitors(screen) {
  return screen?.monitors?.length > 1 ? screen.monitors : null;
}

/** Zusatz für Beschriftungen, z. B. „ · 2 Monitore“ */
export function monitorNote(screen) {
  const monitors = multipleMonitors(screen);
  return monitors ? ` · ${monitors.length} Monitore` : "";
}

/** Tooltip: Name, dann jeder Monitor von links nach rechts und die Gesamtauflösung */
export function screenTitle(name, screen) {
  const monitors = multipleMonitors(screen);
  if (!monitors) return name;
  const sorted = [...monitors].sort((a, b) => a.x - b.x || a.y - b.y);
  return [name, ...sorted.map((m, i) => `Monitor ${i + 1}: ${m.width} × ${m.height}`), `Gesamt: ${screen.width} × ${screen.height}`].join("\n");
}

/** Maßstäbliche Skizze der Monitore hinter der Beschriftung des Kastens */
export function monitorSketch(screen) {
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
export function renderGameNote() {
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

export function renderServer() {
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

export function renderAddress() {
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
export function screenKey(e, screen) {
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

export function moveScreen(screen, col, row) {
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

export function focusScreen(name) {
  $(`#grid .screen[data-name="${CSS.escape(name)}"]`)?.focus();
}

export function startDrag(e, card, screen) {
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
export function freeCell() {
  const server = state.settings.server;
  const [cols, rows] = state.grid;
  const home = server.screens.find((s) => s.name === server.name) || { col: Math.floor(cols / 2), row: Math.floor(rows / 2) };
  const free = (c, r) => c >= 0 && r >= 0 && c < cols && r < rows && !server.screens.some((s) => s.col === c && s.row === r);
  const near = [[1, 0], [-1, 0], [0, -1], [0, 1]].map(([dc, dr]) => [home.col + dc, home.row + dr]);
  for (const [c, r] of near) if (free(c, r)) return [c, r];
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) if (free(c, r)) return [c, r];
  return null;
}

export function addScreenNamed(name, cell = freeCell()) {
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

export async function addScreen(col, row) {
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

export async function renameScreen(oldName) {
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

export function removeScreen(name) {
  const server = state.settings.server;
  server.screens = server.screens.filter((s) => s.name !== name);
  delete server.sharing.clients[name];
  state.focusName = server.name;
  settingsChanged({ immediate: true });
  toast(`„${name}“ entfernt`);
}

export function setServerName(name) {
  const server = state.settings.server;
  const own = server.screens.find((s) => s.name === server.name);
  if (own && !server.screens.some((s) => s !== own && s.name === name)) {
    renameSwitchKey(own.name, name);
    own.name = name;
  }
  server.name = name;
  settingsChanged();
}
