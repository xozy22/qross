// Tastatur

import { $, $$, invoke, state } from "./core.js";
import { el, icon, openMenu, toast } from "./util.js";
import { locale, t } from "./i18n.js";
import { settingsChanged } from "./store.js";

export const KEY_GROUPS = [
  ["keys.groupLetters", /^Key[A-Z]$/],
  ["keys.groupDigits", /^Digit\d$/],
  ["keys.groupModifiers", /^(Control|Shift|Alt|Meta)(Left|Right)$|^CapsLock$/],
  ["keys.groupFunction", /^F\d+$/],
  ["keys.groupMedia", /^(Audio|Media)/],
  ["keys.groupNavigation", /^(Arrow|Home|End|Page|Insert|Delete)/],
  ["keys.groupNumpad", /^Num/],
  ["keys.groupOther", /./],
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
  const groups = KEY_GROUPS.map(([label, re]) => ({ label: t(label), re, keys: [] }));
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
  if (!text) return t("common.off");
  const mac = state.platform === "macos";
  const shift = t("keys.shift");
  const names = mac ? { Ctrl: "Ctrl", Shift: shift, Alt: "Option", Meta: "Cmd" } : { Ctrl: t("keys.ctrl"), Shift: shift, Alt: "Alt", Meta: "Windows" };
  const parts = text.split("+");
  const key = parts.pop();
  return [...parts.map((m) => names[m] || m), keyLabel(key)].join("+");
}

export function machinesForRemap() {
  const server = state.settings.server;
  return server.screens.map((s) => s.name).filter((n) => n !== server.name).sort((a, b) => a.localeCompare(b, locale()));
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
  renderSwitchKeys();
  renderKeyModes();

  const media = $("#mediaMode");
  media.value = keys.media;
  const platform = state.platform;
  media.disabled = platform !== "windows";
  $("#mediaHint").textContent =
    platform === "macos" ? t("keys.mediaMac")
    : platform === "linux" ? t("keys.mediaLinux")
    : t("keys.mediaHint", { option: t("settings.keys.mediaLocal") });

  const machines = machinesForRemap();
  if (!machines.includes(state.remapPc)) state.remapPc = machines[0] ?? null;
  const pc = $("#remapPc");
  pc.replaceChildren(
    ...(machines.length
      ? machines.map((m) => el("option", { value: m, text: m }))
      : [el("option", { value: "", text: t("keys.noOtherComputer") })]),
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
                title: t("keys.removeRule"),
                "aria-label": t("keys.removeRuleNamed", { from: keyLabel(rule.from), to: keyLabel(rule.to) }),
                onclick: () => removeRule(i),
              },
              icon("x"),
            ),
          ),
        )
      : [el("li", { class: "empty", text: hasPc ? t("keys.noRules", { name: state.remapPc }) : t("keys.remapNoComputer") })]),
  );
}

// Wechsel-Kürzel: je Richtung und je Rechner im Layout eines. Der Server prüft dieselben Regeln;
// hier werden doppelte Belegungen nur angezeigt.

export const DIRECTIONS = [
  ["left", "keys.left", 180],
  ["right", "keys.right", 0],
  ["up", "keys.up", -90],
  ["down", "keys.down", 90],
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
    .sort((a, b) => (a === server.name ? -1 : b === server.name ? 1 : a.localeCompare(b, locale())));
  return [
    ...DIRECTIONS.map(([id, label, turn]) => ({ kind: "directions", id, label: t(label), turn })),
    ...names.map((name) => ({ kind: "screens", id: name, label: t("keys.toScreen", { name }), name, isServer: name === server.name })),
  ];
}

/** Ziel in „Auch belegt für“: Richtung wie beschriftet, Rechner nur mit Namen */
const listName = (target) => (target.kind === "screens" ? t("keys.quotedName", { name: target.name }) : target.label);

/** Hinweis je Ziel: gleich wie die Fang-Taste, doppelt belegt oder ohne Zusatztaste */
export function switchProblems(targets) {
  const keys = state.settings.server.keys;
  const all = switchKeys();
  const lock = hotkeyId(keys.lock);
  const byKey = new Map();
  for (const target of targets) {
    const id = hotkeyId(all[target.kind][target.id]);
    if (id) byKey.set(id, [...(byKey.get(id) || []), target]);
  }
  const problems = new Map();
  for (const target of targets) {
    const hotkey = all[target.kind][target.id];
    const id = hotkeyId(hotkey);
    if (!id) continue;
    const others = byKey.get(id).filter((o) => o !== target);
    if (id === lock) problems.set(target, { text: t("keys.isLock"), error: true });
    else if (others.length) problems.set(target, { text: t("keys.alsoUsed", { targets: others.map(listName).join(", ") }), error: true });
    else if (!hotkey.includes("+") && TYPING_KEYS.test(hotkey)) problems.set(target, { text: t("keys.noModifier"), error: false });
  }
  return problems;
}

