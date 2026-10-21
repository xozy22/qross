// Reine Logik der Oberfläche: Texte, Zahlenformate, Auswahl, Vergleiche

import "./setup.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import { state } from "../ui/js/core.js";
import { netMatches } from "../ui/js/profiles.js";
import { sharingSummary } from "../ui/js/sharing.js";
import { hotkeyId, hotkeyLabel } from "../ui/js/keys.js";
import { chartMax, formatMs, formatPct, formatRate } from "../ui/js/quality.js";
import { chainNumber } from "../ui/js/latency.js";
import { USAGE_TEXT, formatCpu, formatMemory, usageLine, usagePartLines, usageSplit } from "../ui/js/usage.js";
import { updateStatusText } from "../ui/js/settings.js";
import { selectedName } from "../ui/js/layout.js";
import { clientPhase } from "../ui/js/client-view.js";
import { approveLabel, pendingText } from "../ui/js/clients.js";
import { setLanguage } from "../ui/js/i18n.js";

// Die Erwartungen hier sind deutsch; englische Texte prüft i18n.test.mjs.
setLanguage("de");

const NB = " ";

test("Netzwerk: die MAC des Gateways entscheidet, sonst Gateway und Subnetz", () => {
  const mac = { gateway: "192.168.1.1", gateway_mac: "AA:BB:CC:00:11:22", subnet: "192.168.1.0/24" };
  assert.equal(netMatches(mac, { gateway: "10.0.0.1", gateway_mac: "aa:bb:cc:00:11:22" }), true);
  assert.equal(netMatches(mac, { gateway: "192.168.1.1", gateway_mac: "aa:bb:cc:00:11:99", subnet: "192.168.1.0/24" }), false);
  const plain = { gateway: "192.168.1.1", subnet: "192.168.1.0/24" };
  assert.equal(netMatches(plain, { gateway: "192.168.1.1", subnet: "192.168.1.0/24" }), true);
  assert.equal(netMatches(plain, { gateway: "192.168.1.1", subnet: "192.168.2.0/24" }), false);
  assert.equal(netMatches(plain, { gateway: "192.168.1.1" }), true);
  assert.equal(netMatches(plain, { gateway: "192.168.1.254" }), false);
  assert.equal(netMatches({ gateway: null }, { gateway: "192.168.1.1" }), false);
  assert.equal(netMatches(plain, null), false);
});

test("Rechte: Kurzfassung nur für Eingeschränktes", () => {
  assert.equal(sharingSummary({ clipboard: "both", files: "both", drag: "both" }), null);
  assert.equal(
    sharingSummary({ clipboard: "to-client", files: "both", drag: "off" }),
    "Zwischenablage nur empfangen · Drag & Drop aus",
  );
  assert.equal(sharingSummary({ clipboard: "both", files: "from-client", drag: "both" }), "Dateien nur senden");
});

test("Tastenkürzel: Reihenfolge der Zusatztasten egal, Beschriftung je System", () => {
  assert.equal(hotkeyId("Alt+Ctrl+KeyL"), hotkeyId("Ctrl+Alt+KeyL"));
  assert.equal(hotkeyId(" Shift + KeyA "), "shift+keya");
  assert.equal(hotkeyId(""), "");
  assert.equal(hotkeyId(null), "");

  state.keyLabels = new Map([["KeyL", "L"]]);
  state.platform = "windows";
  assert.equal(hotkeyLabel("Ctrl+Alt+Meta+KeyL"), "Strg+Alt+Windows+L");
  state.platform = "macos";
  assert.equal(hotkeyLabel("Ctrl+Alt+Meta+KeyL"), "Ctrl+Option+Cmd+L");
  assert.equal(hotkeyLabel("Shift+F13"), "Umschalt+F13");
  assert.equal(hotkeyLabel(""), "Aus");
});

