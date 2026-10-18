# Qross-Protokoll, Version 3

Seit Qross 0.3 abwärtskompatibel erweitert um kopierte Dateien in der Zwischenablage, seit Qross 0.4 um
Drag & Drop vom Client zum Server, seit Qross 0.5 um die einzelnen Monitore eines Clients, seit Qross 0.6
um den Spielemodus und seit Qross 0.7 um Zeichen statt Tastenpositionen und Rechte je Rechner, danach um die
Messung der Eingabekette (siehe unten).

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
| Control (bidirektionaler Stream, vom Client geöffnet) | ⇄ | ja | `Hello`, `Welcome`/`Reject`, `ScreenChanged`, `Monitors`, `Game`, `TypesText`, `Sharing`, `Latency` |
| Suche (UDP-Broadcast, Port 24901, außerhalb von QUIC) | ⇄ | nein | `Discovery::Probe` / `Announce` |
| Input (unidirektionaler Stream, Priorität 10) | Server → Client | ja, geordnet | `Enter`, `Leave`, `Key`, `Button`, `Wheel`, im Zeichenmodus `Text`, `Shortcut` |
| Datagramm | Server → Client | **nein** | `MouseMove`, im Spielemodus `MouseDelta` |
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
   Ab Version 5 meldet er außerdem mit `Game`, wenn bei ihm etwas im Vollbild läuft (siehe Spielemodus).
   Ab Version 6 meldet er mit `TypesText` direkt nach dem `Welcome`, dass er Zeichen eingeben kann (siehe
   Zeichen statt Tastenposition).
   Ab Version 7 schickt der Server gleich nach dem `Welcome` die Rechte des Clients (`Sharing`, siehe Rechte
   je Rechner).
   Ab Version 13 meldet der Client mit einer leeren `Latency::Report`, dass er die Eingabekette mitmisst (siehe
   Verzögerung der Eingabekette).
3. Der Server öffnet den Input-Stream.
4. Erreicht der Cursor eine Kante mit einem verbundenen Nachbarn: `Enter { epoch, x, y }`. Danach folgen
   `MouseMove`-Datagramme (im Spielemodus `MouseDelta`) mit dieser `epoch` und steigender `seq`.
5. Verlässt der Cursor den Client: `Leave`. Der Client lässt alle Tasten los und schickt sein Clipboard,
   falls es sich geändert hat.

## Mehrere Monitore

`Hello` und `ScreenChanged` beschreiben nur das umschließende Rechteck aller Monitore. Seit Qross 0.5
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

## Spielemodus

Seit Qross 0.6. Server und Clients sehen alle 250 ms in einem eigenen Thread nach, was bei ihnen im
Vordergrund läuft (`GameState { fullscreen, captured }`):

| System | Vollbild (`fullscreen`) | Zeiger eingefangen (`captured`) |
|---|---|---|
| Windows | `SHQueryUserNotificationState` meldet `QUNS_RUNNING_D3D_FULL_SCREEN` oder `QUNS_BUSY`, oder das Vordergrundfenster hat keine Titelleiste und bedeckt seinen Monitor. Desktop und Taskleiste zählen nie. | `GetClipCursor` liefert weniger als den virtuellen Bildschirm |
| macOS | Das fokussierte Fenster ist im Vollbild (`AXFullScreen` über die Bedienungshilfen) oder bedeckt einen Bildschirm samt Menüleiste. | nicht erkennbar: macOS verrät nicht, ob ein Spiel den Zeiger mit `CGAssociateMouseAndMouseCursorPosition` vom Cursor gelöst hat |
| Linux (X11) | Das aktive Fenster (`_NET_ACTIVE_WINDOW`) trägt `_NET_WM_STATE_FULLSCREEN`. | nicht erkannt: Qross müsste dafür selbst kurz nach dem Zeiger greifen (`XGrabPointer`) und könnte dabei Klicks schlucken |

Der Server wertet seinen eigenen Zustand direkt aus. Ein Client meldet seinen mit `Control::Game(GameState)`,
nach dem `Welcome` nur, wenn schon etwas läuft, danach bei jeder Änderung.

Steht der Cursor auf einem Rechner, auf dem gerade etwas im Vollbild läuft oder den Zeiger eingefangen hat:

