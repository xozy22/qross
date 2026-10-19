// Hilfen

import { $, state } from "./core.js";

export const ICONS = {
  plus: '<path d="M12 5v14M5 12h14"/>',
  x: '<path d="M7 7l10 10M17 7L7 17"/>',
  arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  screen: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
  cursor: '<path d="M5 3l14 7.5-6.2 1.7L10 19z" fill="currentColor" stroke="none"/>',
  lock: '<rect x="5" y="11" width="14" height="9" rx="2" fill="currentColor" stroke="none"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
  game: '<path d="M7.5 7h9a5 5 0 0 1 4.9 6l-.7 3.4a2.2 2.2 0 0 1-3.7 1.1L14.6 15H9.4l-2.4 2.5a2.2 2.2 0 0 1-3.7-1.1L2.6 13a5 5 0 0 1 4.9-6z"/><path d="M8 9.5v3M6.5 11h3"/><path d="M15.5 10.5h.01M17.5 12h.01"/>',
  up: '<path d="M12 19V5M6 11l6-6 6 6"/>',
  shield: '<path d="M12 3l7 3v5.5c0 4.2-2.9 7.8-7 9.5-4.1-1.7-7-5.3-7-9.5V6z"/><path d="M9 12h6"/>',
  down: '<path d="M12 5v14M6 13l6 6 6-6"/>',
  pencil: '<path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>',
  trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>',
  wifi: '<path d="M2 8.5a15 15 0 0 1 20 0M5.5 12a10 10 0 0 1 13 0M9 15.5a5 5 0 0 1 6 0"/><path d="M12 19h.01"/>',
  dots: '<circle cx="5.5" cy="12" r="1.7" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.7" fill="currentColor" stroke="none"/><circle cx="18.5" cy="12" r="1.7" fill="currentColor" stroke="none"/>',
  ask: '<circle cx="12" cy="8" r="3.5"/><path d="M5 20a7 7 0 0 1 9.5-6.5"/><path d="M17.5 15.5a2 2 0 1 1 2.6 1.9c-.6.2-.9.6-.9 1.1M19.2 21h.01"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>',
};

/** „?“ hinter einer Überschrift: die lange Erklärung steht im Tooltip und für Screenreader */
export function infoIcon(text) {
  return el("span", { class: "qinfo", tabindex: "0", role: "img", title: text, "aria-label": text, text: "?" });
}

/**
 * Kleines Menü an `anchor` mit Einträgen `{ label, run, disabled, danger }`. Schließt bei Auswahl, Esc,
 * Klick daneben oder Fokus woanders; der Fokus geht dann an `anchor` zurück.
 */
export function openMenu(anchor, items) {
  closeMenu();
  const menu = el(
    "div",
    { class: "menu", role: "menu" },
    ...items.map((item) =>
      el("button", {
        type: "button",
        role: "menuitem",
        class: item.danger ? "danger" : "",
        disabled: item.disabled,
        text: item.label,
        onclick: () => {
          closeMenu(true);
          item.run();
        },
      }),
    ),
  );
  // Im selben Dialog wie der Knopf: sonst läge das Menü unter dem Dialog (oberste Ebene).
  (anchor.closest("dialog") ?? document.body).append(menu);
  const at = anchor.getBoundingClientRect();
  const width = menu.offsetWidth;
  const height = menu.offsetHeight;
  menu.style.left = `${Math.max(8, Math.min(at.right - width, innerWidth - width - 8))}px`;
  menu.style.top = `${at.bottom + 4 + height > innerHeight - 8 ? at.top - height - 4 : at.bottom + 4}px`;
  anchor.setAttribute("aria-expanded", "true");
  const buttons = () => [...menu.querySelectorAll("button:not(:disabled)")];
  menu.addEventListener("keydown", (e) => {
    const list = buttons();
    const i = list.indexOf(document.activeElement);
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMenu(true);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      list[(i + (e.key === "ArrowDown" ? 1 : -1) + list.length) % list.length]?.focus();
    } else if (e.key === "Tab") {
      closeMenu();
    }
  });
  const outside = (e) => {
    if (!menu.contains(e.target) && e.target !== anchor && !anchor.contains(e.target)) closeMenu();
  };
  document.addEventListener("pointerdown", outside, true);
  openMenu.current = { menu, anchor, outside };
  buttons()[0]?.focus();
}

export function closeMenu(refocus = false) {
  const current = openMenu.current;
  if (!current) return;
  openMenu.current = null;
  document.removeEventListener("pointerdown", current.outside, true);
  current.anchor.setAttribute("aria-expanded", "false");
  current.menu.remove();
  if (refocus) current.anchor.focus();
}

export function icon(name) {
  const span = document.createElement("span");
  span.style.display = "contents";
  span.innerHTML = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;
  return span.firstElementChild;
}

export function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v !== false && v != null) node.setAttribute(k, v === true ? "" : v);
  }
  for (const child of children.flat()) {
    if (child != null && child !== false) node.append(child);
  }
  return node;
}

/** Kurzer Hinweis unten; mit `action` ({ label, run }) bekommt er einen Knopf und bleibt länger stehen. */
export function toast(message, kind = "", action = null) {
  const node = el(
    "div",
    { class: `toast ${kind}` },
    el("span", { text: message }),
    action
      ? el("button", {
          type: "button",
          class: "toast-action",
          text: action.label,
          onclick: () => {
            node.remove();
            action.run();
          },
        })
      : null,
  );
  $("#toasts").append(node);
  setTimeout(() => node.remove(), action ? 12000 : kind === "error" || kind === "warn" ? 7000 : 3500);
}

export function fingerprintNode(fp, big = false) {
  const node = el("code", { class: big ? "fingerprint big" : "fingerprint" });
  fillFingerprint(node, fp);
  return node;
}

export function fillFingerprint(node, fp) {
  node.replaceChildren(...(fp || "").split(":").map((g) => el("span", { text: g })));
}

export function debounce(fn, ms) {
  let timer;
  const wrapped = (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
  wrapped.flush = () => {
    clearTimeout(timer);
    return fn();
  };
  return wrapped;
}

export function running() {
  return state.status && state.status.role != null;
}

export function store(key, value) {
  try {
    if (value === undefined) return localStorage.getItem(key);
    localStorage.setItem(key, value);
  } catch {
    return null;
  }
  return null;
}

export async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(`„${text}“ kopiert`);
  } catch {
    toast("Kopieren ist hier nicht möglich. Bitte den Text markieren.", "error");
  }
}
