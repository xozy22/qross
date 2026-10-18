# Qross-Protokoll, Version 3

Entscheidungsgrundlage: [benchmark-ergebnisse.md](benchmark-ergebnisse.md).
Implementierung: `crates/qross-proto`.

## Transport

- **QUIC** (`quinn`) über UDP, Standardport **24900**, ALPN `qross/3`, ausschließlich TLS 1.3
- Beide Seiten weisen sich mit einem **selbstsignierten Zertifikat** aus (gegenseitige Authentifizierung).
  Vertrauen entsteht über den SHA-256-Fingerprint des Zertifikats:
  - Der Server nimmt nur Clients an, die er per `qross trust <name> <fingerprint>` freigegeben hat.
  - Der Client speichert den Server-Fingerprint beim ersten Kontakt (TOFU) oder prüft gegen `--server-fingerprint`.
- Keepalive 1 s, Idle-Timeout 5 s. Bricht die Verbindung ab, holt der Server den Cursor zurück, und der
  Client lässt alle gedrückten Tasten los.

## Kanäle

| Kanal | Richtung | Zuverlässig | Inhalt |
|---|---|---|---|
| Control (bidirektionaler Stream, vom Client geöffnet) | ⇄ | ja | `Hello`, `Welcome`/`Reject`, `ScreenChanged` |
| Suche (UDP-Broadcast, Port 24901, außerhalb von QUIC) | ⇄ | nein | `Discovery::Probe` / `Announce` |
| Input (unidirektionaler Stream, Priorität 10) | Server → Client | ja, geordnet | `Enter`, `Leave`, `Key`, `Button`, `Wheel` |
| Datagramm | Server → Client | **nein** | `MouseMove` |
| Clipboard (je Übertragung ein eigener Stream, Priorität −10) | ⇄ | ja | `Clipboard::Data { text, html, png }` |
| Datei (je Datei ein eigener Stream, Priorität −10) | Server → Client | ja | `FileHeader`, Rohbytes, `FileTrailer` (SHA-256) |

Jeder unidirektionale Stream beginnt mit einem Byte `StreamKind` (1 = Input, 2 = Clipboard, 3 = Datei).
Durch die Trennung blockieren Clipboard und Dateien nie die Eingaben, das war im Benchmark der zweite
große Schwachpunkt von Synergy.

## Kodierung

