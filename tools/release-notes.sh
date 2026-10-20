#!/usr/bin/env bash
# Text eines GitHub-Releases: der Abschnitt der Version aus CHANGELOG.md plus Download-Hinweise.
# Der Changelog ist die einzige Quelle; die App zeigt denselben Text unter „Neuigkeiten“.
#
#   tools/release-notes.sh 0.2.1 [CHANGELOG.md] > notes.md
set -euo pipefail

version="${1:?version missing, e.g. 0.2.1}"
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
  echo "${changelog} has no section for version ${version}" >&2
  exit 1
fi

# Nur der Abschnitt, ohne Download-Hinweise (für das Update-Manifest latest.json)
if [ "${NOTES_ONLY:-}" = 1 ]; then
  printf '%s\n' "$section"
  exit 0
fi

printf '%s\n\n' "$section"
cat <<'EOF'
### Downloads

| System | App | Command line |
|---|---|---|
| Windows x64 | `.msi` or `-setup.exe` | `…-x86_64-pc-windows-msvc.zip` |
| macOS Apple Silicon | `.dmg` | `…-aarch64-apple-darwin.tar.gz` |
| Linux x64 | `.AppImage` or `.deb` | `…-x86_64-unknown-linux-gnu.tar.gz` |

The apps aren't signed by Microsoft or notarized by Apple, so the system asks for confirmation on first launch:

- **Windows:** SmartScreen appears; choose “More info → Run anyway”.
- **macOS:** Drag Qross from the DMG into “Applications” and open it. If macOS says it can't verify the app, choose “Open Anyway” at the bottom of **System Settings → Privacy & Security**, then allow “Accessibility” and “Input Monitoring” for Qross there. Alternatively, in Terminal: `xattr -dr com.apple.quarantine /Applications/Qross.app`. If mouse or keyboard input doesn't arrive after an update, Qross shows a hint; more under [Keeping permissions across updates](https://github.com/xozy22/qross#macos-rechte-über-updates-behalten) (in German).

Checksums: `SHA256SUMS.txt` · All versions: [CHANGELOG.md](https://github.com/xozy22/qross/blob/main/CHANGELOG.md)
EOF