test("Zahlen: deutsch, mit geschütztem Leerzeichen vor der Einheit", () => {
  assert.equal(formatMs(0.05), `<${NB}0,1${NB}ms`);
  assert.equal(formatMs(2.34), `2,3${NB}ms`);
  assert.equal(formatMs(12.6), `13${NB}ms`);
  assert.equal(formatPct(0), `0${NB}%`);
  assert.equal(formatPct(0.05), `<${NB}0,1${NB}%`);
  assert.equal(formatPct(1.24), `1,2${NB}%`);
  assert.equal(formatPct(12.4), `12${NB}%`);
  assert.equal(formatRate(999), `999${NB}B/s`);
  assert.equal(formatRate(1500), `1,5${NB}KB/s`);
  assert.equal(formatRate(25_000), `25${NB}KB/s`);
  assert.equal(formatRate(2_500_000), `2,5${NB}MB/s`);
  assert.equal(chainNumber(5), `<${NB}0,01`);
  assert.equal(chainNumber(40), "0,04");
  assert.equal(chainNumber(2500), "2,5");
  assert.equal(chainNumber(12_400), "12");
});

test("Auslastung: Prozent eines Kerns, Arbeitsspeicher wie im Task-Manager", () => {
  assert.equal(formatCpu(null), "–");
  assert.equal(formatCpu(0), `0${NB}%`);
  assert.equal(formatCpu(0.04), `<${NB}0,1${NB}%`);
  assert.equal(formatCpu(2.34), `2,3${NB}%`);
  assert.equal(formatCpu(137.4), `137${NB}%`, "mehrere Kerne zugleich");
  assert.equal(formatMemory(7.5 * 1048576), `7,5${NB}MB`);
  assert.equal(formatMemory(128 * 1048576 + 5000), `128${NB}MB`);
  assert.equal(formatMemory(1.2 * 1073741824), `1,2${NB}GB`);
  assert.equal(usageLine({ cpu: 0.4, memory: 128 * 1048576, processes: 6 }), `CPU 0,4${NB}% eines Kerns · RAM 128${NB}MB`);
  assert.equal(usageLine(null), "Wird gemessen …");
  assert.equal(USAGE_TEXT.processes(1), "in 1 Prozess");
  assert.equal(USAGE_TEXT.processes(6), "in 6 Prozessen");
});

test("Auslastung: Qross selbst und Oberfläche, im Tooltip jede Art", () => {
  const MB = 1048576;
  const part = (kind, cpu, mb, processes = 1) => ({ kind, cpu, memory: mb * MB, processes });
  const usage = {
    cpu: 0.4,
    memory: 103 * MB,
    processes: 7,
    parts: [
      part("core", 0.1, 8),
      part("page", 0.2, 30),
      part("graphics", 0.04, 28),
      part("browser", 0, 23),
      part("network", 0, 6),
      part("other", 0.1, 5, 2),
    ],
  };
  assert.equal(usageSplit(usage), `Qross selbst 8,0${NB}MB · Oberfläche 92${NB}MB`);
  assert.deepEqual(usagePartLines(usage), [
    `Qross selbst: CPU 0,1${NB}% · RAM 8,0${NB}MB`,
    `Seite: CPU 0,2${NB}% · RAM 30${NB}MB`,
    `Grafik: CPU <${NB}0,1${NB}% · RAM 28${NB}MB`,
    `Browser: CPU 0${NB}% · RAM 23${NB}MB`,
    `Netzwerk: CPU 0${NB}% · RAM 6,0${NB}MB`,
    `Sonstiges: CPU 0,1${NB}% · RAM 5,0${NB}MB`,
  ]);
  // Ohne Teile (Qross bis 0.14.0) bzw. ohne Oberfläche (Client auf der Kommandozeile) keine Aufteilung
  assert.equal(usageSplit({ ...usage, parts: [] }), null);
  assert.equal(usageSplit({ cpu: 0.4, memory: 8 * MB, processes: 1 }), null);
  assert.equal(usageSplit({ ...usage, parts: [part("core", 0.1, 8)] }), null);
  assert.equal(usageSplit(null), null);
  assert.deepEqual(usagePartLines(null), []);
  // Unter macOS und Linux gibt es keinen eigenen Browser-Prozess
  const mac = { ...usage, parts: [part("core", 1.2, 41), part("page", 0.5, 62), part("network", 0, 9), part("graphics", 0.3, 12)] };
  assert.equal(usageSplit(mac), `Qross selbst 41${NB}MB · Oberfläche 83${NB}MB`);
});

