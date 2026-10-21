<p align="center">
  <img src="docs/assets/logo.png" width="128" alt="Qross-Logo">
</p>

<h1 align="center">Qross</h1>

<p align="center">Maus, Tastatur, Clipboard und Dateien über mehrere Rechner teilen – schnell, sicher, über QUIC.</p>

<p align="center">
  <a href="https://github.com/xozy22/qross/actions/workflows/ci.yml"><img src="https://github.com/xozy22/qross/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/xozy22/qross/releases/latest"><img src="https://img.shields.io/github/v/release/xozy22/qross" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/Lizenz-MIT-blue" alt="MIT-Lizenz"></a>
</p>

Eine eigene Implementierung in der Art von Synergy, geschrieben in Rust. Statt TCP verwendet Qross ein eigenes
Protokoll über **QUIC**. Bei Paketverlust (WLAN) bleibt der Cursor damit flüssig, wo ein TCP-Transport wie der von
Synergy 200–600 ms hängt ([Benchmark](docs/benchmark-ergebnisse.md)). Große Dateien wandern per **Drag & Drop** über
die Bildschirmkante, ohne dass Maus und Tastatur ins Stocken geraten. Kopierte Dateien lassen sich auf dem
anderen Rechner einfach **einfügen** (Strg+C hier, Strg+V dort); unter Windows kommen sie erst beim Einfügen,
auch mehrere Gigabyte.

<p align="center">
  <img src="docs/assets/screenshots/server.png" width="820" alt="Server-Ansicht: Anordnung der Bildschirme, Rechnerliste mit Freigabe-Anfrage und die Details des ausgewählten Rechners, hier seine Rechte">
</p>

<table>
  <tr>
    <td align="center"><img src="docs/assets/screenshots/client.png" alt="Client: Status, Verbindungsqualität und der verbundene Server"><br><sub>Client: Status, Verbindungsqualität und der Server; andere Server im Netzwerk mit einem Klick</sub></td>
    <td align="center"><img src="docs/assets/screenshots/freigabe.png" alt="Freigabe-Dialog mit Fingerprint"><br><sub>Neuer Rechner: Freigabe erst nach Fingerprint-Vergleich</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="docs/assets/screenshots/einstellungen.png" alt="Einstellungen, hier die Tastatur mit Fang-Taste und Kürzeln zum Wechseln"><br><sub>Einstellungen: oben das aktive Profil (Server, Client), darunter alles für alle Profile, hier die Tastatur</sub></td>
    <td align="center"><img src="docs/assets/screenshots/neuigkeiten.png" alt="Neuigkeiten mit allen Versionen und ihren Änderungen"><br><sub>Neuigkeiten: Klick auf die Version in der Kopfzeile zeigt, was sich geändert hat</sub></td>
  </tr>
  <tr>
    <td align="center" colspan="2"><img src="docs/assets/screenshots/verbindungsqualitaet.png" width="620" alt="Details eines Rechners, Reiter Verbindung: Latenz, Jitter, Verlust, Verlauf und Verzögerung der Eingabe"><br><sub>Klick auf einen Rechner zeigt seine Verbindung: Latenz, Jitter, Verlust, Durchsatz, Verlauf der letzten Minute und die Verzögerung der Eingabe, aufgeteilt in Server, Netz und Client</sub></td>
  </tr>
</table>

<sub>Die Screenshots zeigen Demo-Daten.</sub>

## Warum Qross statt Synergy?

| | Synergy 3 | Qross |
|---|---|---|
| Transport | TCP + TLS, ein Kanal für alles | QUIC: Maus als Datagramm, Tasten, Clipboard und Dateien auf getrennten Streams |
| Cursor bei 1 % / 5 % Paketverlust (p99)¹ | 36 ms / 588 ms | 4 ms / 4,5 ms |
| Eingaben während eines 2-MB-Clipboards (p99)¹ | 116 ms (0 % Verlust) / 248 ms (1 %) | 32 ms / 5 ms, keine Ruckler über 50 ms |
| Screenshots & formatierter Text in der Zwischenablage | ja | ja, als PNG bzw. HTML, ohne die Eingaben auszubremsen |
| Dateien per Drag & Drop | über dieselbe Verbindung wie die Eingaben | eigener Stream je Datei, SHA-256-geprüft, auch GB-große Dateien und Ordner |
| Dateien kopieren und einfügen | nicht über die Zwischenablage, nur per Drag & Drop | Strg+C im Explorer, Strg+V auf dem anderen Rechner, in beide Richtungen; nach Windows erst beim Einfügen und ohne Größengrenze, sonst bis 1 GB automatisch |
| Vertrauen | TLS mit Fingerprint-Bestätigung | gegenseitige Zertifikate: der Server gibt Clients explizit frei, der Client merkt sich den Server |
| Versehentliches Wechseln an Ecken | Ecken-Schutz muss konfiguriert werden | Ecken-Totzone ab Werk, optionale Wechselverzögerung |
| PC ↔ Mac | Modifier von Hand umbelegen | Strg/Cmd wird automatisch getauscht |
| Verbindungsabbruch mitten in einer Eingabe | Tasten können hängen bleiben | Client lässt alles los; Notausgang Strg+Alt+Shift+Esc |

¹ Gemessen mit [`netbench`](bench/netbench/README.md): Linux-netem, 100 Mbit/s, 1 ms Verzögerung. Die Spalte
„Synergy 3“ ist eine Nachbildung von Synergys Transport (TCP+TLS, ein gemeinsamer Kanal), nicht das Original-
programm. Details im [Benchmark](docs/benchmark-ergebnisse.md).

## Funktionen und Stand