- Er wechselt nicht, wie mit der Fang-Taste. Die Oberfläche zeigt das an (`ServerNotice::GameHold`).
- Auf einem Client schickt der Server statt der Position die Bewegung selbst:
  `MouseDelta { epoch, seq, dx, dy }` in Pixeln des Servers, nicht geklemmt. So dreht ein Spiel auch am
  Bildschirmrand weiter. Der Client speist sie relativ ein: unter Windows mit `SendInput` ohne
  `MOUSEEVENTF_ABSOLUTE`, sodass Raw Input eine echte Mausbewegung sieht und `ClipCursor` greift; unter macOS
  als Mausereignis mit `kCGMouseEventDeltaX/Y`, ausgehend von der tatsächlichen Cursorposition; unter X11 per
  XTest als relative Bewegung.
- `MouseDelta` teilt sich `epoch` und `seq` mit `MouseMove`. Der Client übernimmt nur Datagramme, die neuer
  sind als das zuletzt übernommene. Ein überholtes Delta fällt weg wie ein verlorenes.
- Der Server führt die Position geklemmt mit, und `Button` trägt sie weiter. Nach einem `MouseDelta` springt
  der Client beim Klicken aber nicht dorthin, sonst sähe ein Spiel einen Ruck. Endet der Spielemodus, kommen
  wieder Positionen.
- Die Fang-Taste löst den Halt, bis der Cursor den Bildschirm wechselt oder dort ein Vollbild beginnt oder
  endet. Bis dahin kommen wieder Positionen, damit der sichtbare Cursor zur Kante findet. Ein zweiter Druck hält
  wie gewohnt fest, und die Deltas laufen dann weiter.
- Abschaltbar mit `[game] enabled = false` in `qross.toml` bzw. „Spielemodus“ in den Einstellungen. Ein Client
  mit ausgeschaltetem Spielemodus meldet nichts (`qross client --no-game-mode`).

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 5. Nur dann sendet der Client `Game`;
Server aus Qross 0.5 (Version 4) und älter bekommen die Nachricht nie. Wer `Game` sendet, versteht auch
`MouseDelta`. Der Server schickt Deltas deshalb nur an Clients, die einen aktiven Zustand gemeldet haben.
Ältere Clients melden nie etwas und bekommen weiter Positionen; ein Vollbild bei ihnen hält den Cursor nicht.
Ein Datagramm, das ein älterer Client nicht lesen kann, würde er nur verwerfen.

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

## Fortschritt und Abbrechen

Für Drag & Drop in beide Richtungen und kopierte Dateien zählen Sender und Empfänger die gesendeten bzw.
empfangenen Bytes je Übertragung (`transfer`). Etwa viermal pro Sekunde geht an die Oberfläche eine Meldung
mit Gesamtgröße, Stand, Rate (gemittelt über drei Sekunden) und geschätzter Restzeit, zum Schluss eine mit
dem Ergebnis: fertig, abgebrochen (hier oder von der Gegenseite) oder fehlgeschlagen.

Abbrechen geht auf beiden Seiten und braucht keine neue Nachricht:

- **Der Sender** setzt den laufenden Datei-Stream zurück (`RESET_STREAM`, Code 0) und öffnet keine
  weiteren. Der Empfänger bekommt beim Lesen den Reset und verwirft die Übertragung samt Staging.
- **Der Empfänger** verwirft das Staging und hält den Stream an (`STOP_SENDING`, Code 0), wie schon beim
  Verwerfen nicht abgelegter oder überholter Übertragungen. Der Sender bekommt beim nächsten Schreiben
  `Stopped` und hört auf.

Ein `Stopped` gilt beim Sender deshalb immer als Abbruch durch die Gegenseite, auch wenn diese nur
verworfen hat. Ältere Versionen reagieren auf beides wie auf einen Fehler im Stream: Sie verwerfen die
Übertragung bzw. hören auf zu senden und bleiben verbunden. Bricht die Verbindung ab, enden alle
Übertragungen mit diesem Rechner als fehlgeschlagen; was schon im Staging lag, wird gelöscht.

## Rechte je Rechner

Der Server legt je Client fest, was geteilt wird, und setzt es selbst durch. Drei Arten, jede mit Richtung
(`Share`: `Both`, `ToClient`, `FromClient`, `Off`; in `qross.toml` `"both"`, `"to-client"`, `"from-client"`,
`"off"`):

