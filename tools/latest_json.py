"""Update-Manifest `latest.json` für den Tauri-Updater aus den signierten Release-Dateien.

Der Updater der App lädt `https://github.com/<repo>/releases/latest/download/latest.json` und sucht
dort zuerst `{os}-{arch}-{installer}` (z. B. `windows-x86_64-msi`), dann `{os}-{arch}`. So bekommt jede
Installation ein Update in ihrer eigenen Form: NSIS bleibt NSIS, MSI bleibt MSI, `.deb` bleibt `.deb`.

    python3 tools/latest_json.py --version 0.4.0 --repo xozy22/qross --dir release \\
        --notes-file notes.md > release/latest.json
"""

import argparse
import datetime
import json
import pathlib
import sys

# Dateiendung des signierten Pakets → Plattform-Schlüssel. Der erste Schlüssel je Plattform dient
# zusätzlich als Rückfall `{os}-{arch}` für Installationen, deren Art der Updater nicht erkennt.
TARGETS = [
    ("-setup.exe", ["windows-x86_64-nsis", "windows-x86_64"]),
    (".msi", ["windows-x86_64-msi"]),
    (".app.tar.gz", ["darwin-aarch64-app", "darwin-aarch64"]),
    (".AppImage", ["linux-x86_64-appimage", "linux-x86_64"]),
    (".deb", ["linux-x86_64-deb"]),
    (".rpm", ["linux-x86_64-rpm"]),
]


def manifest(version, repo, directory, notes="", now=None):
    directory = pathlib.Path(directory)
    platforms = {}
    for sig in sorted(directory.glob("*.sig")):
        package = sig.with_suffix("")
        if not package.exists():
            raise SystemExit(f"{sig.name}: das signierte Paket {package.name} fehlt")
        keys = next((keys for suffix, keys in TARGETS if package.name.endswith(suffix)), None)
        if keys is None:
            raise SystemExit(f"{sig.name}: unbekannte Paketart")
        entry = {
            "signature": sig.read_text(encoding="utf-8").strip(),
            "url": f"https://github.com/{repo}/releases/download/v{version}/{package.name}",
        }
        for key in keys:
            if key in platforms:
                raise SystemExit(f"{key} ist doppelt belegt ({package.name})")
            platforms[key] = entry
    for required in ("windows-x86_64", "darwin-aarch64", "linux-x86_64"):
        if required not in platforms:
            raise SystemExit(f"kein signiertes Paket für {required}")
    now = now or datetime.datetime.now(datetime.timezone.utc)
    return {
        "version": version,
        "notes": notes.strip(),
        "pub_date": now.replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "platforms": platforms,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True, help="ohne v, z. B. 0.4.0")
    parser.add_argument("--repo", required=True, help="z. B. xozy22/qross")
    parser.add_argument("--dir", required=True, help="Ordner mit Paketen und .sig-Dateien")
    parser.add_argument("--notes-file", help="Text der Neuerungen")
    args = parser.parse_args()
    notes = pathlib.Path(args.notes_file).read_text(encoding="utf-8") if args.notes_file else ""
    json.dump(manifest(args.version, args.repo, args.dir, notes), sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
