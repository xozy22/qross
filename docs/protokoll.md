# Qross-Protokoll, Version 3

Seit Qross 0.3 abwärtskompatibel erweitert um kopierte Dateien in der Zwischenablage, seit Qross 0.4 um
die einzelnen Monitore eines Clients (beides siehe unten).

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
| Control (bidirektionaler Stream, vom Client geöffnet) | ⇄ | ja | `Hello`, `Welcome`/`Reject`, `ScreenChanged`, `Monitors` |
| Suche (UDP-Broadcast, Port 24901, außerhalb von QUIC) | ⇄ | nein | `Discovery::Probe` / `Announce` |
| Input (unidirektionaler Stream, Priorität 10) | Server → Client | ja, geordnet | `Enter`, `Leave`, `Key`, `Button`, `Wheel` |
| Datagramm | Server → Client | **nein** | `MouseMove` |
| Clipboard (je Übertragung ein eigener Stream, Priorität −10) | ⇄ | ja | `Clipboard::Data { text, html, png }`, `Clipboard::Files { transfer, entries, data }` |
| Datei für Drag & Drop (je Datei ein eigener Stream, Priorität −10) | Server → Client | ja | `FileHeader`, Rohbytes, `FileTrailer` (SHA-256) |
| Datei aus der Zwischenablage (wie oben) | ⇄ | ja | `FileHeader`, Rohbytes, `FileTrailer` (SHA-256) |
| Drag (je Nachricht ein eigener Stream, Priorität 10) | ⇄ | ja | `DragMessage::Query` / `Offer` |

Jeder unidirektionale Stream beginnt mit einem Byte `StreamKind` (1 = Input, 2 = Clipboard, 3 = Datei für
Drag & Drop, 4 = Datei aus der Zwischenablage, 5 = Drag-Nachricht). Dateien für Drag & Drop fließen seit
Qross 0.4 in beide Richtungen.
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
   antwortet mit `Welcome { version, …, platform }` oder `Reject { reason }`. Ist genau eine Seite ein Mac,
   tauscht der Client Strg und Cmd, damit etwa Strg+C auf dem Mac weiterhin kopiert.
   Meldet das `Welcome` mindestens Version 4, schickt der Client gleich danach `Monitors` (siehe unten).
3. Der Server öffnet den Input-Stream.
4. Erreicht der Cursor eine Kante mit einem verbundenen Nachbarn: `Enter { epoch, x, y }`. Danach folgen
   `MouseMove`-Datagramme mit dieser `epoch` und steigender `seq`.
5. Verlässt der Cursor den Client: `Leave`. Der Client lässt alle Tasten los und schickt sein Clipboard,
   falls es sich geändert hat.

## Mehrere Monitore

`Hello` und `ScreenChanged` beschreiben nur das umschließende Rechteck aller Monitore. Seit Qross 0.4
meldet der Client zusätzlich die einzelnen Monitore als `Monitors(Vec<Rect>)`, in denselben Koordinaten
(Windows und Linux: physische Pixel, macOS: Punkte wie beim Einspeisen). Er schickt die Liste direkt
nach dem `Welcome` und erneut, sobald sich die Anordnung ändert, nach einem eventuellen `ScreenChanged`.
Der Server übernimmt höchstens 16 Monitore und schneidet sie auf das umschließende Rechteck zu.

Mit der Liste gilt auf dem Client:

- Der Cursor bewegt sich nur auf Monitorflächen. Liegt das Ziel einer Bewegung neben allen Monitoren,
  bleibt er am Rand des aktuellen Monitors stehen. Zwischen sich berührenden Monitoren wechselt er frei.
- Zu einem Nachbarn wird nur an Kantenstücken gewechselt, an denen ein Monitor an der äußeren Kante des
  Rechtecks liegt. Die Ecken-Totzone gilt für die Ecken dieses Monitors.
- Beim Betreten wird die Position wie bisher anteilig entlang der Kante umgerechnet und dann auf den
  nächstgelegenen Monitor an der Eintrittskante gelegt.

Dieselben Regeln wendet der Server auf seine eigenen Monitore an. Ohne Liste (ältere Clients) gilt das
Rechteck als ein einziger Monitor, also das bisherige Verhalten.

**Kompatibilität:** `Hello.version` bleibt 3, denn Server bis Qross 0.3 lehnen andere Versionen ab. Eine
unbekannte Control-Nachricht kann ein solcher Server nicht lesen und trennt die Verbindung. Deshalb meldet
ein neuer Server im `Welcome` die Version 4, die ältere Clients nicht auswerten. Nur dann sendet der Client
`Monitors`. Ältere Server melden 3 und bekommen die Nachricht nie.

