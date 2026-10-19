// Tastatur

import { $, $$, invoke, state } from "./core.js";
import { el, icon, toast } from "./util.js";
import { settingsChanged } from "./store.js";

export const KEY_GROUPS = [
  ["Buchstaben", /^Key[A-Z]$/],
  ["Ziffern", /^Digit\d$/],
  ["Zusatztasten", /^(Control|Shift|Alt|Meta)(Left|Right)$|^CapsLock$/],
  ["Funktionstasten", /^F\d+$/],
  ["Medien", /^(Audio|Media)/],
  ["Navigation", /^(Arrow|Home|End|Page|Insert|Delete)/],
  ["Ziffernblock", /^Num/],
  ["Weitere", /./],
];
export const DEFAULT_LOCK = "ScrollLock";
export const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);
export const PRESETS = {
  "alt-meta": [["AltLeft", "MetaLeft"], ["MetaLeft", "AltLeft"], ["AltRight", "MetaRight"], ["MetaRight", "AltRight"]],
  "caps-ctrl": [["CapsLock", "ControlLeft"]],
};

export async function loadKeyNames() {
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

export const keyLabel = (name) => state.keyLabels.get(name) || name;

/** „Ctrl+Alt+KeyL“ → „Strg+Alt+L“ (Mac: Ctrl, Option, Cmd) */
export function hotkeyLabel(text) {
  if (!text) return "Aus";
  const mac = state.platform === "macos";
  const names = mac ? { Ctrl: "Ctrl", Shift: "Umschalt", Alt: "Option", Meta: "Cmd" } : { Ctrl: "Strg", Shift: "Umschalt", Alt: "Alt", Meta: "Windows" };
  const parts = text.split("+");
  const key = parts.pop();
  return [...parts.map((m) => names[m] || m), keyLabel(key)].join("+");
}

export function machinesForRemap() {
  const server = state.settings.server;
  return server.screens.map((s) => s.name).filter((n) => n !== server.name).sort((a, b) => a.localeCompare(b, "de"));
}

export function renderKeys() {
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

export const DIRECTIONS = [
  ["left", "Nach links", 180],
  ["right", "Nach rechts", 0],
  ["up", "Nach oben", -90],
  ["down", "Nach unten", 90],
];
/** Tasten, die ohne Zusatztaste zum Tippen gebraucht werden */
export const TYPING_KEYS = /^(Key|Digit|Numpad|Arrow|Space$|Enter$|Tab$|Backspace$|Minus$|Equal$|Bracket|Semicolon$|Quote$|Backquote$|Comma$|Period$|Slash$|Backslash$|Intl|Home$|End$|Page|Delete$)/;

export function switchKeys() {
  const keys = state.settings.server.keys;
  keys.switch ??= {};
  keys.switch.directions ??= {};
  keys.switch.screens ??= {};
  return keys.switch;
}

/** Vergleichbare Form, z. B. „Alt+Ctrl+KeyL“ und „Ctrl+Alt+KeyL“ gleich */
export function hotkeyId(text) {
  if (!text) return "";
  const parts = text.split("+").map((p) => p.trim().toLowerCase());
  const key = parts.pop();
  return [...["ctrl", "shift", "alt", "meta"].filter((m) => parts.includes(m)), key].join("+");
}

export function switchTargets() {
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
export function switchProblems(targets) {
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

export function renderSwitchKeys() {
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

export function recordSwitch(target) {
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

export function renderKeyModes() {
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

export function setKeyMode(machine, mode) {
  const modes = state.settings.server.keys.modes;
  if (mode === "chars") modes[machine] = "chars";
  else delete modes[machine];
  keysChanged();
}

/** Die Einstellung eines umbenannten Rechners wandert mit. */
export function renameKeyMode(oldName, name) {
  const modes = (state.settings.server.keys.modes ??= {});
  if (oldName in modes) modes[name] = modes[oldName];
  delete modes[oldName];
}

/** Das Kürzel eines umbenannten Rechners wandert mit. */
export function renameSwitchKey(oldName, name) {
  const screens = switchKeys().screens;
  if (oldName in screens) screens[name] = screens[oldName];
  delete screens[oldName];
}

export function keysChanged() {
  renderKeys();
  settingsChanged();
}

export function setRules(machine, change) {
  const remap = state.settings.server.keys.remap;
  const rules = change([...(remap[machine] || [])]);
  if (rules.length) remap[machine] = rules;
  else delete remap[machine];
  keysChanged();
}

/** Regel setzen; eine vorhandene Regel für dieselbe gedrückte Taste wird ersetzt. */
export function addRules(pairs) {
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

export function removeRule(index) {
  setRules(state.remapPc, (rules) => {
    rules.splice(index, 1);
    return rules;
  });
}

/** Tastenaufnahme: fängt Tasten ab, bevor Dialog (Esc) oder Formularfelder sie sehen. */
export function startRecording(recording) {
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

export function finishRecording(value) {
  const recording = state.recording;
  stopRecording();
  recording?.done(value);
}

export function stopRecording() {
  const recording = state.recording;
  if (!recording) return;
  state.recording = null;
  window.removeEventListener("keydown", recording.keyDown, true);
  window.removeEventListener("keyup", recording.keyUp, true);
  $("#settings").removeEventListener("cancel", recording.cancel);
  recording.end?.();
}

export function recordLock() {
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

export function recordInto(selectId, button) {
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

export function setupKeys() {
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
