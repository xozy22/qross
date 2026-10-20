// Eigene Titelleiste: Die Kopfzeile zieht das Fenster (data-tauri-drag-region), die Knöpfe rechts
// minimieren, maximieren und schließen es. Schließen versteckt es wie bisher im Tray (CloseRequested
// in main.rs). Unter macOS bleiben die Ampel-Knöpfe des Systems; sie liegen links in der Kopfzeile.

import { $, state } from "./core.js";
import { t } from "./i18n.js";

export async function setupWindowControls() {
  document.documentElement.dataset.platform = state.platform;
  if (state.platform === "macos") return;
  const win = window.__TAURI__.window.getCurrentWindow();
  $("#winMin").addEventListener("click", () => win.minimize());
  $("#winMax").addEventListener("click", () => win.toggleMaximize());
  $("#winClose").addEventListener("click", () => win.close());
  // Maximiert oder nicht: Symbol und Beschriftung des mittleren Knopfs
  const sync = async () => {
    const maximized = await win.isMaximized();
    const button = $("#winMax");
    button.classList.toggle("maximized", maximized);
    button.title = t(maximized ? "window.restore" : "window.maximize");
    button.setAttribute("aria-label", button.title);
  };
  await sync();
  await win.onResized(sync);
}