export function renderSwitchKeys() {
  const all = switchKeys();
  const targets = switchTargets();
  const problems = switchProblems(targets);
  const recordingId = state.recording?.kind === "switch" ? state.recording.target : null;
  const focused = document.activeElement?.closest?.(".switch-list li")?.dataset.target;
  $("#switchHint").textContent = t("keys.switchHint");
  const info = state.platform === "linux" ? t("keys.switchInfoLinux") : t("keys.switchInfo");
  $("#switchInfo").title = info;
  $("#switchInfo").setAttribute("aria-label", info);

  // Je Ziel: Taste (oder „Belegen“), dahinter ein Menü mit Ändern und Entfernen
  const row = (target) => {
    const id = `${target.kind}:${target.id}`;
    const hotkey = all[target.kind][target.id] || "";
    const recording = recordingId === id;
    const problem = problems.get(target);
    const capClass = ["keycap", recording && "recording", problem?.error && "conflict"].filter(Boolean).join(" ");
    const arrow = target.kind === "directions" ? icon("arrow") : icon("screen");
    if (target.kind === "directions" && target.turn) arrow.style.transform = `rotate(${target.turn}deg)`;
    let controls;
    if (recording) {
      controls = [
        el("kbd", { class: capClass, text: t("keys.pressKey") }),
        el("button", { type: "button", class: "btn small ghost record-switch", text: t("common.cancel"), "aria-label": t("keys.cancelRecording", { target: target.label }), onclick: stopRecording }),
      ];
    } else if (hotkey) {
      const menu = el(
        "button",
        {
          type: "button",
          class: "icon-btn small record-switch",
          title: t("keys.changeOrRemove"),
          "aria-label": t("keys.changeOrRemoveNamed", { target: target.label }),
          "aria-haspopup": "menu",
          onclick: () =>
            openMenu(menu, [
              { label: t("keys.changeMenu"), run: () => recordSwitch(target) },
              {
                label: t("common.remove"),
                danger: true,
                run: () => {
                  delete all[target.kind][target.id];
                  keysChanged();
                },
              },
            ]),
        },
        icon("dots"),
      );
      controls = [el("kbd", { class: capClass, text: hotkeyLabel(hotkey) }), menu];
    } else {
      controls = [
        el(
          "button",
          { type: "button", class: "btn small ghost record-switch", "aria-label": t("keys.assignNamed", { target: target.label }), onclick: () => recordSwitch(target) },
          icon("plus"),
          t("keys.assign"),
        ),
      ];
    }
    return el(
      "li",
      { "data-target": id, class: problem?.error ? "conflict" : "" },
      el(
        "span",
        { class: "switch-target" },
        arrow,
        el("span", { class: "name", text: target.label, title: target.label }),
        target.isServer ? el("span", { class: "server-tag", text: "Server" }) : null,
      ),
      el("span", { class: "switch-key" }, ...controls),
      problem ? el("p", { class: `switch-problem${problem.error ? "" : " warn"}`, text: problem.text }) : null,
    );
  };
  $("#switchDirections").replaceChildren(...targets.filter((target) => target.kind === "directions").map(row));
  const screens = targets.filter((target) => target.kind === "screens");
  $("#switchScreens").replaceChildren(
    ...(screens.length ? screens.map(row) : [el("li", { class: "empty", text: t("keys.noComputer") })]),
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
  $("#modeHint").textContent = t("keys.modeHint");
  const info = state.desktop === "wayland" ? t("keys.modeInfoWayland") : t("keys.modeInfo");
  $("#modeInfo").title = info;
  $("#modeInfo").setAttribute("aria-label", info);
  const machines = machinesForRemap();
  $("#modeList").replaceChildren(
    ...(machines.length
      ? machines.map((machine) => {
          const mode = keys.modes[machine] === "chars" ? "chars" : "position";
          const client = clients.find((c) => c.name === machine);
          const note =
            mode === "chars" && client && !client.types_text
              ? t("keys.noChars", { name: machine })
              : null;
          const select = el(
            "select",
            { "aria-label": t("keys.modeFor", { name: machine }), onchange: (e) => setKeyMode(machine, e.target.value) },
            el("option", { value: "position", text: t("keys.position") }),
            el("option", { value: "chars", text: t("keys.chars") }),
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
      : [el("li", { class: "empty", text: t("keys.modesNoComputer") })]),
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
      label.textContent = t("keys.pressKey");
      label.className = "keycap recording";
      button.textContent = t("common.cancel");
      $("#lockRecordHint").hidden = false;
    },
    end() {
      button.textContent = t("common.change");
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
      button.textContent = t("keys.pressKey");
      button.classList.add("recording");
    },
    end() {
      button.textContent = t("settings.keys.record");
      button.classList.remove("recording");
    },
    done(code) {
      select.value = code;
    },
  });
}

export function setupKeys() {
  $("#lockChange").addEventListener("click", () => (state.recording?.kind === "lock" ? stopRecording() : recordLock()));
  $("#lockMenu").addEventListener("click", () => {
    const keys = state.settings.server.keys;
    openMenu($("#lockMenu"), [
      {
        label: t("keys.lockDefault", { key: hotkeyLabel(DEFAULT_LOCK) }),
        disabled: keys.lock === DEFAULT_LOCK,
        run: () => {
          keys.lock = DEFAULT_LOCK;
          keysChanged();
        },
      },
      {
        label: t("keys.lockOff"),
        disabled: !keys.lock,
        run: () => {
          keys.lock = "";
          keysChanged();
        },
      },
    ]);
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
    if (from === to) return toast(t("keys.sameKey"), "error");
    addRules([[from, to]]);
  });
  $("#settings").addEventListener("close", stopRecording);
}
