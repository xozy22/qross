// Grundlagen: Tauri-Aufrufe, DOM-Kurzformen, Zustand der Oberfläche

export const { invoke } = window.__TAURI__.core;
export const { listen } = window.__TAURI__.event;

export const $ = (sel) => document.querySelector(sel);
export const $$ = (sel) => [...document.querySelectorAll(sel)];

export const DEFAULT_PORT = 24900;

export const state = {
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
  /** Rechner, dessen Details die Seitenleiste zeigt (Name in der Anordnung), und ob ihn jemand gewählt hat */
  selected: null,
  selectedByUser: false,
  /** Reiter der Details: connection, rights oder info */
  inspectorTab: "connection",
};

export const PLATFORMS = {
  windows: { label: "Windows", short: "Win" },
  macos: { label: "macOS", short: "Mac" },
  linux: { label: "Linux", short: "Lx" },
  other: { label: "anderes System", short: "?" },
};
