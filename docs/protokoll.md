# Qross-Protokoll, Version 3

Seit Qross 0.3 abwärtskompatibel erweitert um kopierte Dateien in der Zwischenablage, seit Qross 0.4 um
Drag & Drop vom Client zum Server, seit Qross 0.5 um die einzelnen Monitore eines Clients, seit Qross 0.6
um den Spielemodus, seit Qross 0.7 um Zeichen statt Tastenpositionen und Rechte je Rechner, seit Qross 0.8
um den Abgleich von Feststelltaste, Num und Rollen, den Cursor-Finder, den Bildschirmschoner und die Messung
der Eingabekette, seit Qross 0.13 um kopierte Dateien auf Abruf, seit Qross 0.14 um CPU und Arbeitsspeicher je
Rechner und danach um deren Aufteilung nach Art der Prozesse (siehe unten).

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
- Ruhezustand und Netzwechsel (nach Qross 0.7, `qross_net::wake`, ohne neue Nachrichten): Vor dem Ruhezustand
  schließt die Seite, die einschläft, die Verbindung mit dem QUIC-Anwendungsfehlercode **1**; ein Client
  verbindet sich erst nach dem Aufwachen neu, einer, dessen Server schläft, wartet 1 s. Nach dem Aufwachen
  und bei neuen eigenen Adressen wechselt der Client auf ein neues UDP-Socket (QUIC-Migration, danach ein
  PING). Kommt binnen 1,5 s nichts zurück, baut er sofort neu auf; ein Stateless Reset des Servers heißt
  ebenfalls: sofort neu. Der Server trennt nach dem eigenen Aufwachen jeden Client, von dem 2 s lang nichts
  kommt. Ein Verbindungsaufbau gibt nach 2 s auf; der nächste Versuch folgt nach 0,25 s, verdoppelt bis 1 s,
  nach einer Minute ohne Erfolg bis 3 s. Ältere Gegenstellen werten die Fehlercodes nicht aus.

## Kanäle

| Kanal | Richtung | Zuverlässig | Inhalt |
|---|---|---|---|
| Control (bidirektionaler Stream, vom Client geöffnet) | ⇄ | ja | `Hello`, `Welcome`/`Reject`, `ScreenChanged`, `Monitors`, `Game`, `TypesText`, `Sharing`, `SyncsLocks`, `CursorFinder`, `FollowsScreenSaver`, `ScreenSaver`, `Latency`, `PastesOnDemand`, `ReportsUsage`, `WatchUsage`, `Usage`, `UsageDetail` |
| Suche (UDP-Broadcast, Port 24901, außerhalb von QUIC) | ⇄ | nein | `Discovery::Probe` / `Announce` |
| Input (unidirektionaler Stream, Priorität 10) | Server → Client | ja, geordnet | `Enter`, `Leave`, `Key`, `Button`, `Wheel`, im Zeichenmodus `Text`, `Shortcut`, `Locks` |
| Datagramm | Server → Client | **nein** | `MouseMove`, im Spielemodus `MouseDelta` |
| Clipboard (je Übertragung ein eigener Stream, Priorität −10) | ⇄ | ja | `Clipboard::Data { text, html, png }`, `Clipboard::Files { transfer, entries, data }`, `Clipboard::FilesOnDemand { transfer, entries, data }` |
| Datei für Drag & Drop (je Datei ein eigener Stream, Priorität −10 und tiefer) | ⇄ | ja | `FileHeader`, Rohbytes, `FileTrailer` (SHA-256) |
| Datei aus der Zwischenablage (wie oben) | ⇄ | ja | `FileHeader`, Rohbytes, `FileTrailer` (SHA-256) |
| Drag (je Nachricht ein eigener Stream, Priorität 10) | ⇄ | ja | `DragMessage::Query` / `Offer` |
| Abruf (je Datei ein eigener bidirektionaler Stream, vom Empfänger geöffnet, Priorität −10) | ⇄ | ja | `FileRequest`; zurück `FileReply`, Rohbytes, `FileTrailer` (SHA-256) |

Jeder Stream außer dem Control-Stream beginnt mit einem Byte `StreamKind` (1 = Input, 2 = Clipboard, 3 = Datei
für Drag & Drop, 4 = Datei aus der Zwischenablage, 5 = Drag-Nachricht, 6 = Abruf). Dateien für Drag & Drop
fließen seit Qross 0.4 in beide Richtungen.

Durch die Trennung warten Eingaben nie hinter Clipboard und Dateien im selben Stream; das war im Benchmark der
zweite große Schwachpunkt von Synergy. Ganz unabhängig sind sie trotzdem nicht: Alle Kanäle einer Verbindung
teilen sich die Leitung, quinns Staukontrolle und sein Sendefenster (10 MB für alles, was geschrieben, aber noch
nicht bestätigt ist). Deshalb, nach Qross 0.12.2 und ohne Änderung am Protokoll:

- **Datei-Streams:** Höchstens drei fertig geschriebene Datei-Streams warten auf die Bestätigung der Gegenstelle
  (`SendStream::stopped`), mit dem gerade geschriebenen vier. Ohne Grenze füllten viele Dateien das Sendefenster,
  und ein Tastendruck wartete auf dem Input-Stream, bis wieder Platz war. Jede Datei bekommt eine Stufe niedrigere
  Priorität als die vorige (−10, −11, …): So geht die älteste zuerst ganz hinaus. Liefen die wartenden reihum,
  würden sie zugleich fertig, und die Leitung stünde kurz still und bekäme danach einen Schwall.
