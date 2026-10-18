# qross-gui

Grafische Oberfläche für Qross (Tauri 2, Frontend als reines HTML/CSS/JS in `ui/`, kein npm-Build nötig).
Server und Client laufen direkt im Prozess der Oberfläche.

```bash
cargo run -p qross-gui
```

- **Server:** Bildschirme im Raster anordnen (Ziehen, Doppelklick zum Umbenennen). Direkt benachbarte
  Bildschirme ergeben die `links`; die Oberfläche schreibt sie zusätzlich als `qross.toml` ins Datenverzeichnis.
  Unbekannte Clients erscheinen mit Fingerprint zur Freigabe (ersetzt `qross trust`).
- **Client:** Server-Adresse eingeben; beim ersten Kontakt wird der Fingerprint des Servers zur Bestätigung angezeigt.
- **Dateiübertragungen:** Laufen Dateien per Drag & Drop oder über die Zwischenablage länger als eine halbe
  Sekunde, erscheint über dem Protokoll je Übertragung eine Zeile mit Namen, Prozent, Rate, Restzeit und
  „Abbrechen“; das geht auf beiden Seiten. Am Ende steht kurz das Ergebnis da, bei kopierten Dateien der
  Hinweis zum Einfügen. Läuft eine Übertragung länger als drei Sekunden und ist das Fenster im Hintergrund,
  meldet sich ihr Ende als Systembenachrichtigung. Ausprobieren ohne zweiten Rechner: Oberfläche als Client
  mit `127.0.0.1:24990` (Testmodus), dann `cargo run -p qross-net --example scripted_server -- drag <Name> <Datei>`.
- **Tray:** Farbiger Punkt zeigt den Zustand. Fenster schließen versteckt es nur, beendet wird über das Tray-Menü.
  Zeigt das System kein Tray an (Linux ohne StatusNotifier-Host, z. B. GNOME ohne AppIndicator-Erweiterung),
  beendet Schließen die App, damit sie nicht unerreichbar im Hintergrund weiterläuft.

Daten liegen wie beim CLI in `%APPDATA%\Qross` (`gui.json` für die Oberfläche), abweichend per
Umgebungsvariable `QROSS_DATA_DIR`.

Debug-Builds enthalten das MCP-Bridge-Plugin (nur `127.0.0.1`, ab Port 9223) für automatisierte Tests
über den Tauri-MCP-Server. Release-Builds enthalten es nicht. Mit `QROSS_NO_TRAY=1` verhält sich ein
Debug-Build wie ein System ohne Tray.

Das Logo gibt es als `icons/logo.svg` (volles Logo, ab 128 px und in der Kopfzeile) und
`icons/logo-small.svg` (vereinfachte Variante für 16–64 px und das Tray). Beide erzeugt
`icons/render_logo.py` aus derselben Geometrie, zusammen mit allen PNG/ICO-Dateien sowie
`docs/assets/logo.svg` und `docs/assets/logo.png`. Änderungen am Logo also im Skript vornehmen, nicht in
den SVGs. Das Tray-Icon bekommt seinen Statuspunkt zur Laufzeit.
