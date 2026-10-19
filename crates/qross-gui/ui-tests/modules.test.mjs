// Zusammenhalt der Oberfläche ohne Browser: Importe passen zu den Exporten, und jede Element-ID,
// die ein Modul anspricht, steht in index.html oder wird von einem Modul selbst angelegt.

import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..", "ui");
const html = readFileSync(join(ui, "index.html"), "utf8");
const modules = new Map(
  readdirSync(join(ui, "js"))
    .filter((file) => file.endsWith(".js"))
    .map((file) => [file, readFileSync(join(ui, "js", file), "utf8")]),
);

/** Namen in „{ a, b as c }“: links die Namen im anderen Modul, rechts die hier gültigen */
function names(list) {
  return list
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const [from, as] = part.split(/\s+as\s+/);
      return { from: from.trim(), as: (as ?? from).trim() };
    });
}

function exportsOf(source) {
  const found = new Set();
  for (const m of source.matchAll(/^export\s+(?:async\s+)?(?:function\*?|const|let|var|class)\s+([A-Za-z_$][\w$]*)/gm)) found.add(m[1]);
  for (const m of source.matchAll(/^export\s+(?:const|let|var)\s+\{([^}]+)\}\s*=/gm)) for (const n of names(m[1])) found.add(n.as);
  for (const m of source.matchAll(/^export\s*\{([^}]+)\}/gm)) for (const n of names(m[1])) found.add(n.as);
  return found;
}

function importsOf(source) {
  return [...source.matchAll(/^import\s*\{([^}]+)\}\s*from\s*"\.\/([^"]+)"/gm)].map((m) => ({ file: m[2], names: names(m[1]) }));
}

test("index.html lädt main.js als Modul", () => {
  assert.match(html, /<script type="module" src="js\/main\.js"><\/script>/);
});

test("jeder Import findet sein Modul und dort einen gleichnamigen Export", () => {
  const problems = [];
  let checked = 0;
  for (const [file, source] of modules) {
    for (const { file: target, names: wanted } of importsOf(source)) {
      if (!modules.has(target)) {
        problems.push(`${file}: Modul ${target} fehlt`);
        continue;
      }
      const offered = exportsOf(modules.get(target));
      for (const { from } of wanted) {
        checked++;
        if (!offered.has(from)) problems.push(`${file}: ${target} exportiert „${from}“ nicht`);
      }
    }
  }
  assert.deepEqual(problems, []);
  // Findet das Muster nichts mehr (anderer Stil), prüfte der Test sonst stillschweigend nichts
  assert.ok(checked > 100, `nur ${checked} importierte Namen gefunden`);
});

test("jedes Modul wird von irgendwo geladen", () => {
  const loaded = new Set(["main.js"]);
  for (const source of modules.values()) for (const { file } of importsOf(source)) loaded.add(file);
  assert.deepEqual([...modules.keys()].filter((file) => !loaded.has(file)), []);
});

test("angesprochene Element-IDs gibt es", () => {
  const known = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  // Von Modulen angelegte Elemente: el("div", { id: "…" }) oder node.id = "…"
  for (const source of modules.values()) {
    for (const m of source.matchAll(/\bid:\s*"([^"]+)"/g)) known.add(m[1]);
    for (const m of source.matchAll(/\.id\s*=\s*"([^"]+)"/g)) known.add(m[1]);
  }
  const missing = [];
  const checked = new Set();
  for (const [file, source] of modules) {
    // "#name" als Selektor; Farben wie "#fff" sind keine IDs
    for (const m of source.matchAll(/["'`]#([A-Za-z][\w-]*)/g)) {
      const id = m[1];
      if (/^[0-9a-f]{3,8}$/i.test(id)) continue;
      checked.add(id);
      if (!known.has(id)) missing.push(`${file}: #${id}`);
    }
  }
  assert.deepEqual([...new Set(missing)], []);
  assert.ok(checked.size > 50, `nur ${checked.size} IDs gefunden`);
});

test("Icons und Bilder aus index.html liegen im Ordner", () => {
  for (const m of html.matchAll(/\b(?:src|href)="(?!https?:|#|data:)([^"]+)"/g)) {
    assert.ok(existsSync(join(ui, m[1])), `${m[1]} fehlt`);
  }
});
