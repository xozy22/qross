// Sprache der Oberfläche: Texte aus den Wörterbüchern (lang/de.js, lang/en.js), Zahlen und Daten im
// passenden Format. Welche Sprache gilt, legt main.js beim Start fest (Einstellung oder Systemsprache);
// ein Wechsel lädt die Oberfläche neu, deshalb dürfen Module Texte auch einmalig beim Laden holen.

import de from "./lang/de.js";
import en from "./lang/en.js";

/** Wörterbücher je Sprache: Schlüssel „bereich.name“, Platzhalter {name} */
export const DICTIONARIES = { de, en };
/** Zuletzt gültige Sprache, damit der nächste Start gleich in ihr beginnt */
export const LANG_KEY = "qross.lang";

let current = initialLanguage();
let plural = null;

function initialLanguage() {
  try {
    const saved = localStorage.getItem(LANG_KEY);
    if (DICTIONARIES[saved]) return saved;
  } catch {
    // ohne Speicher (Test, gesperrt): Systemsprache
  }
  return resolveLanguage("auto", globalThis.navigator?.language);
}

/** Sprache aus der Einstellung ("auto", "de", "en") und der Systemsprache (z. B. "de-DE") */
export function resolveLanguage(setting, system) {
  if (DICTIONARIES[setting]) return setting;
  return /^de(?:[-_]|$)/i.test(system ?? "") ? "de" : "en";
}

export function language() {
  return current;
}

/** Gebietsschema für Zahlen und Daten */
export function locale() {
  return current === "de" ? "de-DE" : "en-US";
}

export function setLanguage(lang) {
  current = DICTIONARIES[lang] ? lang : "en";
  plural = null;
  if (globalThis.document?.documentElement) document.documentElement.lang = current;
}

/** Text zum Schlüssel, {name} durch `vars.name` ersetzt. Ein fehlender Schlüssel erscheint als er selbst. */
export function t(key, vars) {
  const text = DICTIONARIES[current][key] ?? DICTIONARIES.en[key] ?? key;
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name) => (name in vars ? String(vars[name]) : whole));
}

/** Mehrzahl: „key.one“ oder „key.other“ je nach Anzahl; {count} ist die Zahl im Format der Sprache. */
export function tn(key, count, vars = {}) {
  plural ??= new Intl.PluralRules(locale());
  const form = plural.select(count) === "one" ? "one" : "other";
  return t(`${key}.${form}`, { count: formatNumber(count), ...vars });
}

/**
 * Wie t(), aber ein Platzhalter darf auch ein Knoten sein (z. B. el("strong", …)). Ergibt eine Liste
 * für el(tag, attrs, …tx(…)) oder node.append(…tx(…)); Text bleibt Text, nie HTML.
 */
export function tx(key, slots = {}) {
  return t(key)
    .split(/\{(\w+)\}/)
    .map((part, i) => (i % 2 ? (slots[part] ?? `{${part}}`) : part))
    .filter((part) => part !== "");
}

/** Zahl im Format der Sprache; `digits` legt die Nachkommastellen fest. */
export function formatNumber(value, digits) {
  const options = digits == null ? {} : { minimumFractionDigits: digits, maximumFractionDigits: digits };
  return value.toLocaleString(locale(), options);
}

/** Aufzählung „a, b und c“ bzw. „a, b, and c“ */
export function formatList(items) {
  return new Intl.ListFormat(locale(), { type: "conjunction" }).format(items);
}

/**
 * Feste Texte in index.html: data-i18n (Inhalt), data-i18n-title, data-i18n-label (aria-label),
 * data-i18n-tip (title und aria-label zugleich) und data-i18n-placeholder.
 */
export function translatePage(root = document) {
  for (const node of root.querySelectorAll("[data-i18n]")) fill(node, t(node.dataset.i18n));
  for (const node of root.querySelectorAll("[data-i18n-title]")) node.title = t(node.dataset.i18nTitle);
  for (const node of root.querySelectorAll("[data-i18n-label]")) node.setAttribute("aria-label", t(node.dataset.i18nLabel));
  for (const node of root.querySelectorAll("[data-i18n-tip]")) {
    const text = t(node.dataset.i18nTip);
    node.title = text;
    node.setAttribute("aria-label", text);
  }
  for (const node of root.querySelectorAll("[data-i18n-placeholder]")) node.placeholder = t(node.dataset.i18nPlaceholder);
}

/** Text in ein Element; Kinder mit data-slot="name" bleiben erhalten und stehen dort, wo {name} steht. */
function fill(node, text) {
  const slots = new Map([...node.children].filter((child) => child.dataset.slot).map((child) => [child.dataset.slot, child]));
  if (!slots.size) {
    node.textContent = text;
    return;
  }
  node.replaceChildren(...text.split(/\{(\w+)\}/).map((part, i) => (i % 2 ? (slots.get(part) ?? `{${part}}`) : part)));
}