test("Diagramm: Skala mit etwas Luft über dem höchsten Wert", () => {
  assert.equal(chartMax([]), 6);
  assert.equal(chartMax([1, 2]), 6);
  assert.equal(chartMax([9]), 15);
  assert.equal(chartMax([26]), 30);
  assert.equal(chartMax([70]), 100);
});

test("Update-Status: Text und Ton je Phase", () => {
  assert.deepEqual(updateStatusText({ phase: "checking" }), ["Suche nach Updates …", ""]);
  assert.deepEqual(updateStatusText({ phase: "downloading", version: "1.2.0", progress: 40 }), ["Update 1.2.0 wird geladen … 40 %", "new"]);
  assert.deepEqual(updateStatusText({ phase: "available", version: "1.2.0", bundle: "msi" }), [
    "Update 1.2.0 ist verfügbar. Die Installation fragt nach Administratorrechten.",
    "new",
  ]);
  assert.deepEqual(updateStatusText({ phase: "available", version: "1.2.0", bundle: "nsis" }), ["Update 1.2.0 ist verfügbar.", "new"]);
  assert.deepEqual(updateStatusText({ phase: "error", message: "Kein Netz" }), ["Kein Netz.", "error"]);
});

test("Auswahl in der Seitenleiste: Wahl des Nutzers bleibt, sonst Cursor, verbunden, erster anderer", () => {
  state.settings = { server: { name: "pc", screens: [{ name: "pc" }, { name: "mac" }, { name: "laptop" }] } };
  const pick = (status, selected = null, byUser = false) => {
    state.status = { role: null, clients: [], cursor_on: null, ...status };
    state.selected = selected;
    state.selectedByUser = byUser;
    return selectedName();
  };
  // Gestoppt: der erste andere Rechner
  assert.equal(pick({}), "mac");
  // Im Betrieb: der mit dem Cursor, sonst der erste verbundene
  assert.equal(pick({ role: "server", clients: [{ name: "laptop" }, { name: "mac" }], cursor_on: "mac" }), "mac");
  assert.equal(pick({ role: "server", clients: [{ name: "laptop" }], cursor_on: "pc" }), "laptop");
  // Eine automatische Wahl springt nicht, solange der Rechner verbunden ist
  assert.equal(pick({ role: "server", clients: [{ name: "laptop" }, { name: "mac" }], cursor_on: "mac" }, "laptop"), "laptop");
  // Getrennt: neu wählen; die Wahl des Nutzers bleibt dagegen
  assert.equal(pick({ role: "server", clients: [{ name: "mac" }], cursor_on: null }, "laptop"), "mac");
  assert.equal(pick({ role: "server", clients: [{ name: "mac" }], cursor_on: "mac" }, "pc", true), "pc");
  // Nicht mehr in der Anordnung: neu wählen
  assert.equal(pick({}, "alt", true), "mac");
  assert.equal(state.selectedByUser, false);
});

test("Client-Phase für Status und Serverwahl", () => {
  const phase = (status) => {
    state.status = status;
    return clientPhase();
  };
  assert.equal(phase({ role: "client", phase: "connected" }), "connected");
  assert.equal(phase({ role: "client", phase: "connecting" }), "connecting");
  assert.equal(phase({ role: "client", phase: "starting" }), "connecting");
  assert.equal(phase({ role: null, phase: "error" }), "error");
  assert.equal(phase({ role: null, phase: "stopped" }), "stopped");
});

test("Freigabe-Anfragen: Hinweis und Knopf je Vertrauen", () => {
  assert.equal(approveLabel({ trust: "unknown", in_layout: false }), "Freigeben und aufnehmen");
  assert.equal(approveLabel({ trust: "unknown", in_layout: true }), "Freigeben");
  assert.equal(approveLabel({ trust: "known", in_layout: false }), "In Anordnung aufnehmen");
  assert.equal(approveLabel({ trust: "mismatch", in_layout: true }), "Neuen Fingerprint freigeben");
  assert.match(pendingText({ trust: "mismatch" }), /anderen Fingerprint/);
  assert.equal(pendingText({ trust: "known", in_layout: false }), "Ist freigegeben, steht aber noch nicht in der Anordnung.");
  assert.equal(pendingText({ trust: "known", in_layout: true, reason: "Name belegt" }), "Abgewiesen: Name belegt");
});