| Art | Betrifft |
|---|---|
| `clipboard` | `Clipboard::Data` und den Text, den `Clipboard::Files` mitbringt (`data`) |
| `files` | `Clipboard::Files` und Streams vom Typ 4 |
| `drag` | `Input::DragStart`/`Drop`, Streams vom Typ 3 und 5 |

„Vom Client“ heißt: zum Server und von dort weiter zu anderen Clients, soweit diese empfangen dürfen.

- **Senden:** Der Server liest seine Zwischenablage für einen Client nur, wenn `clipboard` oder `files` zu ihm
  erlaubt ist, und schickt nur den erlaubten Teil: ohne `files` gehen kopierte Dateien als ihr Text, ohne
  `clipboard` ohne Text. Ohne `drag` zum Client wechselt der Cursor mit gedrückter Taste nicht zu ihm und der
  Server prüft gar nicht erst, ob Dateien gezogen werden.
- **Empfangen:** Was ein Client nicht abgeben darf, verwirft der Server beim Eintreffen: `Clipboard::Data`
  ganz, `Clipboard::Files` bis auf den erlaubten Text, `Offer` als „keine Dateien“. Datei-Streams vom Typ 3
  und 4 hält er an (`STOP_SENDING`, Code 0), bevor etwas davon im Staging landet. Ohne `drag` vom Client fragt
  er nicht mit `Query` nach; mit gedrückter Taste bleibt der Cursor dann auf dem Client.
- **Ändern im Betrieb:** Neue Rechte gelten sofort, die Verbindung bleibt. Laufende Übertragungen, die nicht
  mehr erlaubt sind, bricht der Server ab wie mit „Abbrechen“ (siehe oben), Ankommendes wird gelöscht.
- **Mitteilen:** Nach dem `Welcome` und nach jeder Änderung schickt der Server auf dem Control-Stream
  `Sharing { clipboard, files, drag }` aus Sicht des Clients. Ein Client ab Qross 0.7 zeigt das an und
  versucht Verbotenes nicht: Er behält seine Zwischenablage, schickt ohne `files` nur den Text und antwortet
  auf ein `Query` ohne Recht mit „keine Dateien“, ohne seinen Drag abzubrechen.

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 7. Clients bis Qross 0.6 lesen den
Control-Stream nach dem `Welcome` nicht mehr; `Sharing` bleibt bei ihnen ungelesen liegen. Sie kennen ihre
Rechte nicht und senden wie bisher; der Server weist Verbotenes ab, sie bleiben verbunden. Ein neuer Client
liest den Control-Stream nur bei Version 7 und mehr und überspringt Nachrichten, die er nicht kennt. Ältere
Server schicken nichts; dann ist alles erlaubt.

## Tasten

- Medientasten der HID-Consumer-Page (Wiedergabe/Pause, Weiter, Zurück, Stopp) werden als `0xC000 | Usage`
  übertragen. Ältere Gegenstellen kennen sie nicht und ignorieren sie. Ein Mac-Client erzeugt daraus und aus
  Lautstärke und Stumm Systemereignisse (`NX_SYSDEFINED`), denn nur auf diese reagiert macOS.
- Fang-Taste, Wechsel-Kürzel, Medientasten-Ziel und Umbelegung je Rechner wertet nur der Server aus; das
  Protokoll ändert sich dadurch nicht. Ein Wechsel per Kürzel sieht für den Client aus wie ein Wechsel über die
  Kante (`Leave`, `Enter` mit neuer `epoch`); die Taste des Kürzels selbst bekommt er nie.

## Zeichen statt Tastenposition

Seit Qross 0.7, je Client einstellbar (`[keys.mode]` in `qross.toml`, Einstellungen → Tastatur). Standard bleibt
die Tastenposition: Das Layout des Clients bestimmt das Zeichen, wie bei einer dort angeschlossenen Tastatur.
Im Zeichenmodus kommt dagegen das Zeichen an, das die Taste auf dem **Server** ergibt, egal welches Layout der
Client hat.

Übersetzt wird beim Server, denn nur er kennt sein Layout samt aktuellem Zustand (Gruppe, Feststelltaste,
Layout je Programm unter Windows). Seine Erfassung meldet im Remote-Modus zu jeder Zeichentaste (Buchstaben,
Ziffern der oberen Reihe, Satzzeichen, Leertaste, ISO-Taste „<>“), was sie ergibt (`CaptureEvent::Typed`):

