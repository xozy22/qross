// Kopierte Dateien

import { $$, state } from "./core.js";
import { t } from "./i18n.js";
import { saveSoon, updatePreview } from "./store.js";

export const TOAST_KIND = { info: "", success: "success", warn: "warn", error: "error" };

export function renderClipLimit() {
  for (const input of $$("[data-clip-limit]")) {
    if (document.activeElement !== input) input.value = state.settings.clipboard_files_mb;
  }
}

export function setupClipboardFiles() {
  const mod = state.platform === "macos" ? "Cmd" : t("keys.ctrl");
  for (const hint of $$("[data-clip-hint]")) hint.textContent = t("clipboardFiles.hint", { mod });
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