## Zwischenablage

Übertragen werden alle vorhandenen Darstellungen gleichzeitig: Text, HTML-Ausschnitt und Bild als PNG.
Der Empfänger stellt sie wieder gemeinsam bereit, sodass jedes Programm sein bevorzugtes Format findet.
Unter Windows sind das Unicode-Text, „HTML Format“, „PNG“ und zusätzlich ein DIB für ältere Programme.
Lesen und PNG-Kodierung laufen in einem eigenen Thread, damit große Screenshots die Maus nicht ausbremsen.
Bilder über der Frame-Grenze werden weggelassen, Text und HTML gehen trotzdem mit.

### Kopierte Dateien

1. Liegen beim Wechsel Dateien in der Zwischenablage (Windows `CF_HDROP`, macOS `public.file-url`,
   Linux `text/uri-list`), sammelt der Sender sie rekursiv ein, höchstens bis zur eingestellten Grenze
   (Standard 1 GB, `[clipboard] max_file_mb`, 0 = aus). Darüber bricht er das Einsammeln ab, überträgt keine
   Dateien und meldet das der Oberfläche. Große Dateien gehen per Drag & Drop.
2. Er sendet `Clipboard::Files { transfer, entries, data }`. `data` enthält, was sonst noch in der
   Zwischenablage liegt, zum Beispiel die Dateinamen als Text. Danach folgt jede Datei auf einem eigenen
   Stream vom Typ 4, aufgebaut wie beim Drag & Drop.
3. Der Empfänger schreibt in einen eigenen Ablageordner (`<Cache>/Qross/clipboard/<transfer>`), der
   Neuverbindungen und Programmende übersteht, denn eingefügt wird oft erst später. Er bereinigt die
   Namen wie beim Drag & Drop und prüft jede Datei per SHA-256.
4. Erst wenn alle Dateien angekommen sind, setzt er seine Zwischenablage auf die obersten Dateien und
   Ordner. Unter Windows kommen `CF_HDROP` mit „Preferred DropEffect = Kopieren“ und alle Darstellungen aus
   `data` dazu. Über `arboard` (macOS, Linux) gewinnen die Dateien.
5. Ein neuerer Inhalt verwirft laufende Übertragungen und bricht ihre Streams ab, damit nie Älteres über
   Neueres geschrieben wird. Vorherige Übertragungen werden nach 10 Minuten gelöscht, alles nach einem Tag.

Über den Server gelangen Dateien auch von einem Client zum nächsten: Er legt sie in seine eigene
Zwischenablage und gibt sie beim nächsten Wechsel weiter.

Ältere Gegenstellen (Qross 0.2) kennen weder `Clipboard::Files` noch Stream-Typ 4. Sie verwerfen nur diese
Streams und bleiben verbunden. Text, HTML und Bilder funktionieren mit ihnen weiter.

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

### Vom Client zum Server

1. Der Server weiß, welche Maustasten auf dem Client gedrückt sind. Erreicht der Cursor dort mit gedrückter
   Taste die Kante zum Server, wechselt er nicht sofort, sondern fragt mit `DragMessage::Query { probe }` nach.
2. Der Client prüft, ob gerade Dateien gezogen werden. Unter Windows geschieht das mit demselben unsichtbaren
   OLE-Drop-Ziel wie am Server, unter macOS über die Drag-Zwischenablage des Systems.
3. Zieht er Dateien, bricht er seinen Drag mit Esc ab, antwortet mit `Offer { probe, transfer, entries }` und
   schickt die Dateien auf Streams vom Typ 3. Sonst kommt `Offer` mit leerer Liste. Der Server wechselt dann
   nicht, solange die Taste gedrückt ist; so bleibt ein an den Rand geschobenes Fenster auf dem Client.
4. Mit Dateien wechselt der Cursor zum Server. Lässt der Nutzer die Taste dort los, legt der Server die Dateien
   im Ordner unter dem Cursor ab (Windows: Explorer oder Desktop), sonst in `Downloads/Qross`.
5. Geht der Cursor vorher zurück zu einem Client, wird die Übertragung verworfen.

Ein älterer Client antwortet nie auf die Nachfrage. Mit gedrückter Taste bleibt der Cursor dann auf ihm.

## Tasten

- Medientasten der HID-Consumer-Page (Wiedergabe/Pause, Weiter, Zurück, Stopp) werden als `0xC000 | Usage`
  übertragen. Ältere Gegenstellen kennen sie nicht und ignorieren sie. Ein Mac-Client erzeugt daraus und aus
  Lautstärke und Stumm Systemereignisse (`NX_SYSDEFINED`), denn nur auf diese reagiert macOS.