| System | Wie der Server sein Layout liest | Kürzel statt Text |
|---|---|---|
| Windows | `ToUnicodeEx` mit Flag 4 (ab Windows 10 1607) für das Layout des Vordergrundfensters; das Flag lässt den Zustand toter Tasten im System unverändert | Strg, Alt ohne AltGr, Windows-Taste; Strg+Alt wie AltGr, wenn es ein Zeichen ergibt |
| macOS | `UCKeyTranslate` mit dem `uchr`-Layout der aktuellen Eingabequelle; die Eingabequelle liest Qross auf dem Hauptthread und arbeitet mit einer Kopie | Cmd, Ctrl (Option tippt Zeichen) |
| Linux (X11) | Kern-Mapping des X-Servers samt Gruppe, Ebene (Umschalt, AltGr = ISO_Level3_Shift), Feststelltaste und toter Tasten (`dead_*`) | Strg, Alt ohne AltGr, Super |
| Wayland | – (das Layout käme nur über libxkbcommon): Der Server schickt immer Tastenpositionen | |

Daraus macht der Server (`qross_core::chars::Typist`) für einen Client im Zeichenmodus:

- `Input::Text { key, text }`: Der Client gibt `text` ein und hält das letzte Zeichen, bis `Key { key, down:
  false }` kommt. Eine gehaltene Taste wiederholt `Text`.
