// Sprachen der Oberfläche: Beide Wörterbücher passen zusammen, jeder benutzte Schlüssel steht darin, keiner
// liegt brach, und in Modulen und index.html steht kein Text mehr außerhalb der Wörterbücher.

import "./setup.mjs";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import de from "../ui/js/lang/de.js";
import en from "../ui/js/lang/en.js";
import { formatList, formatNumber, language, resolveLanguage, setLanguage, t, tx } from "../ui/js/i18n.js";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..", "ui");
const html = readFileSync(join(ui, "index.html"), "utf8");
const modules = new Map(
  readdirSync(join(ui, "js"))
    .filter((file) => file.endsWith(".js"))
    .map((file) => [file, readFileSync(join(ui, "js", file), "utf8")]),
);
const KEY = /"([a-zA-Z]\w*(?:\.\w+)+)"/g;
const HTML_KEY = /data-i18n(?:-[a-z]+)?="([^"]+)"/g;

const placeholders = (text) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

/** Kommentare weg (grob, reicht hier): Blockkommentare und // bis Zeilenende, aber nicht in „http://“ */
const withoutComments = (source) => source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:"'`\\])\/\/.*$/gm, "$1");

test("Deutsch und Englisch haben dieselben Schlüssel und Platzhalter", () => {
  assert.deepEqual(Object.keys(en).sort(), Object.keys(de).sort());
  for (const key of Object.keys(de)) {
    assert.equal(typeof de[key], "string", key);
    assert.ok(en[key].trim(), `${key} ist englisch leer`);
    assert.deepEqual(placeholders(en[key]), placeholders(de[key]), key);
  }
});

test("Mehrzahl gibt es immer als .one und .other", () => {
  for (const key of Object.keys(de)) {
    if (key.endsWith(".one")) assert.ok(`${key.slice(0, -4)}.other` in de, key);
    if (key.endsWith(".other")) assert.ok(`${key.slice(0, -6)}.one` in de, key);
  }
});

test("jeder benutzte Schlüssel steht im Wörterbuch", () => {
  const missing = [];
  for (const [file, source] of modules) {
    for (const m of source.matchAll(/\b(?:t|tx)\(\s*"([^"]+)"/g)) if (!(m[1] in de)) missing.push(`${file}: ${m[1]}`);
    for (const m of source.matchAll(/\btn\(\s*"([^"]+)"/g)) if (!(`${m[1]}.one` in de)) missing.push(`${file}: ${m[1]}.one`);
  }
  for (const m of html.matchAll(HTML_KEY)) if (!(m[1] in de)) missing.push(`index.html: ${m[1]}`);
  assert.deepEqual(missing, []);
});

test("kein Schlüssel liegt brach", () => {
  // Benutzt ist, was als Zeichenkette in einem Modul steht (auch in Tabellen) oder in index.html; Mehrzahl
  // über den gemeinsamen Anfang („x.files“ für „x.files.one“ und „x.files.other“)
  const used = new Set();
  for (const source of modules.values()) {
    for (const m of source.matchAll(KEY)) {
      used.add(m[1]);
      used.add(`${m[1]}.one`);
      used.add(`${m[1]}.other`);
    }
  }
  for (const m of html.matchAll(HTML_KEY)) used.add(m[1]);
  assert.deepEqual(Object.keys(de).filter((key) => !used.has(key)), []);
});

test("Schlüssel werden nicht zusammengesetzt", () => {
  // Nur tn() selbst hängt „.one“/„.other“ an; sonst steht jeder Schlüssel ausgeschrieben im Code
  for (const [file, source] of modules) if (file !== "i18n.js") assert.doesNotMatch(source, /\b(?:t|tn|tx)\(\s*`/, file);
});

test("in den Modulen steht kein deutscher Text außerhalb der Wörterbücher", () => {
  // Zeichenketten mit Umlauten oder ß; Texte ohne Umlaute fallen hier nicht auf, dafür die Durchsicht
  const found = [];
  for (const [file, source] of modules) {
    for (const m of withoutComments(source).matchAll(/(["'`])((?:\\.|(?!\1)[^\\\n])*?[äöüÄÖÜß](?:\\.|(?!\1)[^\\\n])*?)\1/g)) {
      found.push(`${file}: ${m[0]}`);
    }
  }
  assert.deepEqual(found, []);
});

test("index.html: Texte nur über Schlüssel", () => {
  const text = html
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<svg[\s\S]*?<\/svg>/g, "")
    .replace(/<(script|style|title)[\s\S]*?<\/\1>/g, "")
    .replace(/<[^>]+>/g, "\n");
  // Namen und Einheiten, die in beiden Sprachen gleich sind
  const same = new Set(["Qross", "Server", "Client", "Updates", "Version", "Deutsch", "English", "v–", "?", "px", "ms", "MB", "→"]);
  const code = new Set(["qross.toml", "qross server --config"]);
  const left = text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !same.has(line) && !code.has(line));
  assert.deepEqual(left, []);
  const attributes = [...html.matchAll(/\s(title|aria-label|placeholder|alt)="([^"]+)"/g)]
    .map((m) => `${m[1]}="${m[2]}"`)
    .filter((attr) => !/="(Server|Client)"$/.test(attr));
  assert.deepEqual(attributes, []);
});

test("Sprache: Einstellung vor System, Deutsch nur bei deutschem System", () => {
  assert.equal(resolveLanguage("en", "de-DE"), "en");
  assert.equal(resolveLanguage("de", "en-US"), "de");
  assert.equal(resolveLanguage("auto", "de-AT"), "de");
  assert.equal(resolveLanguage("auto", "de_CH.UTF-8"), "de");
  assert.equal(resolveLanguage("auto", "de"), "de");
  assert.equal(resolveLanguage("auto", "fr-FR"), "en");
  assert.equal(resolveLanguage("auto", "dex"), "en");
  assert.equal(resolveLanguage(undefined, ""), "en");
  assert.equal(resolveLanguage("xx", undefined), "en");
});

test("t, tx, Zahlen und Aufzählungen je Sprache", () => {
  const before = language();
  try {
    setLanguage("de");
    assert.equal(t("news.updated", { version: "1.2.0" }), "Qross wurde auf 1.2.0 aktualisiert.");
    assert.equal(t("gibt.es.nicht"), "gibt.es.nicht");
    assert.equal(t("news.updated", {}), "Qross wurde auf {version} aktualisiert.");
    const strong = { node: "strong" };
    assert.deepEqual(tx("settings.saver.unlock", { title: strong })[0], strong);
    assert.equal(formatNumber(2.345, 1), "2,3");
    assert.equal(formatNumber(12345), "12.345");
    assert.equal(formatList(["a", "b", "c"]), "a, b und c");

    setLanguage("en");
    assert.equal(t("news.updated", { version: "1.2.0" }), "Qross has been updated to 1.2.0.");
    assert.equal(formatNumber(2.345, 1), "2.3");
    assert.equal(formatNumber(12345), "12,345");
    assert.equal(formatList(["a", "b", "c"]), "a, b, and c");

    setLanguage("fr");
    assert.equal(language(), "en", "unbekannte Sprache → Englisch");
  } finally {
    setLanguage(before);
  }
});
