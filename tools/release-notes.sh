#!/usr/bin/env bash
# Text eines GitHub-Releases: der Abschnitt der Version aus CHANGELOG.md plus Download-Hinweise.
# Der Changelog ist die einzige Quelle; die App zeigt denselben Text unter „Neuigkeiten“.
#
#   tools/release-notes.sh 0.2.1 [CHANGELOG.md] > notes.md
set -euo pipefail

version="${1:?Version fehlt, z. B. 0.2.1}"
changelog="${2:-CHANGELOG.md}"

# Umbrochene Listeneinträge (Folgezeilen mit zwei Leerzeichen) werden zu einer Zeile verbunden,
# weil GitHub in Release-Texten jeden Zeilenumbruch anzeigt.
section="$(awk -v head="## [${version}]" '
  { sub(/\r$/, "") }
  index($0, "## [") == 1 { if (found) exit; found = (index($0, head) == 1); next }
  !found { next }
  /^\[[^]]+\]: / { exit }
  /^  +[^ ]/ && item != "" { sub(/^ +/, ""); item = item " " $0; next }
  item != "" { print item; item = "" }
  /^- / { item = $0; next }
  { print }
  END { if (item != "") print item }
' "$changelog" | sed '/./,$!d')"

if [ -z "$section" ]; then
  echo "${changelog} hat keinen Abschnitt für Version ${version}" >&2
  exit 1
fi

printf '%s\n\n' "$section"
cat <<'EOF'
### Downloads

| System | App | Kommandozeile |
|---|---|---|
| Windows x64 | `.msi` oder `-setup.exe` | `…-x86_64-pc-windows-msvc.zip` |
| macOS Apple Silicon | `.dmg` | `…-aarch64-apple-darwin.tar.gz` |
| Linux x64 | `.AppImage` oder `.deb` | `…-x86_64-unknown-linux-gnu.tar.gz` |

Die Programme sind nicht von Microsoft bzw. Apple beglaubigt, deshalb fragt das System beim ersten Start nach:

- **Windows:** SmartScreen meldet sich, dann „Weitere Informationen → Trotzdem ausführen“.
- **macOS:** Qross aus dem DMG in „Programme“ ziehen und öffnen. Meldet macOS, dass es die App nicht überprüfen kann, unter **Systemeinstellungen → Datenschutz & Sicherheit** ganz unten „Dennoch öffnen“ wählen und danach dort „Bedienungshilfen“ und „Eingabeüberwachung“ für Qross erlauben. Alternativ im Terminal: `xattr -dr com.apple.quarantine /Applications/Qross.app`

Prüfsummen: `SHA256SUMS.txt` · Alle Versionen: [CHANGELOG.md](https://github.com/xozy22/qross/blob/main/CHANGELOG.md)
EOF