- Tote Tasten (^ ´ ` und weitere) verbindet der Server selbst mit dem nächsten Zeichen (`^` + `e` = `ê`); gibt es
  die Verbindung nicht, kommen beide Zeichen, nach der Leertaste nur der Akzent. Eine andere Taste verwirft sie.
- Zusatztasten hält der Server zurück: Für Text gehen Umschalt, AltGr und Option nie zum Client (dort wären sie
  womöglich Alt und machten aus „@“ ein Kürzel). Vor einem Kürzel, einer anderen Taste oder einem Klick bzw. einer
  Raddrehung (Umschalt+Klick, Strg+Rad) gehen sie als `Key` raus, allein gedrückt (Windows-Taste fürs Startmenü)
  beim Loslassen.
- `Input::Shortcut { key, ch }` für Kürzel wie Strg+Z: Der Client drückt die Taste, die in **seinem** Layout `ch`
  ergibt, sodass Strg+Z dort „Rückgängig“ bleibt; kennt er das Zeichen nicht, die Taste `key`.
- Pfeile, F-Tasten, Eingabe, Rücktaste, Tab, Esc, Entf, Pos1/Ende, Ziffernblock und Medientasten bleiben `Key`.
  Eine umbelegte Taste (`[keys.remap]`) bleibt immer eine Taste. Die Feststelltaste wirkt nur auf das Layout des
  Servers und geht nicht zum Client.

Der Client gibt Zeichen so ein: Windows `SendInput` mit `KEYEVENTF_UNICODE` (`VK_PACKET`, außerhalb der
Basisebene als Ersatzzeichenpaar), macOS ein Tastenereignis mit `CGEventKeyboardSetUnicodeString` ohne
Zusatztasten, X11 über eine Taste, die das Zeichen schon ergibt, sonst über einen freien Keycode, den Qross
vorübergehend mit dem Zeichen belegt (`ChangeKeyboardMapping`, wie `xdotool type`). Für `Shortcut` sucht er die
Taste: Windows `VkKeyScanEx`, macOS `UCKeyTranslate` rückwärts, X11 das Kern-Mapping.

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 6. Nur dann sendet der Client `TypesText`,
und nur, wenn er Zeichen eingeben kann (nicht unter Wayland/libei). Server bis Qross 0.6 bekommen die Nachricht
nie. `Text` und `Shortcut` schickt der Server nur an Clients, die `TypesText` gesendet haben; ein älterer Client
könnte sie nicht lesen, und sein ganzer Input-Stream bräche ab. Alle anderen bekommen weiter Tastenpositionen,
auch wenn sie auf „Zeichen“ stehen; die Oberfläche zeigt das beim Rechner an.

## Regeln für Robustheit

- **Mausposition absolut**, in Client-Koordinaten (physische Pixel, virtueller Desktop), immer auf einem
  Monitor des Clients. Ein verlorenes
  Datagramm wird vom nächsten überholt, Fehler summieren sich nicht auf. Nur im Spielemodus gehen
  Bewegungen relativ; ein verlorenes Delta fehlt dann wie bei einer echten Maus ein Ruckeln.
- Der Client verwirft Datagramme mit falscher `epoch` (Nachzügler eines früheren Besuchs) und mit nicht
  neuerer `seq` (Umsortierung im Netz). Der Vergleich ist wrap-around-sicher.
- `Button` enthält die Position. So landet ein Klick richtig, auch wenn das letzte Bewegungs-Datagramm fehlt.
- Wechsel nur außerhalb der **Ecken-Totzone** (Standard 8 px) und optional erst nach einer
  **Wechselverzögerung**, also wenn eine bestimmte Zeit gegen die Kante gedrückt wird. Das verhindert
  versehentliche Wechsel beim Zielen auf Startmenü oder Schließen-Knopf.
- Tasten sind **physische Tasten als USB-HID-Usage**. Das Layout des Clients bestimmt das Zeichen.
  Das ist plattformneutral und funktioniert auch mit unterschiedlichen Tastaturlayouts. Wer bei
  unterschiedlichen Layouts lieber die Zeichen des Servers will, stellt den Client auf „Zeichen“ (siehe oben).

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

## Verzögerung der Eingabekette

Ab Welcome-Version 13 misst Qross nebenbei, wie lange eine Eingabe vom Server bis zum Einspeisen am Client
braucht, aufgeteilt in drei Teile und für Mausbewegungen und Tasten getrennt, als Median und 95. Perzentil der
letzten Minute. Die Oberfläche zeigt das im aufgeklappten Panel der Verbindungsqualität (Server: je Client, Client:
im Status). Implementierung: `crates/qross-net/src/latency.rs`, Wire-Typen in `crates/qross-proto/src/latency.rs`.

| Teil | Von | Bis | Gemessen mit |
|---|---|---|---|
| Server | Ereignis aus der Erfassung abgeholt | an QUIC übergeben (`send_datagram` bzw. `write_frame` zurück) | Uhr des Servers |
| Netz | an QUIC übergeben | beim Client gelesen (Datagramm bzw. Nachricht) | beide Uhren, Versatz geschätzt |
| Client | beim Client gelesen | Einspeisen im Einspeise-Thread erledigt | Uhr des Clients |

**Stichproben ohne zusätzliche Bytes auf dem Eingabeweg.** Beide Seiten erkennen dieselben Eingaben ohne Absprache:
Maus-Datagramme (`MouseMove`, `MouseDelta`), deren `seq` durch 32 teilbar ist (bei 125 Hz etwa 4, bei 1000 Hz
etwa 30 je Sekunde), und jeder Druck einer Taste oder Maustaste auf dem Input-Stream (`Key` und `Button` mit
`down`, `Text`, `Shortcut`), erkannt an ihrer laufenden Nummer dort (ab 1). Der Server merkt sich für jede
Stichprobe, wann er sie erfasst und gesendet hat; der Client nimmt beim Lesen die Zeit und reiht hinter die
Einspeise-Aufträge eine Markierung ein, die der Einspeise-Thread mit der Zeit nach dem letzten Auftrag beantwortet.

**Rückmeldung.** Der Client sammelt höchstens 200 ms und schickt dann auf dem Control-Stream
`Latency::Report { sent_us, samples }`: seine Uhr beim Absenden und je Stichprobe `id` (`Mouse { epoch, seq }`
bzw. `Input { index }`), `age_us` (so lange vor `sent_us` kam sie an) und `client_us`. Eine Rückmeldung mit bis
zu acht Stichproben ist etwa 100 Bytes groß. Gleich nach dem `Welcome` schickt er eine leere; daran erkennt der
Server, dass der Client mitmisst. Der Server rechnet und schickt etwa jede Sekunde, wenn sich etwas geändert hat,
`Latency::Summary` mit dem Ergebnis zurück (`Chain` je Art: Server-, Netz-, Client-Anteil und Summe je Stichprobe
als `Spread { p50_us, p95_us }`, dazu `uncertainty_us`).

**Uhren.** Die Uhren der beiden Rechner haben verschiedene Nullpunkte und laufen leicht verschieden schnell. Jede
Stichprobe ergibt mit ihrer Rückmeldung einen Austausch wie bei NTP: Versand am Server T1, Empfang am Client T2,
Rückmeldung T3 = `sent_us`, Ankunft beim Server T4, mit T2 = T3 − `age_us`. Daraus folgen die Umlaufzeit
δ = (T4 − T1) − (T3 − T2) und der Versatz θ = ((T2 − T1) + (T3 − T4)) / 2. Es gilt der Versatz des Austauschs der
letzten 10 s mit dem kleinsten δ / 2 + 15 µs je Sekunde Alter (wie `PHI` bei NTP: so weit können Quarzuhren je
Sekunde auseinanderlaufen). Der Netzanteil einer Stichprobe ist T2 − T1 − θ, begrenzt auf 0 bis δ.

**Genauigkeit und Grenzen:**

- Server- und Client-Anteil messen je eine Uhr (monoton, unter Windows QueryPerformanceCounter) und sind auf etwa
  eine Mikrosekunde genau.
- Den Netzanteil kann ohne gemeinsame Uhr niemand genau kennen. Für den besten Austausch ist er δ / 2, richtig nur,
  wenn Hin- und Rückweg gleich lang dauern. Er liegt höchstens um diesen Wert daneben; die Oberfläche zeigt ihn als
  „Netzanteil auf ± … genau“ (auf Loopback rund 0,2 ms, im LAN einige zehntel Millisekunden, im WLAN mehr).
  Schwankungen einzelner Stichproben, etwa ein voller Puffer im WLAN, erscheinen dagegen in voller Höhe, denn
  sie bleiben im Unterschied T2 − T1 erhalten.
- Die Wartezeit beim Client bis zur Rückmeldung (bis 200 ms) geht mit der Gangabweichung der Uhren ein, bei 50 ppm
  also mit höchstens 10 µs.
- Nicht enthalten: der Weg vom Eingabegerät durch das Betriebssystem bis zum Hook und der Thread-Wechsel vom Hook
  bis zur Ereignisschleife des Servers (dafür müsste jede Erfassung selbst Zeitstempel setzen), außerdem alles nach
  dem Einspeisen bis zum Bildschirm. In der Oberfläche als Server zählt die Weitergabe der Erfassung an den
  laufenden Server mit, sie geschieht vor dem Abholen.
- Bei zusammengefassten Mausbewegungen (mehrere warten schon) zählt die Zeit der ersten. Verlorene oder vom Client
  verworfene Datagramme fehlen in Netz- und Client-Anteil, im Server-Anteil zählen sie mit.

Messen auf einem Rechner ohne echte Erfassung und Einspeisung: `cargo run -p qross-net --example latency_probe`
(Client im selben Prozess) bzw. mit `server` und `qross client … --dry-run` in einem zweiten Fenster.

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 13. Nur dann schickt der Client `Latency`;
ältere Server bekommen die Nachricht nie (sie trennten bei unbekannten Control-Nachrichten). Das Ergebnis schickt
der Server nur Clients, die sich gemeldet haben; Clients aus 0.7 lesen den Control-Stream zwar, bekommen es aber
nie. Für ältere Clients misst der Server nur seinen eigenen Anteil, die Oberfläche zeigt dann nur ihn. Ein neuer
Client an einem älteren Server misst nichts, dort entfällt die Anzeige.

## Noch offen

- Drag & Drop mit Linux als Quelle (XDND) und vom Mac-Server aus
- Spielemodus: eingefangenen Zeiger unter macOS und Linux erkennen; Rohdaten der Maus am Server
  (Raw Input) statt Cursor-Deltas, damit Zeigerbeschleunigung des Servers nicht mitwandert
- Zwischenablage: RTF; virtuelle Dateien (z. B. Outlook-Anhänge) und Dateien erst beim Einfügen übertragen
- Bildschirmschoner-Sync, Strg+Alt+Entf, Secure Desktop (UAC)
- Wayland (libei/InputCapture-Portal)
- Verzögerung der Eingabekette: Zeitstempel schon im Hook der Erfassung setzen, damit der Thread-Wechsel bis zur
  Ereignisschleife mitzählt
