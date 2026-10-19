// Einstellungen speichern und übernehmen

import { $, invoke, state } from "./core.js";
import { debounce, toast } from "./util.js";
import { renderServer } from "./layout.js";

export const saveNow = async () => {
  try {
    await invoke("save_settings", { settings: state.settings });
  } catch (e) {
    toast(String(e), "error");
  }
};
export const saveSoon = debounce(saveNow, 450);

export async function settingsChanged({ immediate = false } = {}) {
  renderServer();
  updatePreview();
  if (immediate) await saveSoon.flush();
  else saveSoon();
}

export async function updatePreview() {
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