| | Windows | Linux (X11) | Linux (Wayland)³ | macOS |
|---|---|---|---|---|
| Server: Eingaben erfassen, Kanten pro Monitor | ✔ | ✔ | ✔ GNOME ≥ 45, KDE ≥ 6.1, Hyprland ≥ 0.56 | ✔¹ |
| Client: Eingaben einspeisen | ✔ | ✔ | ✔ GNOME ≥ 45, KDE ≥ 6.1, sonst über XWayland | ✔¹ |
| Zwischenablage in beide Richtungen: Text, HTML, Bilder (z. B. Screenshots) | ✔ | ✔ | ✔ KDE, wlroots direkt; GNOME über XWayland | ✔¹ |
| Dateien kopieren und einfügen, in beide Richtungen | ✔ Explorer, übertragen erst beim Einfügen¹⁰ | ✔ Dateimanager mit `text/uri-list` (Nautilus, Dolphin) | wie Zwischenablage | ✔ Finder¹ |
| Passwörter nicht teilen: was Passwortmanager als vertraulich markieren, bleibt auf dem Rechner | ✔ `ExcludeClipboardContentFromMonitorProcessing` (KeePass, KeePassXC, 1Password) | ✔ `x-kde-passwordManagerHint` (KeePassXC) | wie Zwischenablage | ✔ `ConcealedType`, `TransientType`¹ |
| Cursor auf dem Server ausblenden | ✔ | ✔ | Sache des Compositors | soweit macOS es zulässt¹ |
| Drag & Drop **vom Server** ziehen⁸ | ✔ | ✔ Dateimanager (XDND) | – | ✔ Finder¹ |
| Drag & Drop **vom Client** zum Server ziehen⁸ | ✔ Explorer | ✔ Dateimanager (XDND) | – | ✔ Finder¹ |
| Gezogene Dateien **ablegen**⁸ | ✔ Explorer-Fenster oder Desktop unter dem Cursor | ✔ Schreibtisch unter dem Cursor, sonst `~/Downloads/Qross` | Client: in `~/Downloads/Qross` | ✔ Finder-Fenster oder Schreibtisch unter dem Cursor¹ |
| Vorschau beim Ziehen: Symbol, Name und Größe der Dateien am Cursor des anderen Rechners⁹ | ✔ | ✔ mit Compositor | – | ✔ in der App¹ |
| Fang-Taste frei wählbar, Medientasten, Tasten je Rechner umbelegen | ✔ | ✔² | Fang-Taste nur, solange der Cursor auf einem Client ist | ✔¹ ² |
| Zeichen statt Tastenposition je Rechner: anderes Tastaturlayout, tote Tasten, AltGr | ✔ | ✔ | – (bleibt Tastenposition) | ✔¹ |
| Feststelltaste, Num und Rollen auf allen Rechnern gleich (Stand des Servers) | ✔ alle drei | ✔ Feststelltaste, Num | Client: per Tastendruck, so gut der Compositor seinen Stand meldet; Server: – | ✔ Feststelltaste¹ |
| Mehrere Monitore je Rechner: Skizze im Layout, Cursor nur auf Monitorflächen | ✔ | ✔ | ✔ in logischen Pixeln | ✔¹ |
| Verbindungsqualität: Latenz, Jitter, Verlust, Durchsatz, Verzögerung der Eingabe (Server, Netz, Client) | ✔ | ✔ | ✔ | ✔¹ |
| Spielemodus: bei Vollbild festhalten, relative Mausbewegung für Spiele | ✔ Vollbild, eingefangener Zeiger | ✔ Vollbild | – | ✔ Vollbild¹ |
| Dateiübertragungen: Fortschritt, Rate und Restzeit, Abbrechen auf beiden Seiten | ✔ | ✔ | ✔ | ✔¹ |
| Rechte je Rechner: Zwischenablage, kopierte Dateien, Drag & Drop je Richtung erlauben oder sperren | ✔ | ✔ | ✔ | ✔¹ |
| Server: [„Sichere Tastatureingabe“](#macos-sichere-tastatureingabe) erkennen, Hinweis, Cursor bleibt auf dem Mac | entfällt | entfällt | entfällt | ✔¹ |
| Server: Fenster mit Administratorrechten (z. B. Task-Manager) erkennen, die Maus und Tastatur für Qross sperren: Hinweis, Cursor bleibt hier | ✔ | entfällt | entfällt | entfällt |
| Cursor-Finder: beim Wechsel Ring um den Cursor und Name des Rechners⁶ | ✔ | ✔ mit Compositor | – (keine Fenster über anderen Programmen möglich) | ✔ in der App¹ |
| Bildschirmschoner abgleichen: startet und endet mit dem des Servers, auf Wunsch Clients mitsperren⁷ | ✔ | ✔ über den Dienst des Desktops, sonst den X-Server | ✔ über den Dienst des Desktops | ✔¹ |
| Grafische Oberfläche mit Layout-Editor, auf Deutsch und Englisch (folgt der Systemsprache, umschaltbar) | ✔ | ✔² | ✔², mit Hinweisen zu Portal und Freigabe | ✔² |
| Profile je Ort, automatisch nach Netzwerk (Gateway, ohne Zusatzrechte)⁴ | ✔ mit Netzwerkname | ✔, Name über NetworkManager | ✔ wie X11 | ✔ ohne WLAN-Namen¹ |
| [Autostart](docs/autostart.md) bei der Anmeldung, unsichtbar im Tray⁵ | ✔ `HKCU\…\Run` | ✔ XDG-Autostart | ✔ XDG-Autostart | ✔ LaunchAgent¹ |

¹ macOS wird in der CI auf Apple Silicon gebaut und getestet, auch mit der echten Zwischenablage. Erfassung und
Einspeisung sind auf einem echten Mac noch nicht geprüft. Qross braucht dort die Rechte „Bedienungshilfen“ und
„Eingabeüberwachung“.
² „Medientasten immer auf dem Server“ wirkt mit einem Windows-Server. Unter macOS bleiben Medientasten des
Servers ohnehin dort, unter X11 gehen sie immer an den aktiven Rechner.
² Die Oberfläche basiert auf Tauri; gebaut und geprüft wurde sie unter Windows.
³ Wayland läuft über die Desktop-Portale RemoteDesktop und InputCapture und libei. Das System fragt
einmal nach der Freigabe; wo der Desktop es kann, merkt sich Qross sie (GNOME fragt beim Erfassen bis
Version 50 bei jedem Start). Geprüft sind die Logik, das Einspeisen über libei gegen eine eigene
Gegenstelle und gegen mutter ohne Bildschirm. Auf einem echten GNOME- oder KDE-Desktop ist Wayland
noch nicht getestet, ebenso wenig der Freigabe-Dialog. Sway und andere wlroots-Desktops: Server nur in
einer X11-Sitzung. Details, Grenzen und Testanleitung: [Wayland](docs/wayland.md).
⁴ Nur in der Oberfläche; `qross.toml` bleibt eine Konfiguration (für mehrere Orte mehrere Dateien und
`--config`). Erkannt wird das Netzwerk an Adresse und MAC-Adresse des Standard-Gateways und am Subnetz. Den
WLAN-Namen gibt macOS ab Version 14 nur mit Ortungsdiensten heraus; Qross fragt nicht danach. Unter Windows
echt geprüft; Linux und macOS lesen Routen und ARP-Cache aus `/proc/net` bzw. `netstat`, dort nur mit Testdaten
geprüft.
⁵ Für den angemeldeten Nutzer, ohne Administratorrechte; der Eintrag folgt dem Programm, wenn es verschoben
wird. Die CI schreibt, prüft und entfernt auf allen drei Systemen einen echten Eintrag, unter Windows auch
lokal geprüft; den Start bei der Anmeldung selbst prüft kein Test. Ohne Oberfläche: `qross` als
systemd-Dienst, LaunchAgent oder geplante Aufgabe, siehe [Autostart](docs/autostart.md).
⁶ Standard aus. Am Server eingestellt, gilt für alle Rechner (Clients ab Qross 0.8); nicht über Programmen im
Vollbild. Unter Wayland lassen sich keine Fenster über andere Programme legen, dort zeigt der Rechner selbst
nichts. Unter macOS braucht das Fenster die Ereignisschleife der App; `qross` auf der Kommandozeile zeigt dort
nichts. Unter Windows echt geprüft, macOS und X11 nur gebaut und mit Clippy geprüft.
⁷ Startet ein Client seinen Bildschirmschoner über Qross, gilt dort dasselbe wie nach Ablauf seiner eigenen
Wartezeit: **Verlangt er danach ein Passwort, ist er gesperrt** – Windows mit „Anmeldeseite bei
Reaktivierung“, macOS mit „Passwort erforderlich“ (bei „Sofort“ gleich, sonst nach der eingestellten Zeit),
GNOME mit automatischer Bildschirmsperre, KDE immer (dort ist der Bildschirmschoner die Sperre). Entsperren
musst du ihn dann **an seiner eigenen Tastatur**; Qross kann nicht in den Sperrbildschirm tippen. Das gilt
erst recht mit „Clients mitsperren“. Ohne eingerichteten Bildschirmschoner (Windows „(Kein)“, macOS „Nie“,
GNOME „Bildschirm abschalten: Nie“, KDE ohne automatische Sperre) oder während dort ein Video den Bildschirm
wach hält, passiert auf dem Client nichts, auch keine Sperre. In einer Windows-Remotedesktop-Sitzung startet
Windows keinen Bildschirmschoner, Qross versucht es dort nicht. Geprüft: Logik, Protokoll und ältere
Gegenstellen im Test; unter Windows echt das Erkennen und „keiner eingerichtet“, Starten und Beenden nur bis zur
Systemschnittstelle (der Testrechner lief per Remotedesktop). Ob ein Client sich sperrt, folgt aus dem
Verhalten der Systeme, auf echter Hardware ist es noch nicht geprüft; macOS ist nur übersetzt, Linux nur gegen
Xvfb getestet.
⁸ Abgelegt wird im Ordner unter dem Cursor: der Ordner, den ein Explorer- bzw. Finder-Fenster zeigt, oder der
Schreibtisch; sonst in `Downloads/Qross`. Direkt in ein Programm, etwa als Anhang in eine E-Mail, legt Qross auf
keinem System ab. Den Ordner eines Finder-Fensters liest Qross über die Bedienungshilfen aus Titel, Pfadleiste
und den sichtbaren Einträgen; in einer Suche, in „Zuletzt benutzt“ oder AirDrop gibt es keinen, dann gilt
`Downloads/Qross`. Unter X11 erkennt Qross nur den Schreibtisch (Fenstertyp `_NET_WM_WINDOW_TYPE_DESKTOP`, etwa
von xfdesktop, Plasma, Nemo oder Caja): Dateimanager verraten nicht verlässlich, welchen Ordner ein Fenster
zeigt. Unter Wayland sieht kein Programm fremde Drag-&-Drop-Vorgänge oder das Fenster unter dem Cursor, und kein
Desktop-Portal bietet das an; InputCapture meldet an der Kante nicht einmal, ob eine Taste gedrückt ist.
⁹ Nur eine Vorschau: Für das System des Zielrechners läuft dort kein Drag, abgelegt wird wie beschrieben im
Ordner unter dem Cursor. Sie steht rechts unter dem Cursor (am Rand links bzw. darüber) und folgt ihm, bis
abgelegt oder abgebrochen wird, der Cursor den Rechner verlässt oder die Verbindung endet; die Spitze des
Cursors deckt sie nie ab. Immer an, nicht über Programmen im Vollbild. Wie beim Cursor-Finder lassen sich unter
Wayland keine Fenster über andere Programme legen, und unter macOS zeigt nur die App etwas, nicht `qross` auf
der Kommandozeile. Geprüft sind Bild und Ablauf im Test, beide Richtungen über echtes QUIC; auf echten Rechnern
ist sie noch nicht geprüft.
¹⁰ Fügt ein Windows-Rechner ein (ab Qross 0.13 auf beiden Seiten), liegt die Liste sofort in seiner
Zwischenablage, als „virtuelle Dateien“ wie bei Outlook-Anhängen oder Remotedesktop. Übertragen wird erst, wenn
der Explorer einfügt, mit seinem Fortschrittsdialog und dem von Qross; ohne Einfügen fließt nichts, und die Grenze
für kopierte Dateien gilt nicht. Abbrechen geht im Explorer und in Qross auf beiden Seiten. Programme, die nur
echte Dateien annehmen (manche Chat- und Upload-Felder), bekommen so nichts; mit der Umgebungsvariablen
`QROSS_PASTE_ON_DEMAND=0` kommen kopierte Dateien wieder vorab. Ein Server mit mehreren Clients holt sie weiterhin
vorab, weil er sie an den nächsten weitergibt. macOS und Linux übertragen weiter vorab: Der Finder nimmt beim
Einfügen keine Dateiversprechen an. Geprüft im Test über echtes QUIC (beide Richtungen, Abbrechen auf jeder Seite
und im Programm, zweimal gleichzeitig, ältere Gegenstellen) und unter Windows mit der echten Zwischenablage und
dem Einfügen des Explorers, auch mit 1,2 GB; mit einem echten Mac noch nicht.

**Wie geprüft wurde:**
- Plattformunabhängige Logik, Protokoll und Netzwerk: Unit-Tests und Ende-zu-Ende-Tests über echtes QUIC
  (Wechsel, Eingaben, Zwischenablage mit Text, HTML und Bildern, Drag & Drop mit 8 MB und Ordnern, kopierte
  Dateien und Ordner in beide Richtungen, Drag & Drop zum Server, Fortschritt und Abbrechen von Sender und
  Empfänger, Rückkehr mit gedrückter Taste ohne Ablegen, mehrere Monitore je Client, Messwerte der
  Verbindungsqualität, Verzögerung der Eingabekette samt Uhrenversatz und Gegenstellen aus 0.7, Spielemodus samt
  Gegenstellen aus 0.5, Wechsel per Tastenkürzel, Zeichen statt Tastenposition samt Gegenstellen aus 0.6, Abgleich
  von Feststelltaste, Num und Rollen samt Gegenstellen aus 0.7, Rechte je Rechner für jede Art und Richtung samt
  sofortiger Änderung und einem älteren Client, der es trotzdem versucht, zurückgehaltene vertrauliche Inhalte,
  Cursor-Finder samt sofortiger Änderung und Clients aus 0.7, Drag-Vorschau in beide Richtungen samt Ausblenden
  beim Ablegen, Verlassen, Abbrechen und Verbindungsende, Bildschirmschoner samt Ende durch Eingaben für einen
  Client, Sperren und Gegenstellen aus 0.7, Ablehnung unbekannter Clients, LAN-Suche, Zurückholen und Halten des
  Cursors bei sicherer Tastatureingabe).
- macOS, sichere Tastatureingabe: Die CI schaltet sie im Test selbst ein, erkennt sie samt Prozess und schaltet
  sie wieder aus. Mit einem echten Passwortfeld oder Terminal ist es noch nicht geprüft.
- Windows-Tastaturlayouts: echter Test mit dem deutschen und dem US-Layout (AltGr, tote Tasten, Kürzel), soweit
  sie auf dem Rechner eingerichtet sind.
- Windows-Zwischenablage: echter Test mit Text, HTML, PNG und Screenshot-Bitmap (DIB) sowie kopierten Dateien,
  die der Explorer anschließend selbst einfügt, und mit den Formaten, mit denen Passwortmanager markieren.
  Kopierte Dateien auf Abruf: virtuelle Dateien (Ordner, große und leere Datei), die der Explorer in einem zweiten
  Prozess über sein Drop-Ziel einfügt, so wie Strg+V; dabei auch ein Abbruch mitten in einer Datei und 1,2 GB
  über QUIC von einem Client im selben Prozess. Die Shell-Automatisierung (`Shell.Application`, Verb „Einfügen“)
  holt virtuelle Dateien nicht ab; dafür gibt es den Weg über das Drop-Ziel.
- Feststelltaste und Num unter Windows: echt umgeschaltet und zurückgestellt (`SendInput`, `GetKeyState`),
  solange der Rechner nicht gesperrt ist. Unter X11 setzt und liest der X11-Test sie. macOS
  (`IOHIDSetModifierLockState`) ist nur übersetzt, nicht auf einem echten Mac geprüft.
- Linux: echte Tests gegen einen X-Server (Xvfb in Docker und in der CI), darunter Drag & Drop mit einer
  nachgebauten ziehenden Anwendung (XDND, Zeiger- und Tastatur-Grab, Abbruch mit Esc) und der Schreibtisch unter
  dem Cursor. Mit echten Dateimanagern ist das Ziehen noch nicht geprüft.
- macOS: Drag-Erkennung gegen die echte Drag-Zwischenablage in der CI. Den Ordner eines Finder-Fensters prüft die
  CI nur, wenn sie das Recht „Bedienungshilfen“ hat; echtes Ziehen im Finder ist noch nicht geprüft.
- Wayland: Einspeisen über libei gegen eine EIS-Gegenstelle im Test und gegen mutter ohne Bildschirm
  (eigener CI-Job, noch experimentell). Portal, Freigabe-Dialog und Erfassen sind nur übersetzt, nicht
  auf einem echten Desktop geprüft.
- Windows: Bau- und Smoke-Tests sowie die Explorer-Ordnererkennung. Hooks und Einspeisung wurden noch nicht
  mit zwei echten Rechnern getestet.

**Bedienung:**
- **Rollen-Taste** hält den Cursor auf dem aktuellen Bildschirm fest, z. B. für Spiele. Die Taste ist frei
  wählbar (Einstellungen bzw. `[keys] lock` in `qross.toml`).
- **Spielemodus:** Läuft auf dem Rechner mit dem Cursor ein Spiel, ein Video oder eine Präsentation im Vollbild,
  bleibt der Cursor dort, bis es endet. Spiele auf einem Client bekommen dabei relative Mausbewegungen wie von
  einer echten Maus, auch wenn sie den Zeiger einfangen. Die Fang-Taste gibt den Cursor frei; abschaltbar in
  den Einstellungen bzw. mit `[game] enabled = false`.
- **Strg+Alt+Shift+Esc** holt Maus und Tastatur sofort zurück (macOS: Strg+Option+Shift+Esc).
- **Verbindungsqualität** (Klick auf die Plakette mit den Millisekunden): Latenz, Jitter, Verlust und Durchsatz,
  darunter die Verzögerung der Eingabe vom Server bis zum Einspeisen am Client, für Maus und Tasten getrennt und
  aufgeteilt in Server, Netz und Client. So siehst du, ob eine Verzögerung am Netz oder an einem der Rechner
  liegt. Wie gemessen wird und wie genau: [Protokoll](docs/protokoll.md#verzögerung-der-eingabekette).
- **Profile je Ort** (Einstellungen → Profile): z. B. „Büro“ als Client und „Zuhause“ als Server mit eigener
  Anordnung. Ein Profil umfasst alles aus dem Hauptfenster: Rolle, Anordnung, Name und Port, Wechsel und die
  Server-Adresse. Umschalten in der Kopfzeile oder im Tray-Menü. Mit „Aktuelles Netzwerk übernehmen“ wählt
  Qross das Profil beim Start und bei jedem Netzwerkwechsel selbst und startet Server bzw. Client neu.
- **Kürzel zum Wechseln** (frei belegbar, Einstellungen → Tastatur bzw. `[keys.switch]`): zu einem bestimmten
  Rechner, der Cursor landet in dessen Mitte, oder zum Nachbarn links, rechts, oben oder unten, dann an der
  gegenüberliegenden Kante. Sie wirken auf jedem Rechner und erreichen keine Programme; hält die Fang-Taste den
  Cursor fest, wechselt er nicht. Unter X11 wirkt ein Kürzel, das der Desktop schon belegt (etwa Strg+Alt+Pfeil
  für Arbeitsflächen), nur, solange der Cursor auf einem anderen Rechner ist.
- **Rechte je Rechner** (Server-Ansicht bzw. `[sharing]`): je Client und Art – Zwischenablage, kopierte Dateien,
  Drag & Drop – senden und empfangen, nur empfangen, nur senden oder aus. Was ein Client sendet, erreicht über
  den Server auch andere Rechner, soweit sie es empfangen dürfen. Der Server setzt das selbst durch, auch bei
  älteren Clients; Änderungen gelten sofort.
- **Cursor-Finder** (Einstellungen → Allgemein bzw. `[finder]`, Standard aus): Beim Wechsel blitzt auf dem
  Rechner, auf dem der Cursor ankommt, etwa eine Sekunde lang ein Ring um den Cursor auf und darunter sein Name;
  beides einzeln schaltbar. Eingestellt wird am Server, er gibt es an alle Clients weiter. „Ausprobieren“ zeigt
  es sofort. Die Anzeige lässt Klicks hindurch und erscheint nicht über Programmen im Vollbild.
- **Drag-Vorschau:** Ziehst du Dateien über die Kante, hängt auf dem anderen Rechner ein Kärtchen am Cursor wie
  beim Ziehen auf dem eigenen: Symbol, Name der Datei (bei mehreren „3 Dateien“) und Gesamtgröße. Es verschwindet
  beim Loslassen, wenn du ohne Ablegen zurückgehst oder die Übertragung abgebrochen wird.
- **Zeichen statt Tastenposition** (je Rechner, Einstellungen → Tastatur bzw. `[keys.mode]`): Standard ist die
  Tastenposition, dort bestimmt das eigene Layout das Zeichen. Auf „Zeichen“ kommt an, was du auf dem Server
  tippst, auch bei anderem Layout (z. B. Server Deutsch, Mac US): Y und Z, Umlaute, @, €, Klammern, ^ und ´.
  Kürzel wie Strg+Z gehen nach dem Zeichen, Pfeile, F-Tasten und Co. bleiben Tasten. Für Spiele die
  Tastenposition lassen. Clients mit älterem Qross oder unter Wayland bekommen weiter Tastenpositionen.
- **Feststelltaste, Num und Rollen** stehen auf allen Rechnern gleich, wie bei Synergy. Betritt der Cursor einen
  Client, übernimmt der den Stand des Servers. Drückst du die Taste, während der Cursor auf einem Client ist,
  schalten Server und Client um, und die Leuchten der Tastatur stimmen. Die eigene Tastatur eines Clients
  schaltet nur dort, bis der Cursor ihn das nächste Mal betritt. Am Mac gibt es nur die Feststelltaste; Num
  und Rollen eines Clients bleiben dann, wie sie sind. Mit Qross bis 0.7 auf einer Seite geht die Taste wie
  bisher nur an den Rechner mit dem Cursor.
- **Bildschirmschoner** (Einstellungen → Bildschirmschoner bzw. `[screensaver]`, Standard an): Startet er auf
  dem Server, starten ihn auch die Clients; bewegst du dort die Maus, enden alle. Der Cursor bleibt dabei, wo
  er ist, auch auf einem Client. „Clients mitsperren“ (Standard aus) sperrt die Clients stattdessen. **Ein gesperrter Client wird an seiner eigenen Tastatur entsperrt**, siehe ⁶.
  Clients ab Qross 0.8; ältere laufen unverändert weiter.

## Download

Fertige Pakete gibt es unter [Releases](https://github.com/xozy22/qross/releases/latest):

| System | App | Kommandozeile |
|---|---|---|
| Windows x64 | `.msi` oder `-setup.exe` | `qross-…-x86_64-pc-windows-msvc.zip` |
| macOS Apple Silicon | `.dmg` | `qross-…-aarch64-apple-darwin.tar.gz` |
| Linux x64 | `.AppImage` oder `.deb` | `qross-…-x86_64-unknown-linux-gnu.tar.gz` |

Die Programme sind nicht von Microsoft bzw. Apple beglaubigt, deshalb fragt das System beim ersten Start nach:

- **Windows:** SmartScreen meldet sich, dann „Weitere Informationen → Trotzdem ausführen“.
- **macOS:** Qross aus dem DMG in „Programme“ ziehen und öffnen. macOS meldet, dass es die App nicht
  überprüfen kann. Dann unter **Systemeinstellungen → Datenschutz & Sicherheit** ganz unten „Dennoch öffnen“
  wählen. Danach dort „Bedienungshilfen“ und „Eingabeüberwachung“ für Qross erlauben. Alternativ im Terminal:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Qross.app
  ```

  Für die Kommandozeile entsprechend `xattr -d com.apple.quarantine qross` im entpackten Ordner.

### macOS: Rechte über Updates behalten

macOS merkt sich „Bedienungshilfen“ und „Eingabeüberwachung“ zusammen mit der Signatur der App, genauer mit
ihrer designierten Anforderung. Bis 0.5.0 war Qross nur ad hoc signiert, dort steckt in dieser Anforderung eine
Prüfsumme genau dieses Builds. Jedes Update galt deshalb als andere App: Qross blieb in der Liste eingeschaltet,
die Rechte wirkten aber nicht mehr, und Maus und Tastatur kamen nicht an.

Ab der ersten Version nach 0.5.0 ist Qross mit einem eigenen, dauerhaften Zertifikat („Qross Code Signing“)
signiert. Die Anforderung nennt dann nur noch den Bundle-Identifier und das Zertifikat, und die Rechte bleiben
über Updates erhalten. Beim Update von 0.5.0 oder älter müssen sie ein letztes Mal neu erteilt werden:

1. Qross starten. Fehlt ein Recht, zeigt Qross oben im Fenster einen Hinweis.
2. Unter **Systemeinstellungen → Datenschutz & Sicherheit → Bedienungshilfen** Qross auswählen und mit „−“
   entfernen. Der alte Eintrag bleibt sonst eingeschaltet, wirkt aber nicht.
3. Im Hinweis von Qross „Freigeben …“ wählen und Qross in der Liste einschalten.
4. Läuft dieser Mac als Server, dasselbe unter **Eingabeüberwachung**. macOS bietet danach an, Qross neu zu öffnen.

Statt Schritt 2 geht auch das Terminal, danach Qross neu starten:

```bash
tccutil reset Accessibility app.qross.desktop
```

```bash
tccutil reset ListenEvent app.qross.desktop
```

### macOS: Sichere Tastatureingabe

Ein aktives Passwortfeld, Terminal mit „Sichere Tastatureingabe“ (Menü „Terminal“) oder ein Passwortmanager
schaltet unter macOS die sichere Tastatureingabe ein. Solange sie an ist, gibt macOS Tasten an kein Programm
weiter, das mitliest, auch nicht an Qross; die Maus funktioniert weiter. Mit dem Mac als Server hieße das: Der
Cursor wechselt zum anderen Rechner, die Tasten aber landen im Programm vorn auf dem Mac, schlimmstenfalls in
einem Terminal oder Chat.

Qross sieht deshalb viermal pro Sekunde nach. Ist sie an, holt es den Cursor zurück auf den Mac und lässt ihn
dort, bis sie endet, auch gegen Fang-Taste und Spielemodus. Ist ein Client verbunden, nennt ein Hinweis oben im
Fenster und im Tray das Programm und was hilft; wird der Cursor zurückgeholt oder bleibt er an der Kante
stehen, kommt einmal eine Systembenachrichtigung. Hält ein Programm sie fest, obwohl kein Passwortfeld offen ist:

- **Terminal, iTerm2:** im Menü des Programms „Sichere Tastatureingabe“ bzw. „Secure Keyboard Entry“ ausschalten
  oder ein anderes Programm nach vorn holen.
- **loginwindow:** ein bekannter Fehler von macOS nach dem Sperren. Den Bildschirm sperren (⌃⌘Q) und wieder
  entsperren, sonst ab- und wieder anmelden.
- **Sonst** das genannte Programm beenden. Welches es ist, zeigt auch das Terminal:

  ```bash
  ioreg -l -w 0 | grep SecureInput
  ```

  Die Zahl hinter `kCGSSessionSecureInputPID` ist die Prozessnummer.

Das Zertifikat ist selbst signiert, nicht von Apple ausgestellt. Gatekeeper fragt deshalb beim ersten Start wie
bisher nach („kann nicht überprüft werden“, freizugeben unter Datenschutz & Sicherheit). Welche Anforderung eine
installierte Version hat, zeigt:

```bash
codesign -d -r- /Applications/Qross.app
```

Signiert mit dem Qross-Zertifikat nennt die Ausgabe `certificate root = H"12208ca2cc71fcbb560201275b2eaec6a8afdabb"`
(SHA-1 des Zertifikats), ad hoc signiert dagegen `cdhash H"…"`.

Was sich in jeder Version geändert hat, steht im [Changelog](CHANGELOG.md) und in der App unter „Neuigkeiten“.
Ab Version 0.4.0 aktualisiert sich die App selbst: Sie prüft beim Start und täglich, lädt Updates im
Hintergrund, prüft ihre Signatur und installiert sie, wenn gerade niemand Maus und Tastatur braucht
(abschaltbar unter Einstellungen → Updates).

### Mit dem System starten

Beim ersten Start trägt sich Qross in den Autostart ein und läuft danach bei jeder Anmeldung unsichtbar im
Tray, mit Server oder Client wie zuletzt. Ein- und ausschalten unter Einstellungen → Allgemein → „Mit dem
System starten“; darunter steht, wo das System den Eintrag zeigt. Unter macOS erst, wenn Qross im Ordner
„Programme“ liegt, nicht direkt aus dem DMG. Alles Weitere, auch für `qross` ohne Oberfläche:
[Autostart](docs/autostart.md).

## Einrichten

Am einfachsten über die grafische Oberfläche (`qross-gui`). Auf der Kommandozeile geht es so:

**Auf dem Server** (Rechner mit Maus und Tastatur): `qross.example.toml` nach `qross.toml` kopieren,
Namen und Anordnung eintragen und starten:

```bash
qross server
```

**Auf jedem Client:** Ohne Adresse sucht Qross den Server im LAN selbst.

```bash
qross client
```

Beim ersten Mal lehnt der Server den Client ab und zeigt im Log, wie er freigegeben wird. Auf dem Server
einmalig ausführen:

```bash
qross trust laptop <fingerprint-aus-dem-log>
```

Der Client merkt sich seinerseits den Fingerprint des Servers beim ersten Kontakt. Zum Vergleich
zeigt `qross fingerprint` auf dem Server den richtigen Wert.

Hinweise:
- Die Firewall muss auf dem Server **UDP 24900** (Verbindung) und **UDP 24901** (LAN-Suche) eingehend erlauben.
  Beim ersten Start fragt Windows nach.
- `qross discover` listet alle Qross-Server im LAN, `qross client <adresse>` verbindet gezielt.
- `[switching]` in `qross.toml`: `corner_size` (Ecken-Totzone in Pixeln, Standard 8) und `delay_ms`
  (so lange gegen die Kante drücken, bevor gewechselt wird, Standard 0).
- `[keys]` in `qross.toml`: `lock` (Fang-Taste, z. B. `"Ctrl+Alt+KeyL"`, leer = aus), `[keys.switch]` (Kürzel
  zum Wechseln: `"Ctrl+Alt+Digit2" = "laptop"` springt in die Mitte des Laptops, `"Ctrl+Alt+ArrowLeft" = "left"`
  zum linken Nachbarn; Richtungen `left`, `right`, `up`, `down`), `media` (`"active"` oder
  `"local"`), `[keys.remap.<rechner>]` zum Umbelegen und `[keys.mode]` (`laptop = "chars"`: Zeichen statt
  Tastenposition). Tastennamen wie `KeyboardEvent.code` im Browser.
- `[game] enabled` in `qross.toml` (Standard `true`) schaltet den Spielemodus; `qross client --no-game-mode`
  meldet dem Server nie, dass auf dem Client etwas im Vollbild läuft.
- `[finder]` in `qross.toml`: `ring` und `name` (Standard je `false`) schalten den Cursor-Finder für alle Rechner;
  die Clients erfahren es vom Server.
- `[sharing]` in `qross.toml`: `clipboard`, `files` und `drag` je `"both"` (Standard), `"to-client"`,
  `"from-client"` oder `"off"` für alle Rechner, `[sharing.clients.<rechner>]` für einzelne. Fehlende Angaben
  übernimmt ein Rechner aus `[sharing]`.
- `[clipboard] max_file_mb` in `qross.toml` bzw. `qross client --clipboard-max-mb`: kopierte Dateien bis zu
  dieser Gesamtgröße beim Wechsel mitgeben (Standard 1000, 0 = aus). Sie liegen danach im Cache
  (`%LOCALAPPDATA%\Qross\clipboard`) und werden nach einem Tag aufgeräumt. Fügt die Gegenstelle auf Abruf ein
  (Windows), gilt die Grenze nicht; 0 schaltet kopierte Dateien auch dann aus.
- `[clipboard] share_concealed` in `qross.toml` bzw. `qross client --share-concealed`: auch Inhalte teilen, die
  ein Passwortmanager als vertraulich markiert (Standard `false`: sie bleiben auf dem Rechner).
- Solange kein Client verbunden ist, verhält sich der Server völlig unauffällig.
- `qross client --dry-run` protokolliert Eingaben nur. Damit lässt sich testen, ohne einen zweiten Rechner fernzusteuern.
- `--log-file <datei>` und `-v` für ausführliche Logs.
- Daten (Identität, Vertrauenslisten) liegen in `%APPDATA%\Qross` bzw. `~/.config/Qross`, abweichend per `--data-dir`.

## Aufbau

| Crate | Aufgabe |
|---|---|
| `qross-proto` | Wire-Protokoll: Nachrichten, Framing ([Beschreibung](docs/protokoll.md)) |
| `qross-core` | Plattformunabhängige Logik: Layout, Zustandsmaschinen von Server und Client, Drag & Drop |
| `qross-platform` | Betriebssystem: Windows (Hooks, SendInput, OLE/Shell), Linux (X11/XTest, Wayland über Desktop-Portale und libei), macOS (CGEventTap) |
| `qross-net` | QUIC/TLS, Fingerprint-Vertrauen, LAN-Suche, Dateiübertragung, Laufzeit von Server und Client |
| `qross` | Kommandozeilenprogramm |
| `qross-gui` | Grafische Oberfläche (Tauri); die Seite liegt in `ui/`, ihr Code als ES-Module in `ui/js/` (`main.js` startet, je Bereich ein Modul), die Texte in `ui/js/lang/` (`de.js`, `en.js`); Logs und Fehlermeldungen sind englisch |
| `bench/netbench` | Benchmark TCP+TLS vs. QUIC (eigener Cargo-Workspace) |

Hintergrund: [Benchmark](docs/benchmark-ergebnisse.md) · [Protokoll](docs/protokoll.md) · [Wayland](docs/wayland.md)

## Bauen und testen

Voraussetzung: Rust 1.99 oder neuer.

```bash
cargo build --release
```

```bash
cargo test --workspace
```

Oberfläche (Node.js 22 oder neuer): ESLint und Tests ohne Browser, wie in der CI.

```bash
cd crates/qross-gui && npx --yes eslint@10.12.0 ui/js ui-tests
```

```bash
node --test "crates/qross-gui/ui-tests/*.test.mjs"
```

Linux-Tests mit virtuellem X-Server (Docker):

```bash
docker build -f docker/linux-test.Dockerfile -t qross-linux-test docker
```

```bash
docker run --rm -v "%CD%:/src" -v qross-cargo:/usr/local/cargo/registry -v qross-target:/target qross-linux-test
```

Diagnose für Drag & Drop: `cargo run -p qross-platform --example folder_at -- <x> <y>` zeigt den Ablageordner
an einer Bildschirmposition, `cargo run -p qross-platform --example finder -- drag <Datei …>` die Drag-Vorschau
am Cursor, ohne dass etwas gezogen wird. `cargo run -p qross-platform --example clipboard_check` prüft die echte
Zwischenablage dieses Rechners mit allen Formaten und stellt den vorherigen Inhalt danach wieder her.
Kopierte Dateien auf Abruf unter Windows: `cargo run -p qross-platform --example virtual_files` legt virtuelle
Dateien in die echte Zwischenablage und lässt sie den Explorer einfügen, `cargo run --release -p qross-net --example
paste_on_demand` dasselbe von Ende zu Ende über QUIC mit einer Datei von 1,2 GB; beide stellen den vorherigen
Inhalt wieder her.
`cargo run -p qross-net --example scripted_server -- drag <Client-Name> <Datei>` spielt einen Server mit
simulierter Maus, der einer Oberfläche im Client-Modus eine Datei hinüberzieht (oder mit `copy` kopiert, mit
`receive` ihre kopierten Dateien abholt). So lassen sich Fortschritt und Abbrechen ohne zweiten Rechner
ansehen.

### Releases und Signaturen

Ein Release entsteht per Tag `v*` über `.github/workflows/release.yml`. Der Workflow braucht diese Secrets:

| Secret | Wofür |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Signatur der Update-Pakete für den Updater der App |
| `MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD` | Selbst signiertes Code-Signing-Zertifikat „Qross Code Signing“ (p12 als Base64) für das macOS-App-Paket |

Die Schlüssel liegen außerdem lokal in `%USERPROFILE%\.tauri` (`qross.key`, `qross-macos-signing.p12`, jeweils
mit Passwortdatei), nie im Repository. Das macOS-Zertifikat heißt bewusst nicht `APPLE_CERTIFICATE`: Unter
diesem Namen importiert Tauri selbst und nimmt nur Zertifikate von Apple an. Es gilt bis 2056 und darf nicht
ausgetauscht werden, sonst verlieren alle Mac-Nutzer beim nächsten Update ihre Rechte. Der Job „macOS-Paket auf
frischem Mac prüfen“ bricht ein Release deshalb ab, wenn die neue Version die designierte Anforderung der
zuletzt veröffentlichten nicht erfüllt.

Die Tests laufen im Release nicht noch einmal: Der Job „CI für diesen Commit“ verlangt eine grüne CI für den
Commit des Tags und wartet, solange sie noch läuft. Gebaut wird mit dem Cache, den
`.github/workflows/release-cache.yml` auf main anlegt: wenn sich Abhängigkeiten oder Build-Einstellungen ändern
und zweimal die Woche für neue Rust-Versionen. Gibt es den Cache schon, baut er nichts. Einen Cache vom Tag
selbst sähe schon das nächste Release nicht, denn GitHub gibt einem Tag nur seine eigenen Caches und die von main.

Testlauf ohne Veröffentlichung, gebaut wird der Stand des Branches:

```bash
gh workflow run release.yml --ref main -f tag=v0.5.0 -f test_run=true
```

## Lizenz

[MIT](LICENSE) © 2026 DennisK