- **Zwischenablage beim Wechsel:** Große Inhalte schickt der Server erst, wenn die Maus nach dem Wechsel ruht
  (siehe [Zwischenablage](#zwischenablage)).
- **Datagramme:** Die Warteschlange für ausgehende Datagramme fasst 4 KB, etwa 100 Mausbewegungen; kann nichts
  hinaus, verdrängt die neueste die älteste. Mit quinns Standard (1 MB) lagen nach einem Hänger im WLAN über 20 s
  Bewegungen bereit, und der Cursor fuhr die alte Bahn nach.

Was bleibt: Füllt eine große Übertragung die Leitung, warten Eingaben in der Warteschlange am Engpass mit
(Messung: [benchmark-ergebnisse.md](benchmark-ergebnisse.md), Abschnitt 3c).

## Kodierung

- Stream-Frames: `u32` Länge (big-endian) + [postcard](https://postcard.jamesmunns.com/)-kodierte Nachricht, maximal 64 MiB;
  das `Hello` höchstens 16 KiB (es kommt, bevor der Client geprüft ist)
- Datagramme: reines postcard (`enum Datagram`), eine Mausbewegung braucht ca. 8–12 Bytes

## Ablauf

0. Optional: Ohne angegebene Adresse sucht der Client per UDP-Broadcast (`Discovery::Probe`) auf jeder
   IPv4-Schnittstelle. Jeder Server antwortet mit Name, Port, Fingerprint und Betriebssystem. Die Antwort
   ist nur ein Vorschlag; vertraut wird erst beim Verbinden über den Fingerprint.
1. Client öffnet den Control-Stream und sendet `Hello { version, name, screen, platform }`. Bis hierher kommt
   jeder mit irgendeinem Zertifikat; deshalb nimmt der Server höchstens 16 KiB an und wartet höchstens 3 s nach
   dem Handshake darauf (nach Qross 0.12.2), sonst trennt er.
2. Server prüft Version, Namen (muss im Layout stehen) und Fingerprint und antwortet mit
   `Welcome { version, …, platform }` oder `Reject { reason }`. Ist derselbe Client (Name und Fingerprint)
   noch verbunden, ersetzt die neue Verbindung die alte; die alte schließt er mit dem Anwendungsfehlercode
   **2**, und ein Client, der ihn noch empfängt, hört auf (nach Qross 0.7; bis dahin lehnte der Server ab).
   Ist genau eine Seite ein Mac, tauscht der Client Strg und Cmd, damit etwa Strg+C auf dem Mac weiterhin
   kopiert.
   Meldet das `Welcome` mindestens Version 4, schickt der Client gleich danach `Monitors` (siehe unten).
   Ab Version 5 meldet er außerdem mit `Game`, wenn bei ihm etwas im Vollbild läuft (siehe Spielemodus).
   Ab Version 6 meldet er mit `TypesText` direkt nach dem `Welcome`, dass er Zeichen eingeben kann (siehe
   Zeichen statt Tastenposition).
   Ab Version 7 schickt der Server gleich nach dem `Welcome` die Rechte des Clients (`Sharing`, siehe Rechte
   je Rechner).
   Ab Version 8 meldet der Client mit `SyncsLocks` direkt nach dem `Welcome`, dass er Feststelltaste, Num und
   Rollen setzen kann (siehe Feststelltaste, Num und Rollen). Ab Version 9 schickt der Server danach den
   Cursor-Finder (`CursorFinder`, sofern eingeschaltet).
   Ab Version 10 meldet der Client mit `FollowsScreenSaver` direkt nach dem `Welcome`, dass er dem
   Bildschirmschoner des Servers folgen kann (siehe Bildschirmschoner).
   Ab Version 11 meldet der Client mit einer leeren `Latency::Report`, dass er die Eingabekette mitmisst (siehe
   Verzögerung der Eingabekette).
   Ab Version 12 meldet der Client mit `PastesOnDemand` direkt nach dem `Welcome`, dass er kopierte Dateien auf
   Abruf einfügt; der Server schickt dasselbe nach dem `Welcome`, wenn er es selbst tut (siehe Kopierte Dateien
   auf Abruf).
   Ab Version 13 meldet der Client mit `ReportsUsage` direkt nach dem `Welcome`, dass er auf Nachfrage CPU und
   Arbeitsspeicher meldet (siehe CPU und Arbeitsspeicher).
3. Der Server öffnet den Input-Stream.
4. Erreicht der Cursor eine Kante mit einem verbundenen Nachbarn: `Enter { epoch, x, y }`, bei Clients mit
   `SyncsLocks` gleich gefolgt von `Locks`. Danach folgen `MouseMove`-Datagramme (im Spielemodus `MouseDelta`)
   mit dieser `epoch` und steigender `seq`. Hat sich seine Zwischenablage geändert, schickt der Server sie mit
   (siehe Zwischenablage). Muss ein Paket des Input-Streams nachgeschickt werden, kommen die ersten Datagramme
   vor dem `Enter` an; der Client hebt das neueste auf und speist es gleich nach dem `Enter` ein (nach Qross 0.12.2;
   bis dahin verwarf er sie).
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

Der Server schickt seine Zwischenablage beim `Enter`, wenn sie sich seit dem letzten Mal geändert hat, der
Client seine beim `Leave`. Text geht sofort mit. Bilder, kopierte Dateien und alles über 64 KB schickt der
Server erst, wenn die Maus nach dem Wechsel 200 ms ruht, höchstens 1 s danach (nach Qross 0.12.2): Genau beim
Wechsel bewegt man drüben die Maus, und die ersten Bewegungen sollen die Leitung für sich haben. Kam inzwischen
Neueres an den Client oder von ihm, entfällt der ältere Inhalt.

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

### Kopierte Dateien auf Abruf

Seit Qross 0.13 (Welcome-Version 12). Fügt der Empfänger auf Abruf ein, geht beim Wechsel nur die Liste hinüber;
eine Datei wird erst übertragen, wenn ein Programm sie einfügt. Damit entfallen die Grenze für kopierte Dateien
und Übertragungen, die nie eingefügt werden. Implementierung: `crates/qross-net/src/ondemand.rs`,
`crates/qross-platform/src/win/virtual_clipboard.rs`.

1. **Wer kann:** Bisher nur Windows. Der Empfänger legt dort „virtuelle Dateien“ in die Zwischenablage, wie
   Outlook für Anhänge oder Remotedesktop: `FileGroupDescriptorW` (relative Pfade mit `\`, Größe, Attribute,
   Fortschrittsanzeige) und `FileContents` mit einem `IStream` je Datei, dazu wie bisher die Darstellungen aus
   `data` und „Preferred DropEffect = Kopieren“. Der Explorer legt die Ordner aus der Liste an und holt die
   Inhalte erst beim Einfügen, mit seinem eigenen Fortschrittsdialog. Abschaltbar mit der Umgebungsvariablen
   `QROSS_PASTE_ON_DEMAND=0`, falls ein Programm nur echte Dateien annimmt (wie bei Remotedesktop gilt: Explorer,
   Desktop und Outlook nehmen virtuelle Dateien an, nicht jedes andere Programm).
2. **Anmelden:** Ein Client, der auf Abruf einfügt, schickt `Control::PastesOnDemand` direkt nach dem
   `Welcome`, nur ab Version 12 (ältere Server trennten bei der unbekannten Nachricht). Ein Server, der auf
   Abruf einfügt, schickt es nach dem `Welcome` an jeden Client; Clients bis Qross 0.12 überspringen es (bis 0.6
   lesen sie den Control-Stream gar nicht). Ein Server mit mehreren Clients im Layout tut es nicht: Was ein
   Client kopiert, gibt er als Datei an den nächsten weiter, und dafür braucht er sie ganz.
3. **Senden:** Wer weiß, dass die Gegenstelle auf Abruf einfügt, schickt beim Wechsel
   `Clipboard::FilesOnDemand { transfer, entries, data }` wie `Clipboard::Files`, aber ohne Datei-Streams und
   ohne Größengrenze (`max_file_mb = 0` schaltet kopierte Dateien weiterhin ganz aus). Mehr als 10 000 Dateien
   und Ordner gehen wie bisher vorab. Der Sender merkt sich die Dateien je Gegenstelle; eine neuere
   Zwischenablage an dieselbe Gegenstelle überholt sie, abrufbar bleiben sie noch 10 Minuten nach dem letzten
   Abruf (ein Einfügen kann gerade laufen).
4. **Ablegen:** Der Empfänger bereinigt die Pfade wie beim Vorab-Übertragen und legt die Liste sofort in seine
   Zwischenablage. Die Oberfläche meldet „bereit – mit Strg+V einfügen“, ohne dass ein Byte übertragen wurde.
5. **Abruf:** Liest ein Programm eine Datei, öffnet der Empfänger einen bidirektionalen Stream vom Typ 6 mit
   `FileRequest { transfer, index, offset, paste }`. Der Sender antwortet mit `FileReply::Data { size }`, gefolgt
   von genau `size` Rohbytes ab `offset` und einem `FileTrailer` mit deren SHA-256, oder mit
   `FileReply::Unavailable { reason }` (überholt und abgelaufen, Datei fehlt oder hat seit dem Kopieren eine
   andere Größe, Teilen nicht mehr erlaubt). Liest das Programm ab einer anderen Stelle, beginnt ein neuer Abruf
   ab dort. Das letzte Stück gibt der Empfänger erst heraus, wenn die Prüfsumme stimmt; sonst bekommt das
   Programm einen Lesefehler, und der Explorer verwirft die Datei.
6. **Gegendruck:** Der Empfänger hält höchstens 8 Stücke zu 256 KB vor. Liest das Programm langsamer (oder
   wartet es auf eine Rückfrage), bremst die Flusskontrolle von QUIC den Sender.
7. **Nach einer Neuverbindung** holt der Empfänger über die neue Verbindung; der Sender behält seine Dateien.

**Windows im Einzelnen:** `OleSetClipboard` verlangt ein STA; Qross betreibt dafür einen eigenen Thread mit
Nachrichtenschleife, an dem die Aufrufe des Explorers (`GetData`) und das Nachreichen von Text und Bild für
Programme ohne OLE ankommen. Die Streams leben dagegen im Multithread-Apartment (MTA): `GetData` gibt einen
Proxy auf einen dort gemarshalten Stream heraus, und COM verbindet den Explorer direkt mit dem MTA. So wartet
ein `Read` auf das Netz, ohne die Zwischenablage für andere Programme anzuhalten, und zwei gleichzeitige
Einfügevorgänge stören sich nicht. Ein Pfad muss in `MAX_PATH` passen (259 Zeichen relativ zum Zielordner),
sonst scheitert das Ablegen mit Meldung; der Explorer könnte so eine Datei ohnehin nicht anlegen.

**Fortschritt und Abbrechen:** Alle Abrufe eines Einfügevorgangs tragen dieselbe Nummer `paste`, die der
Empfänger vergibt; beide Seiten zeigen sie als eine Übertragung (Art „kopierte Dateien“) unter dieser Nummer.
Liest ein Programm eine Datei, die sein laufender Vorgang schon hatte, ist das ein zweites Einfügen mit eigener
Nummer. Ein Vorgang endet, wenn jede Datei einmal ganz übertragen ist, beim ersten Fehler oder Abbruch, sonst
5 s nach dem letzten Abruf (der Explorer hat Dateien übersprungen); die Gesamtgröße ist dann das Übertragene.
Abgebrochen wird wie bei den anderen Übertragungen über die Streams: Der Empfänger hält den Stream an
(`STOP_SENDING`), auch wenn im Explorer „Abbrechen“ gedrückt wird, der Sender setzt ihn zurück
(`RESET_STREAM`). Das Programm bekommt dann „abgebrochen“ (`ERROR_CANCELLED`), der Explorer löscht die halbe
Datei ohne Fehlermeldung, macht aber mit der nächsten weiter. Ein abgebrochener Vorgang bricht deshalb auch die
folgenden Abrufe gleich ab, bis 3 s lang keiner mehr kommt; der Sender lehnt Abrufe mit dieser Nummer ab.

**macOS und Linux** übertragen weiter vorab. Der Finder nimmt Dateiversprechen (`NSFilePromiseProvider`) nur
beim Ziehen an, beim Einfügen aus der Zwischenablage nicht: Apple beschreibt sie nur für Drag & Drop, und wer sie
in die allgemeine Zwischenablage legt, bekommt nie den Aufruf zum Schreiben der Datei (Apple-Entwicklerforum,
nicht selbst am Mac geprüft). Dort ginge es nur über eine File-Provider-Erweiterung, deren Platzhalter macOS beim
Kopieren nachlädt, oder über eine Datei-URL, die erst beim Einfügen geliefert wird; dann wartet der Finder aber
ohne Fortschritt, bis die Datei da ist.

**Kompatibilität:** Ältere Gegenstellen bekommen weiter alles vorab. Einem Server unter Version 12 schickt der
Client kein `PastesOnDemand`; ein Client bis 0.12 überspringt das des Servers und schickt `Clipboard::Files`
samt Inhalt, das ein neuer Server wie bisher annimmt. `Clipboard::FilesOnDemand` und Streams vom Typ 6 gibt es
nur zwischen Gegenstellen, die beide Bescheid wissen; eine ältere verwürfe die Nachricht nur.

## Drag & Drop von Dateien

1. Mit gedrückter Maustaste an einer Kante prüft der Server, ob gerade Dateien gezogen werden. Ohne Dateien,
   etwa beim Verschieben eines Fensters, wird nicht gewechselt.
   - Windows: Kurz erscheint ein unsichtbares OLE-Drop-Ziel unter dem Cursor und liest die Dateiliste des
     laufenden Drags (`CF_HDROP`).
   - macOS: Hat sich die Drag-Zwischenablage (`NSPasteboardNameDrag`, `changeCount`) seit dem Drücken der linken
     Maustaste geändert, läuft ein neuer Drag; seine Datei-URLs sind die Dateien.
   - X11: Wer zieht, wird Besitzer von `XdndSelection`. Hat es seit dem Drücken einen Besitzwechsel gegeben
     (XFixes), fragt der Server die Auswahl als `text/uri-list` ab.
   - Wayland: nicht möglich; kein Programm sieht fremde Drag-&-Drop-Vorgänge.

   Jeder Drag zählt nur einmal: Kommt der Cursor mit gedrückter Taste zurück, gehen dieselben Dateien nicht ein
   zweites Mal hinüber.
2. Mit Dateien bricht der Server den lokalen Drag ab (Esc), wechselt zum Client und sendet
   `DragStart { transfer, entries }`. Danach überträgt er jede Datei auf einem eigenen Stream. Ordner
   werden rekursiv mitgenommen.
3. Der Client drückt dabei **keine** Maustaste. Er schreibt die Dateien in ein Staging-Verzeichnis
   (`*.part`, umbenannt erst nach erfolgreicher SHA-256-Prüfung).
4. Lässt der Nutzer die Taste los, kommt `Drop { transfer, x, y }`. Der Client bestimmt den Ordner unter dem Cursor:
   ein Explorer-Fenster (bei Windows-11-Tabs der sichtbare Tab) bzw. ein Finder-Fenster, den Schreibtisch (unter
   X11 am Fenstertyp `_NET_WM_WINDOW_TYPE_DESKTOP`) oder sonst `Downloads/Qross`.
   Sobald alles angekommen ist, verschiebt er die Dateien dorthin. Vorhandene Namen werden nicht
   überschrieben, sondern durchnummeriert, z. B. „Bericht (2).pdf“.
5. Verlässt der Cursor den Client ohne Drop, verwirft der Client die Übertragung und bricht die Streams ab.

Dateinamen werden beim Empfang bereinigt: keine absoluten Pfade und kein `..`, unter Windows unzulässige
Zeichen und reservierte Namen werden ersetzt.

### Vom Client zum Server

1. Der Server weiß, welche Maustasten auf dem Client gedrückt sind. Erreicht der Cursor dort mit gedrückter
   Taste die Kante zum Server, wechselt er nicht sofort, sondern fragt mit `DragMessage::Query { probe }` nach.
2. Der Client prüft, ob gerade Dateien gezogen werden, genauso wie der Server (Windows: OLE-Drop-Ziel, macOS:
   Drag-Zwischenablage, X11: `XdndSelection`).
3. Zieht er Dateien, bricht er seinen Drag mit Esc ab, antwortet mit `Offer { probe, transfer, entries }` und
   schickt die Dateien auf Streams vom Typ 3. Sonst kommt `Offer` mit leerer Liste. Der Server wechselt dann
   nicht, solange die Taste gedrückt ist; so bleibt ein an den Rand geschobenes Fenster auf dem Client.
4. Mit Dateien wechselt der Cursor zum Server. Lässt der Nutzer die Taste dort los, legt der Server die Dateien
   im Ordner unter dem Cursor ab (Explorer- bzw. Finder-Fenster oder Schreibtisch), sonst in `Downloads/Qross`.
5. Geht der Cursor vorher zurück zu einem Client, wird die Übertragung verworfen.

Ein älterer Client antwortet nie auf die Nachfrage. Mit gedrückter Taste bleibt der Cursor dann auf ihm.

### Vorschau am Cursor

Auf dem Rechner, auf dem der Cursor mit den Dateien gerade ist, hängt ein Kärtchen rechts unter dem Cursor:
Symbol (Datei, Ordner oder mehrere), der Name bzw. „3 Dateien“, „2 Ordner“ oder „3 Elemente“ und die Gesamtgröße.
Es braucht keine neue Nachricht, `DragStart` und `Offer` nennen die Einträge schon.

- **Client** (vom Server gezogen): ab `DragStart`, sofern der Cursor noch auf ihm ist und noch kein `Drop` kam
  (`DragStart` folgt erst, wenn der Server die Dateien aufgelistet hat), bis `Drop` oder `Leave`.
- **Server** (vom Client gezogen): sobald der Cursor mit den Dateien zurück ist (`AwaitLocalDrop`), bis zum
  Loslassen oder bis der Cursor wieder auf einen Client geht.
- Auf beiden Seiten auch dann nicht mehr, wenn die Übertragung abgebrochen wird oder fehlschlägt, denn dann gibt
  es nichts mehr abzulegen, und wenn die Verbindung endet.

Das Kärtchen zeigt dasselbe Fenster wie der [Cursor-Finder](#cursor-finder), immer und unabhängig von dessen
Einstellung; solange es zu sehen ist, tritt der Ring zurück. Am rechten bzw. unteren Rand des Monitors steht es
links bzw. über dem Cursor, die Spitze des Cursors deckt es nie ab: Dort sucht der Empfänger beim Loslassen den
Ordner im Fenster darunter. Für das System des Zielrechners ist es nur ein Bild, kein Drag; Programme bekommen
nichts davon mit.

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
Übertragungen mit diesem Rechner als fehlgeschlagen; was schon im Staging lag, wird gelöscht. Kopierte Dateien
auf Abruf zählen je Einfügevorgang (siehe dort).

## Rechte je Rechner

Der Server legt je Client fest, was geteilt wird, und setzt es selbst durch. Drei Arten, jede mit Richtung
(`Share`: `Both`, `ToClient`, `FromClient`, `Off`; in `qross.toml` `"both"`, `"to-client"`, `"from-client"`,
`"off"`):

| Art | Betrifft |
|---|---|
| `clipboard` | `Clipboard::Data` und den Text, den `Clipboard::Files` mitbringt (`data`) |
| `files` | `Clipboard::Files`, `Clipboard::FilesOnDemand` und Streams vom Typ 4 und 6 |
| `drag` | `Input::DragStart`/`Drop`, Streams vom Typ 3 und 5 |

„Vom Client“ heißt: zum Server und von dort weiter zu anderen Clients, soweit diese empfangen dürfen.

- **Senden:** Der Server liest seine Zwischenablage für einen Client nur, wenn `clipboard` oder `files` zu ihm
  erlaubt ist, und schickt nur den erlaubten Teil: ohne `files` gehen kopierte Dateien als ihr Text, ohne
  `clipboard` ohne Text. Ohne `drag` zum Client wechselt der Cursor mit gedrückter Taste nicht zu ihm und der
  Server prüft gar nicht erst, ob Dateien gezogen werden.
- **Empfangen:** Was ein Client nicht abgeben darf, verwirft der Server beim Eintreffen: `Clipboard::Data`
  ganz, `Clipboard::Files` und `Clipboard::FilesOnDemand` bis auf den erlaubten Text, `Offer` als „keine
  Dateien“. Datei-Streams vom Typ 3 und 4 hält er an (`STOP_SENDING`, Code 0), bevor etwas davon im Staging
  landet. Dateien auf Abruf holt er ohne Recht nicht mehr; Abrufe eines Clients ohne Recht beantwortet er mit
  `Unavailable`. Ohne `drag` vom Client fragt er nicht mit `Query` nach; mit gedrückter Taste bleibt der
  Cursor dann auf dem Client.
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

## Bildschirmschoner

Seit Qross 0.8, wie `screenSaverSync` bei Synergy: Startet auf dem Server der Bildschirmschoner, starten ihn
die Clients auch; endet er dort (Maus, Taste), beenden sie ihn. Einstellbar nur am Server (`[screensaver]`
in `qross.toml`, Einstellungen → Bildschirmschoner), Standard an.

- **Erkennen (Server):** Windows `SPI_GETSCREENSAVERRUNNING`, macOS die verteilten Benachrichtigungen
  `com.apple.screensaver.didstart`/`didstop`, Linux `GetActive` des Bildschirmschoner-Dienstes auf dem
  Session-Bus (`org.gnome.ScreenSaver`, `org.cinnamon.ScreenSaver`, `org.mate.ScreenSaver`,
  `org.freedesktop.ScreenSaver`), in einer X11-Sitzung ohne Dienst die Erweiterung MIT-SCREEN-SAVER. Abgefragt
  wird alle 500 ms in einem eigenen Thread.
- **Nachricht:** `ScreenSaver { active, lock }` auf dem Control-Stream, bei Beginn (`active: true`) und Ende,
  nur an Clients, die `FollowsScreenSaver` gemeldet haben. Wer sich erst während des Bildschirmschoners
  verbindet, bekommt nur das Ende.
- **Cursor:** bleibt, wo er ist; ihn zu bewegen würde den Bildschirmschoner sofort beenden. Steht er auf
  einem Client, fängt der Server die Eingaben ab, und sie erreichen seinen Bildschirmschoner nicht. Die erste
  davon beendet ihn deshalb ausdrücklich (Windows: kleine Mausbewegung, als Eingabe von Qross markiert, damit
  die eigene Erfassung sie durchlässt; macOS: `ScreenSaverEngine` beenden; Linux: `SetActive(false)` bzw.
  X-Server zurücksetzen); die Eingabe selbst geht an den Client. Das Ende meldet der Server wie sonst.
- **Client:** startet seinen eigenen Bildschirmschoner so, wie es das System nach Ablauf der Wartezeit täte,
  und beendet ihn wie eine Mausbewegung. Ist keiner eingerichtet (Windows „(Kein)“, macOS „Nie“, GNOME
  „Bildschirm abschalten: Nie“, KDE ohne automatische Sperre, X11 `xset s off`) oder hält ein Programm den
  Bildschirm wach (Windows `ES_DISPLAY_REQUIRED`, macOS `PreventUserIdleDisplaySleep`), passiert nichts. In
  einer Remotedesktop-Sitzung startet Windows keinen; dort versucht es der Client nicht.
- **Sperren:** Mit `lock` (Einstellung „Clients mitsperren“, `lock_clients = true`) sperrt sich der Client beim
  Beginn, statt nur den Bildschirmschoner zu starten: Windows `LockWorkStation`, macOS die Sperrfunktion des
  Systems (`SACLockScreenImmediate`, wie Ctrl+Cmd+Q), Linux `loginctl lock-session`. Fehlt das, startet er nur
  den Bildschirmschoner.

Verlangt ein Client nach seinem Bildschirmschoner ein Passwort (Windows „Anmeldeseite bei Reaktivierung“,
macOS „Passwort erforderlich“, GNOME automatische Sperre, KDE immer), ist er danach gesperrt, genau wie nach
Ablauf seiner eigenen Wartezeit. Entsperrt wird an seiner eigenen Tastatur: Eingaben von Qross erreichen den
Sperrbildschirm nicht.

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 10. Nur dann sendet der Client
`FollowsScreenSaver`; ein Server aus 0.7 und älter würde bei der unbekannten Nachricht trennen und bekommt sie
nie. `ScreenSaver` schickt der Server nur an Clients, die sich gemeldet haben; Clients bis 0.7 laufen
unverändert weiter.

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

## Cursor-Finder

Seit Qross 0.8, Standard aus. Beim Wechsel blitzt auf dem Rechner, auf dem der Cursor ankommt, etwa eine
Sekunde lang ein Ring um den Cursor auf und darunter sein Name: auf einem Client bei `Enter`, auf dem Server,
wenn der Cursor zurückkommt (`ReleaseCapture`). Verlässt der Cursor einen Rechner (`Leave` bzw. das Abfangen
beginnt), verschwindet die Anzeige dort sofort. Wechselt er von Client zu Client, zeigt der Server nichts.

Eingestellt wird am Server (`[finder] ring`, `name`; Einstellungen → Allgemein), denn dort sitzt, wer gerade
wechselt, und die Einstellung soll für alle Rechner gleich sein. Der Server schickt auf dem Control-Stream
`CursorFinder { ring, name }`: nach `Sharing`, sofern etwas eingeschaltet ist, und nach jeder Änderung, ohne die
Verbindung zu trennen. Ohne diese Meldung zeigt ein Client nichts. Was er zeigt, bestimmt er selbst: den eigenen
Namen im Layout, um den Cursor dort, wo er gerade steht.

Gezeichnet wird auf allen Systemen gleich (`qross_platform::finder`, Systemschrift über `ab_glyph`); jedes System
stellt nur ein durchsichtiges Fenster, das Klicks durchlässt, keinen Fokus nimmt und über allen Fenstern liegt:

| System | Fenster | Skalierung |
|---|---|---|
| Windows | `WS_EX_LAYERED` mit `UpdateLayeredWindow`, `WS_EX_TRANSPARENT`, `NOACTIVATE`, `TOOLWINDOW`, `TOPMOST`; nur angelegt, solange es zu sehen ist | `GetDpiForMonitor` des Monitors unter dem Cursor |
| macOS | randloses `NSWindow`, `ignoresMouseEvents`, Ebene des Bildschirmschoners, in allen Spaces; Bild als `CGImage` im Layer; nur mit AppKit-Ereignisschleife (die App) | Retina über den Anzeigemodus |
| Linux (X11) | Override-Redirect-Fenster mit 32-Bit-Visual, leerer Eingabebereich (Shape); nur mit Compositor | `Xft.dpi` |
| Wayland | – (globale Überlagerungen sind nicht vorgesehen) | |

Läuft auf dem Zielrechner ein Programm im Vollbild oder hat eins den Zeiger eingefangen (wie beim
Spielemodus), zeigt er nichts. Dasselbe Fenster zeigt beim Drag & Drop die [Vorschau am Cursor](#vorschau-am-cursor).

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 9. Clients bis Qross 0.6 lesen den
Control-Stream nach dem `Welcome` nicht mehr; Clients aus 0.7 überspringen `CursorFinder`, denn sie können die
Nachricht nicht lesen. Beide zeigen nichts und bleiben verbunden. Ältere Server schicken nichts.

## Regeln für Robustheit

- **Mausposition absolut**, in Client-Koordinaten (physische Pixel, virtueller Desktop), immer auf einem
  Monitor des Clients. Ein verlorenes
  Datagramm wird vom nächsten überholt, Fehler summieren sich nicht auf. Nur im Spielemodus gehen
  Bewegungen relativ; ein verlorenes Delta fehlt dann wie bei einer echten Maus ein Ruckeln.
- Der Client verwirft Datagramme eines früheren Besuchs (Nachzügler) und mit nicht neuerer `seq`
  (Umsortierung im Netz). Von einem späteren Besuch, dessen `Enter` noch unterwegs ist, hebt er das neueste
  auf (relative Bewegungen zusammengezählt) und speist es gleich nach dem `Enter` ein. Die Vergleiche von
  `epoch` und `seq` sind wrap-around-sicher.
- Warten im Einspeise-Thread des Clients schon mehrere Bewegungen, etwa weil nach einer Lücke im Netz viele auf
  einmal ankamen, speist er von Positionen nur die letzte und relative Bewegungen zusammengezählt ein. Der Cursor
  springt an die aktuelle Stelle, statt die alte Bahn nachzufahren.
- `Button` enthält die Position. So landet ein Klick richtig, auch wenn das letzte Bewegungs-Datagramm fehlt.
- Wechsel nur außerhalb der **Ecken-Totzone** (Standard 8 px) und optional erst nach einer
  **Wechselverzögerung**, also wenn eine bestimmte Zeit gegen die Kante gedrückt wird. Das verhindert
  versehentliche Wechsel beim Zielen auf Startmenü oder Schließen-Knopf.
- Tasten sind **physische Tasten als USB-HID-Usage**. Das Layout des Clients bestimmt das Zeichen.
  Das ist plattformneutral und funktioniert auch mit unterschiedlichen Tastaturlayouts. Wer bei
  unterschiedlichen Layouts lieber die Zeichen des Servers will, stellt den Client auf „Zeichen“ (siehe oben).

## Verbindungsqualität

Die Oberfläche zeigt je Verbindung Latenz, Jitter, Paketverlust und Durchsatz samt einer Stufe an (Server:
in den Details des ausgewählten Rechners, Reiter „Verbindung“; Client: unter dem Status). Das braucht **keinen Protokollanteil und keinen
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

Ab Welcome-Version 11 misst Qross nebenbei, wie lange eine Eingabe vom Server bis zum Einspeisen am Client
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
- Bei zusammengefassten Mausbewegungen (mehrere warten schon) zählt beim Server die Zeit der ersten; beim Client
  gilt eine Bewegung als eingespeist, wenn die letzte der zusammengefassten eingespeist ist. Verlorene, vom Client
  verworfene und vor ihrem `Enter` angekommene Datagramme fehlen in Netz- und Client-Anteil, im Server-Anteil
  zählen sie mit.

Messen auf einem Rechner ohne echte Erfassung und Einspeisung:
`cargo run --release -p qross-net --example latency_probe` (Client im selben Prozess, zusätzlich von Ende zu
Ende mit 99. Perzentil und Maximum; mit `files` gehen dabei kopierte Dateien über eine Leitung wie WLAN, mit
`hang` hängt die Leitung zweimal 2 s) bzw. mit `server` und `qross client … --dry-run` in einem zweiten Fenster.

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 11. Nur dann schickt der Client `Latency`;
ältere Server bekommen die Nachricht nie (sie trennten bei unbekannten Control-Nachrichten). Das Ergebnis schickt
der Server nur Clients, die sich gemeldet haben; Clients aus 0.7 lesen den Control-Stream zwar, bekommen es aber
nie. Für ältere Clients misst der Server nur seinen eigenen Anteil, die Oberfläche zeigt dann nur ihn. Ein neuer
Client an einem älteren Server misst nichts, dort entfällt die Anzeige.

## CPU und Arbeitsspeicher

Seit Qross 0.14 (Welcome-Version 13), aufgeteilt nach Art der Prozesse ab Welcome-Version 14 (ab Qross 0.15).
Die Oberfläche zeigt, wie viel Rechenzeit und Arbeitsspeicher Qross auf einem Rechner braucht: Qross selbst und seine
Web-Oberfläche samt Hilfsprozessen, am Server in den Details des ausgewählten Rechners (für einen Client im Reiter
„Verbindung“, für den Server selbst als „Qross hier“), am Client in der Karte „Verbindung“. Programme, die Qross nur
startet, zählen nicht mit, etwa der Browser für einen Link oder der Explorer für einen Ordner (bis Qross 0.14.0
zählten unter Windows und Linux alle Nachfahren mit, unter macOS auch selbst gestartete Kommandozeilenprogramme).
Implementierung: Messung in `crates/qross-platform/src/usage/`, Meldungen in `crates/qross-net/src/usage.rs`.

**Gemessen** wird nur auf Abruf, nie im Hintergrund:

| System | Prozesse | CPU | Arbeitsspeicher |
|---|---|---|---|
| Windows | dieser und die Nachfahren, deren Programm `msedgewebview2.exe` heißt (Toolhelp-Schnappschuss); weiter geht es nur über solche | `GetProcessTimes` | privater Arbeitssatz (`PROCESS_MEMORY_COUNTERS_EX2`), wie im Task-Manager |
| macOS | dieser und die Dienste von WebKit (`com.apple.WebKit.*`, Name aus `proc_pidpath`), für die er verantwortlich ist; die startet launchd | `proc_pid_rusage`, Mach-Zeit in Nanosekunden umgerechnet | physischer Speicherbedarf (`ri_phys_footprint`), wie in der Aktivitätsanzeige |
| Linux | dieser und darunter die Prozesse von WebKitGTK (Name beginnt mit `WebKit`), auch hinter bubblewrap (`bwrap`, das selbst nicht zählt); weiter geht es nur über solche (`/proc`) | `utime` + `stime` aus `/proc/<pid>/stat` | PSS aus `smaps_rollup` (geteilte Seiten anteilig), sonst der residente Teil |

Ein Kind zählt nur, wenn es nach seinem Elternprozess gestartet ist; sonst trägt es nur eine neu vergebene
Prozessnummer. Unter macOS zählen die Dienste nur, wenn die App für sich selbst verantwortlich ist (aus Finder,
Dock oder als Anmeldeobjekt gestartet), denn aus dem Terminal heraus wäre es das Terminal. Die CPU ist die
Rechenzeit aller dieser Prozesse seit der vorigen Messung in Prozent eines Kerns (100 % = ein Kern voll
ausgelastet, mit mehreren Kernen auch mehr). Liegt die vorige Messung länger als 5 s zurück, misst Qross zweimal im
Abstand von 0,5 s, damit der Wert für jetzt gilt. Eine Messung sieht alle Prozesse des Systems durch (einige
Millisekunden) und läuft deshalb in einem eigenen Thread, nie in einer Ereignisschleife.

**Arten** der Prozesse (`UsageKind`, auf der Leitung ein Byte). Unter Windows steht die Art in der Kommandozeile von
WebView2; Qross liest sie mit `NtQueryInformationProcess(ProcessCommandLineInformation)` (ab Windows 8.1, ohne den
Speicher des Prozesses zu lesen). Lässt sie sich nicht lesen, gilt das Kind von Qross als Browser-Prozess und alles
darunter als `Other`. Unter macOS und Linux entscheidet der Name des Programms (unter Linux der aus
`/proc/<pid>/stat`, nach 15 Zeichen abgeschnitten).

| Art | Windows (WebView2) | macOS | Linux |
|---|---|---|---|
| `Core` (0) | Qross selbst | Qross selbst | Qross selbst |
| `Page` (1) | `--type=renderer` | `com.apple.WebKit.WebContent` | `WebKitWebProcess` |
| `Graphics` (2) | `--type=gpu-process` | `com.apple.WebKit.GPU` | `WebKitGPUProcess` |
| `Browser` (3) | ohne `--type`: der Browser-Prozess, der die anderen startet | – | – |
| `Network` (4) | `--type=utility --utility-sub-type=network.mojom.NetworkService` | `com.apple.WebKit.Networking` | `WebKitNetworkProcess` |
| `Other` (5) | weitere Dienste (etwa `storage.mojom.StorageService`), `--type=crashpad-handler` | weitere `com.apple.WebKit.*` | weitere `WebKit*` |

Die Oberfläche zeigt unter CPU und RAM die Aufteilung „Kern … · Oberfläche …“ (Oberfläche: alle Arten außer
`Core`) und im Tooltip dazu jede Art einzeln mit CPU und RAM.

**Ablauf** für die Werte eines Clients:

1. Der Client schickt `ReportsUsage` direkt nach dem `Welcome`, nur ab Version 13 und wo er messen kann.
2. Zeigt die Oberfläche des Servers seine Werte, schickt der Server ihm `WatchUsage(true)`, sobald sie nicht mehr zu
   sehen sind (andere Auswahl, anderer Reiter, Fenster versteckt), `WatchUsage(false)`. Gezeigt wird höchstens ein
   Client zugleich; verbindet er sich neu, bekommt er `WatchUsage(true)` gleich nach seinem `ReportsUsage`.
3. Solange er beobachtet wird, schickt der Client etwa alle 2 s seine Werte, die erste rund 0,5 s nach Beginn: an
   Server ab Version 14 `UsageDetail { total, parts }` mit der Summe (`UsageReport { cpu_permille, memory_kib,
   processes }`) und je vorkommender Art einem `UsagePart { kind, cpu_permille, memory_kib, processes }` in der
   Reihenfolge der Tabelle (mit Längenfeld etwa 45 Bytes), an ältere nur die Summe als `Usage(UsageReport)` (etwa
   10 Bytes). Was nach dem Ende der Anzeige noch ankommt, verwirft der Server.

Die eigenen Werte fragt die Oberfläche selbst ab, ebenfalls alle 2 s und nur, solange sie zu sehen sind.

Ansehen ohne zweiten Rechner: `cargo run -p qross-net --example scripted_server -- usage <Client-Name>` gibt die
Meldungen eines Clients samt Teilen aus, etwa der App als Client im Testmodus mit der Adresse `127.0.0.1:24990`.

**Kompatibilität:** Ein Server ab Qross 0.14 meldet im `Welcome` die Version 13, ein neuerer 14. Nur ab 13 schickt
der Client `ReportsUsage` und `Usage`, nur ab 14 `UsageDetail` statt `Usage`; ältere Server bekommen nie, was sie
nicht lesen können (sie trennten bei unbekannten Control-Nachrichten). Der Server nimmt beides; von Clients bis Qross
0.14.0 zeigt die Oberfläche nur die Summe. Eine Art, die er nicht kennt (aus einer späteren Version), zählt er als
`Other`, mehrere Teile derselben Art zählt er zusammen. `WatchUsage` bekommen nur Clients, die sich gemeldet haben;
für ältere zeigt die Oberfläche „meldet das nicht“.

## Noch offen

- Drag & Drop unter Wayland (es fehlt ein Portal dafür); Ablegen im Ordner eines Dateimanagers unter X11
- Drag & Drop: auf dem Zielrechner ein echter Drag des Systems statt der Vorschau (Windows: OLE mit virtuellen
  Dateien, macOS: File Promises, X11: XDND), damit auch Programme die Dateien annehmen, etwa als Anhang
- Spielemodus: eingefangenen Zeiger unter macOS und Linux erkennen; Rohdaten der Maus am Server
  (Raw Input) statt Cursor-Deltas, damit Zeigerbeschleunigung des Servers nicht mitwandert
- Zwischenablage: RTF; virtuelle Dateien anderer Programme (z. B. Outlook-Anhänge) weitergeben
- Kopierte Dateien auf Abruf auch unter macOS (File-Provider-Erweiterung, siehe oben) und Linux sowie für
  Server mit mehreren Clients (weiterreichen statt vorab holen); unter Windows wahlweise echte Dateien für
  Programme, die keine virtuellen annehmen
- Strg+Alt+Entf, Secure Desktop (UAC)
- Verzögerung der Eingabekette: Zeitstempel schon im Hook der Erfassung setzen, damit der Thread-Wechsel bis zur
  Ereignisschleife mitzählt

## Feststelltaste, Num und Rollen

Seit Qross 0.8 stehen diese Tasten auf allen Rechnern gleich, wie bei Synergy. Maßgeblich ist der Server:
An ihm hängt die Tastatur, und ihre Leuchten zeigen seinen Stand.

- **Betreten:** Gleich nach `Enter` liest der Server seinen Stand und schickt
  `Locks(LockState { caps, num, scroll })` (je `Option<bool>`). Der Client stellt seine Tasten genauso ein und
  schaltet dabei nur um, was abweicht. `None` heißt: Diese Taste kann der Server nicht lesen (am Mac Num und
  Rollen); der Client lässt seine, wie sie ist.
- **Drücken auf dem Client:** Drückt man eine der Tasten, während der Cursor auf einem Client steht, schaltet
  der Server mit und schickt den neuen Stand als `Locks`. Den Tastendruck selbst bekommt der Client nicht: Er
  schaltete ihn ein zweites Mal um. Ist die Taste für diesen Client umbelegt oder kennt der Server ihren Stand
  nicht, geht sie wie jede andere als `Key`.
- **Zurück auf dem Server:** nichts zu tun, er hat den Stand schon. Beim Verlassen bleibt der Client, wie er
  ist. Schaltet man am Client mit dessen eigener Tastatur, gilt das dort bis zum nächsten Betreten.
- **Eine Einstellung gibt es nicht.** Wer die Fang-Taste auf Rollen legt (Standard), schaltet damit Rollen
  nur, solange der Cursor auf dem Server ist; auf einem Client fängt Qross die Taste ab.

| System | Lesen | Setzen | Wenn man auf dem Client drückt, schaltet der Server … |
|---|---|---|---|
| Windows | `GetKeyState(VK_CAPITAL / VK_NUMLOCK / VK_SCROLL) & 1` | Taste per `SendInput` drücken, nur wenn der Stand abweicht | mit, weil die Erfassung diese Tasten durchlässt |
| macOS | `IOHIDGetModifierLockState(kIOHIDCapsLockState)`; Num und Rollen gibt es nicht | `IOHIDSetModifierLockState`; ein künstliches CGEvent schaltet nicht um. Eingespeiste Tasten tragen danach das Flag AlphaShift | selbst um, bevor der Event-Tap die Taste sieht; der Tap liest den Stand aus deren Flags |
| X11 | eingerastete Modifier aus `XkbGetState`: Lock, Num auf dem Modifier mit dem Keysym `Num_Lock`; Rollen ist kein Modifier | `XkbLatchLockState` wie `numlockx`, für alle Tastaturen, Leuchten folgen | selbst um (XKB wertet die Taste vor dem Grab aus); die Erfassung liest danach den Stand |
| Wayland | als Client aus `ei_keyboard.modifiers`, sofern der Compositor es meldet; als Server nicht | nur durch Drücken der Taste über libei | – (Server: Taste geht wie bisher an den Client) |

**Kompatibilität:** Ein neuer Server meldet im `Welcome` die Version 8. Nur dann sendet der Client
`SyncsLocks`; Server bis Qross 0.7 bekommen die Nachricht nie und schicken die Tasten wie bisher. `Locks`
kann ein Client bis 0.7 nicht lesen (sein Input-Stream bräche ab). Er bekommt es nie, sondern weiter die
Tasten selbst.
