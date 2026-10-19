// Was die Module der Oberfläche beim Laden brauchen: die Tauri-Schnittstelle (core.js) und ein
// DOM ohne Elemente. Als erster Import einer Testdatei, vor den Modulen selbst.

globalThis.window = {
  __TAURI__: {
    core: { invoke: async () => null },
    event: { listen: async () => () => {} },
  },
};
globalThis.document = { querySelector: () => null, querySelectorAll: () => [], activeElement: null };