- Stream-Frames: `u32` Länge (big-endian) + [postcard](https://postcard.jamesmunns.com/)-kodierte Nachricht, maximal 64 MiB
- Datagramme: reines postcard (`enum Datagram`), eine Mausbewegung braucht ca. 8–12 Bytes

## Ablauf

0. Optional: Ohne angegebene Adresse sucht der Client per UDP-Broadcast (`Discovery::Probe`) auf jeder
   IPv4-Schnittstelle. Jeder Server antwortet mit Name, Port, Fingerprint und Betriebssystem. Die Antwort
   ist nur ein Vorschlag; vertraut wird erst beim Verbinden über den Fingerprint.
1. Client öffnet den Control-Stream und sendet `Hello { version, name, screen, platform }`.
2. Server prüft Version, Namen (muss im Layout stehen und darf nicht bereits verbunden sein) und Fingerprint und
   antwortet mit `Welcome { …, platform }` oder `Reject { reason }`. Ist genau eine Seite ein Mac, tauscht
   der Client Strg und Cmd, damit etwa Strg+C auf dem Mac weiterhin kopiert.
3. Der Server öffnet den Input-Stream.
4. Erreicht der Cursor eine Kante mit einem verbundenen Nachbarn: `Enter { epoch, x, y }`. Danach folgen
   `MouseMove`-Datagramme mit dieser `epoch` und steigender `seq`.
5. Verlässt der Cursor den Client: `Leave`. Der Client lässt alle Tasten los und schickt sein Clipboard,
   falls es sich geändert hat.

## Zwischenablage

Übertragen werden alle vorhandenen Darstellungen gleichzeitig: Text, HTML-Ausschnitt und Bild als PNG.
Der Empfänger stellt sie wieder gemeinsam bereit, sodass jedes Programm sein bevorzugtes Format findet.
Unter Windows sind das Unicode-Text, „HTML Format“, „PNG“ und zusätzlich ein DIB für ältere Programme.
Lesen und PNG-Kodierung laufen in einem eigenen Thread, damit große Screenshots die Maus nicht ausbremsen.
Bilder über der Frame-Grenze werden weggelassen, Text und HTML gehen trotzdem mit.

## Drag & Drop von Dateien

1. Mit gedrückter Maustaste an einer Kante prüft der Server, ob gerade Dateien gezogen werden. Unter Windows
   erscheint dafür kurz ein unsichtbares OLE-Drop-Ziel unter dem Cursor und liest die Dateiliste des laufenden
   Drags (`CF_HDROP`). Ohne Dateien, etwa beim Verschieben eines Fensters, wird nicht gewechselt.
2. Mit Dateien bricht der Server den lokalen Drag ab (Esc), wechselt zum Client und sendet
   `DragStart { transfer, entries }`. Danach überträgt er jede Datei auf einem eigenen Stream. Ordner
   werden rekursiv mitgenommen.
3. Der Client drückt dabei **keine** Maustaste. Er schreibt die Dateien in ein Staging-Verzeichnis
   (`*.part`, umbenannt erst nach erfolgreicher SHA-256-Prüfung).
4. Lässt der Nutzer die Taste los, kommt `Drop { transfer, x, y }`. Der Client bestimmt den Ordner unter dem Cursor:
   ein Explorer-Fenster (bei Windows-11-Tabs der sichtbare Tab), den Desktop oder sonst `Downloads\Qross`.
   Sobald alles angekommen ist, verschiebt er die Dateien dorthin. Vorhandene Namen werden nicht
   überschrieben, sondern durchnummeriert, z. B. „Bericht (2).pdf“.
5. Verlässt der Cursor den Client ohne Drop, verwirft der Client die Übertragung und bricht die Streams ab.

Dateinamen werden beim Empfang bereinigt: keine absoluten Pfade und kein `..`, unter Windows unzulässige
Zeichen und reservierte Namen werden ersetzt.

## Regeln für Robustheit

- **Mausposition absolut**, in Client-Koordinaten (physische Pixel, virtueller Desktop). Ein verlorenes
  Datagramm wird vom nächsten überholt, Fehler summieren sich nicht auf.
- Der Client verwirft Datagramme mit falscher `epoch` (Nachzügler eines früheren Besuchs) und mit nicht
  neuerer `seq` (Umsortierung im Netz). Der Vergleich ist wrap-around-sicher.
- `Button` enthält die Position. So landet ein Klick richtig, auch wenn das letzte Bewegungs-Datagramm fehlt.
- Wechsel nur außerhalb der **Ecken-Totzone** (Standard 8 px) und optional erst nach einer
  **Wechselverzögerung**, also wenn eine bestimmte Zeit gegen die Kante gedrückt wird. Das verhindert
  versehentliche Wechsel beim Zielen auf Startmenü oder Schließen-Knopf.
- Tasten sind **physische Tasten als USB-HID-Usage**. Das Layout des Clients bestimmt das Zeichen.
  Das ist plattformneutral und funktioniert auch mit unterschiedlichen Tastaturlayouts.

## Noch offen

- Drag & Drop in Gegenrichtung (Client → Server) und als Quelle unter Linux/macOS
- Relative Mausbewegung für Spiele
- Zwischenablage: RTF; Dateien per Kopieren/Einfügen
- Bildschirmschoner-Sync, Strg+Alt+Entf, Secure Desktop (UAC)
- Wayland (libei/InputCapture-Portal)
