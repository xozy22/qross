# glide-gui

Grafische Oberfläche für Glide (Tauri 2, Frontend als reines HTML/CSS/JS in `ui/`, kein npm-Build nötig).
Server und Client laufen direkt im Prozess der Oberfläche.

```bash
cargo run -p glide-gui
```

- **Server:** Bildschirme im Raster anordnen (Ziehen, Doppelklick zum Umbenennen). Direkt benachbarte
  Bildschirme ergeben die `links`; die Oberfläche schreibt sie zusätzlich als `glide.toml` ins Datenverzeichnis.
  Unbekannte Clients erscheinen mit Fingerprint zur Freigabe (ersetzt `glide trust`).
- **Client:** Server-Adresse eingeben; beim ersten Kontakt wird der Fingerprint des Servers zur Bestätigung angezeigt.
- **Tray:** Farbiger Punkt zeigt den Zustand. Fenster schließen versteckt es nur, beendet wird über das Tray-Menü.

Daten liegen wie beim CLI in `%APPDATA%\Glide` (`gui.json` für die Oberfläche), abweichend per
Umgebungsvariable `GLIDE_DATA_DIR`.

Debug-Builds enthalten das MCP-Bridge-Plugin (nur `127.0.0.1`, ab Port 9223) für automatisierte Tests
über den Tauri-MCP-Server. Release-Builds enthalten es nicht.

Das Logo liegt als SVG in `icons/logo.svg` (ab 128 px) und `icons/logo-small.svg` (vereinfachte Variante
für 16–64 px, Tray und Kopfzeile). `icons/render_logo.py` erzeugt daraus alle PNG/ICO-Dateien sowie
`docs/assets/logo.png`. Das Tray-Icon bekommt seinen Statuspunkt zur Laufzeit.
