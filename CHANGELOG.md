# Changelog

Alle nennenswerten Änderungen an Qross. Das Format folgt [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
die Versionen folgen [Semantic Versioning](https://semver.org/lang/de/).

Diese Datei ist die einzige Quelle für die Neuigkeiten: Die App zeigt sie unter „Neuigkeiten“ an, und
jedes GitHub-Release übernimmt den Abschnitt seiner Version.

Rubriken: **Neu**, **Verbessert**, **Behoben**, **Wichtig** (z. B. inkompatible Änderungen).

## [0.3.1] - 2026-10-18

### Behoben

- macOS: Nach dem Download meldete macOS „Qross ist beschädigt und kann nicht geöffnet werden“. Das App-Paket
  ist jetzt vollständig signiert, macOS fragt stattdessen nach, ob Qross geöffnet werden soll.

### Verbessert

- Die Anleitung für den ersten Start beschreibt den Weg über „Dennoch öffnen“ in den Systemeinstellungen,
  wie ihn aktuelle macOS-Versionen verlangen.

## [0.3.0] - 2026-10-18

### Neu

- **Dateien per Zwischenablage:** Dateien und Ordner im Explorer, Finder oder Dateimanager kopieren, den
  Cursor auf den anderen Rechner bewegen und dort einfügen. Das klappt in beide Richtungen und über den
  Server auch von einem Client zum nächsten.
- Die Übertragung läuft im Hintergrund, Maus und Tastatur bleiben flüssig. Jede Datei wird per SHA-256
  geprüft, eingefügt werden kann erst, wenn alles vollständig angekommen ist.
- Bis 1 GB geht das automatisch, die Grenze ist einstellbar (0 = aus). Größere Dateien wie bisher per
  Drag & Drop ziehen.
- Die App meldet, sobald die Dateien bereit zum Einfügen sind oder wenn sie zu groß sind. Liegt das Fenster
  im Hintergrund, kommt eine Systembenachrichtigung.

### Wichtig

- Für Dateien in der Zwischenablage brauchen beide Rechner Qross 0.3.0. Rechner mit 0.2 bleiben verbunden,
  Text, formatierter Text und Bilder funktionieren mit ihnen weiter.

## [0.2.1] - 2026-10-18

### Neu

- Die App zeigt unter „Neuigkeiten“, was sich in jeder Version geändert hat – nach einem Update mit
  Hinweis auf die neue Version.

### Verbessert

- Die Release-Notizen auf GitHub stammen jetzt direkt aus diesem Changelog.

## [0.2.0] - 2026-10-18

### Neu

- Die Zwischenablage überträgt jetzt **Bilder** (z. B. Screenshots mit Win+Shift+S) und **formatierten Text**
  (HTML aus Browser oder Office) – zusätzlich zu reinem Text und in beide Richtungen.

### Verbessert

- Große Bilder werden im Hintergrund umgewandelt und mit niedriger Priorität übertragen. Maus und Tastatur
  bleiben dabei flüssig.
- Unter Windows landet ein Bild zusätzlich als Bitmap in der Zwischenablage, damit auch ältere Programme wie
  Paint es einfügen können.

### Wichtig

- Neue Protokollversion 3: Alle Rechner brauchen Qross 0.2.0 oder neuer. Ältere Versionen werden mit einem
  klaren Hinweis abgelehnt.

## [0.1.2] - 2026-10-18

### Verbessert

- Das Logo in der Kopfzeile ist größer, die beiden Bildschirme im Motiv sind jetzt gut erkennbar.

## [0.1.1] - 2026-10-18

### Verbessert

- Im Logo sitzt die Zeigerspitze jetzt genau am Ende der Spur.
- Die Kopfzeile der App zeigt das volle Logo.

### Behoben

- Linux ohne Tray-Symbol (z. B. GNOME ohne AppIndicator-Erweiterung): Schließen beendet die App jetzt sauber,
  statt sie unsichtbar weiterlaufen zu lassen.

## [0.1.0] - 2026-10-18

Erste Veröffentlichung.

### Neu

- Maus und Tastatur über mehrere Rechner teilen – über **QUIC** statt TCP, damit der Cursor auch bei
  Paketverlust im WLAN flüssig bleibt.
- **Drag & Drop** großer Dateien und ganzer Ordner über die Bildschirmkante, mit SHA-256-Prüfung.
- Text-Zwischenablage in beide Richtungen.
- **Server-Suche im Netzwerk**: Clients finden den Server ohne IP-Eingabe.
- **Ecken-Totzone** und optionale **Wechselverzögerung** gegen versehentliches Wechseln.
- Strg und Cmd werden zwischen PC und Mac automatisch getauscht.
- **Rollen-Taste** hält den Cursor auf dem aktuellen Bildschirm fest, **Strg+Alt+Shift+Esc** holt Maus und
  Tastatur sofort zurück.
- Sichere Verbindung mit gegenseitigen Zertifikaten: Neue Rechner werden erst nach Fingerprint-Vergleich
  freigegeben.
- Grafische Oberfläche mit Layout-Editor, Freigabe-Dialog und Tray-Symbol.
- Läuft unter Windows, Linux (X11) und macOS (Apple Silicon).

[0.3.1]: https://github.com/xozy22/qross/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/xozy22/qross/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/xozy22/qross/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/xozy22/qross/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/xozy22/qross/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/xozy22/qross/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/xozy22/qross/releases/tag/v0.1.0
