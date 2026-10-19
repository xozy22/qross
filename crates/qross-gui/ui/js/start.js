// Start/Stopp

import { $, invoke, state } from "./core.js";
import { running, toast } from "./util.js";
import { saveSoon } from "./store.js";
import { serverConfirmDialog } from "./discovery.js";

export async function toggle() {
  const button = $("#toggle");
  button.disabled = true;
  try {
    if (running()) await invoke("stop");
    else await startFlow();
  } finally {
    button.disabled = false;
  }
}

export async function startFlow() {
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