- Fang-Taste, Medientasten-Ziel und Umbelegung je Rechner wertet nur der Server aus; das Protokoll ändert sich
  dadurch nicht.

## Regeln für Robustheit

- **Mausposition absolut**, in Client-Koordinaten (physische Pixel, virtueller Desktop), immer auf einem
  Monitor des Clients. Ein verlorenes
  Datagramm wird vom nächsten überholt, Fehler summieren sich nicht auf.
- Der Client verwirft Datagramme mit falscher `epoch` (Nachzügler eines früheren Besuchs) und mit nicht
  neuerer `seq` (Umsortierung im Netz). Der Vergleich ist wrap-around-sicher.
- `Button` enthält die Position. So landet ein Klick richtig, auch wenn das letzte Bewegungs-Datagramm fehlt.
- Wechsel nur außerhalb der **Ecken-Totzone** (Standard 8 px) und optional erst nach einer
  **Wechselverzögerung**, also wenn eine bestimmte Zeit gegen die Kante gedrückt wird. Das verhindert
  versehentliche Wechsel beim Zielen auf Startmenü oder Schließen-Knopf.
- Tasten sind **physische Tasten als USB-HID-Usage**. Das Layout des Clients bestimmt das Zeichen.
  Das ist plattformneutral und funktioniert auch mit unterschiedlichen Tastaturlayouts.

## Verbindungsqualität

Die Oberfläche zeigt je Verbindung Latenz, Jitter, Paketverlust und Durchsatz samt einer Stufe an (Server:
je Client unter „Verbundene Rechner“, Client: im Status). Das braucht **keinen Protokollanteil und keinen
zusätzlichen Verkehr**: Jede Seite liest nur die Zähler ihrer eigenen QUIC-Verbindung (`quinn::Connection::stats`),
ältere Gegenstellen merken davon nichts. Weil beide Seiten jede Sekunde ein Keepalive senden, das bestätigt
werden muss, entstehen auch ohne Eingaben laufend neue Messwerte. Implementierung: `crates/qross-net/src/quality.rs`.

Alle 100 ms werden die Zähler gelesen, jede Sekunde entsteht ein Messpunkt:

- **Latenz**: die geglättete Round-Trip-Time von quinn (RFC 9002, neu = 7/8 · alt + 1/8 · Probe).
- **Jitter**: mittlerer Betrag der Differenz aufeinanderfolgender RTT-Proben der letzten 10 s (wie der
  Interarrival-Jitter aus RFC 3550, nur ungewichtet). quinn gibt nur den geglätteten Wert heraus. Kam seit der
  letzten Abfrage genau ein ACK-Frame an, lässt sich die einzelne Probe zurückrechnen: Probe = 8 · neu − 7 · alt.
  Bei mehreren ACKs (viel Verkehr) folgt der geglättete Wert den Proben dicht und dient selbst als Probe; dann
  fällt der Jitter etwas zu niedrig aus.
- **Verlust**: von quinn als verloren erkannte Pakete in Prozent der gesendeten, über 10 s. Bei wenig Verkehr
  wird das Fenster bis auf 60 s verlängert, bis mindestens 100 Pakete beisammen sind, sonst machte ein einziges
  verlorenes Keepalive gleich 5 % aus. Reicht auch das nicht (kurz nach dem Verbinden), wird trotzdem durch 100
  geteilt. Erkennbar ist nur der Verlust in Senderichtung des eigenen Rechners; beim Server ist das die Richtung,
  in der Maus und Tastatur laufen.
- **Durchsatz**: gesendete und empfangene UDP-Bytes pro Sekunde einschließlich QUIC-Overhead, etwa bei
  Dateiübertragungen.

Die Stufe richtet sich nach dem schlechtesten der drei Werte (Grenzen jeweils ausschließlich):

| Stufe | Latenz | Jitter | Verlust |
|---|---|---|---|
| Sehr gut | unter 10 ms | unter 3 ms | unter 0,5 % |
| Gut | unter 30 ms | unter 10 ms | unter 2 % |
| Mäßig | unter 80 ms | unter 25 ms | unter 5 % |
| Schlecht | darüber | darüber | darüber |

## Noch offen

- Drag & Drop mit Linux als Quelle (XDND) und vom Mac-Server aus
- Relative Mausbewegung für Spiele
- Zwischenablage: RTF; virtuelle Dateien (z. B. Outlook-Anhänge) und Dateien erst beim Einfügen übertragen
- Bildschirmschoner-Sync, Strg+Alt+Entf, Secure Desktop (UAC)
- Wayland (libei/InputCapture-Portal)
