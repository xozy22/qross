# Changelog

Alle nennenswerten Änderungen an Qross. Das Format folgt [Keep a Changelog](https://keepachangelog.com/de/1.1.0/),
die Versionen folgen [Semantic Versioning](https://semver.org/lang/de/).

Diese Datei ist die einzige Quelle für die Neuigkeiten: Die App zeigt sie unter „Neuigkeiten“ an, und
jedes GitHub-Release übernimmt den Abschnitt seiner Version.

Rubriken: **Neu**, **Verbessert**, **Behoben**, **Wichtig** (z. B. inkompatible Änderungen).

## [Unreleased]

### Verbessert

- **Einstellungen mit Bereichen:** Allgemein, Profile, Tastatur, Spielemodus, Updates, Zwischenablage und Info
  sind jetzt je eine eigene Seite statt einer langen Liste zum Scrollen. Bei schmalem Fenster stehen die Bereiche
  als Zeile oben; Pfeiltasten wechseln zwischen ihnen, und beim nächsten Öffnen kommt der zuletzt gewählte.
- **Autostart auf allen Systemen überarbeitet:** Qross verwaltet den Eintrag jetzt selbst, an denselben Orten
  wie bisher (Windows: Autostart-Liste, macOS: Anmeldeobjekte, Linux: `~/.config/autostart`). Verschiebst du
  Qross oder die AppImage, folgt der Eintrag beim nächsten Start. Unter Windows steht der Pfad in
  Anführungszeichen (Pfade mit Leerzeichen), unter macOS zeigen die Anmeldeobjekte „Qross“. Im System
  abgeschaltet (Task-Manager, GNOME Tweaks) zeigt Qross ihn als aus. Unter dem Schalter steht, wo der Eintrag
  zu finden ist. Alles dazu, auch für `qross` ohne Oberfläche als Dienst, steht in `docs/autostart.md`.
- **Rückfrage beim Wechsel der Rolle:** Läuft der Server und du wechselst zum Client – über den Schalter, ein
  Profil oder das Tray-Menü –, fragt Qross erst nach, denn der Server schaltet sich dabei aus und verbundene
  Rechner verlieren Maus und Tastatur. Ebenso, wenn ein verbundener Client zum Server werden soll.
- **Schneller wieder da nach dem Ruhezustand und bei Netzwechseln:** Qross achtet jetzt auf Ruhezustand,
  Aufwachen und Netzwechsel (WLAN gewechselt, Kabel ab oder an, VPN) und wartet nicht mehr auf eine
  Zeitüberschreitung. Wacht der Laptop auf, prüft der Client die Verbindung sofort und baut sie, wenn nötig,
  ohne Wartezeit neu auf – meist in Sekundenbruchteilen, sobald das Netz wieder da ist. Mit neuer Adresse zieht
  die Verbindung einfach mit um, ohne Neuaufbau. Geht ein Rechner in den Ruhezustand, meldet er sich vorher
  ab: Der Server holt den Cursor sofort zurück, statt ihn 5 s ins Leere zu schicken, und ein schlafender Server
  sagt es seinen Clients. Wacht der Server auf, trennt er Clients, die nicht mehr antworten. Ist der Server
  nicht erreichbar, versucht es der Client in der ersten Minute etwa jede Sekunde neu, danach seltener. Hängt
  nach dem Aufwachen die Namensauflösung, nimmt er die zuletzt erreichte Adresse.

### Behoben

- **„Ein Client namens … ist bereits verbunden“:** Meldete sich ein Rechner neu an, während der Server die alte
  Verbindung noch hielt (nach dem Aufwachen, einem Netzwechsel oder einem Neustart von Qross), wies der Server
  ihn ab, bis die alte nach einigen Sekunden auslief. Jetzt ersetzt die neue Verbindung die alte sofort, sofern
  sie vom selben Rechner kommt (gleicher Schlüssel). Laufen versehentlich zwei Clients mit demselben Namen und
  Schlüssel, gewinnt der neuere, und der ältere hört mit einer Meldung auf.
- **macOS: Autostart aus dem DMG:** Startete Qross das erste Mal direkt aus dem DMG oder aus der
  Quarantäne-Kopie, zeigte der Autostart danach ins Leere. Jetzt trägt es sich erst ein, wenn es im Ordner
  „Programme“ läuft, und sagt in den Einstellungen, warum.

## [0.7.0] - 2026-10-18

### Neu

- **Profile je Ort:** mehrere benannte Profile, z. B. „Büro“ und „Zuhause“, jedes mit eigener Rolle, Anordnung
  der Bildschirme, Name und Port, Wechsel-Einstellungen und Server-Adresse. Umschalten in der Kopfzeile oder im
  Tray-Menü; anlegen, umbenennen, duplizieren und löschen unter Einstellungen → Profile. Tastatur, Spielemodus,
  Zwischenablage, Updates und Autostart gelten weiter für alle Profile.
- **Profil automatisch nach Netzwerk:** Jedes Profil lässt sich an ein oder mehrere Netzwerke binden
  („Aktuelles Netzwerk übernehmen“). Qross erkennt das Netzwerk am Router, an Adresse und MAC-Adresse des
  Gateways, ohne zusätzliche Rechte. Es wechselt beim Start und sobald sich das Netzwerk ändert, startet Server
  bzw. Client mit dem neuen Profil neu und meldet den Wechsel. Passt kein Profil, gilt ein wählbares Profil für
  andere Netzwerke. Abschaltbar je Profil und insgesamt, auch im Tray-Menü.
- **Rechte je Rechner:** Der Server legt je Client fest, was geteilt wird: Zwischenablage (Text, HTML, Bild),
  kopierte Dateien und Drag & Drop, jeweils senden und empfangen, nur empfangen, nur senden oder gar nicht. Eine
  Vorgabe gilt für Rechner, die neu dazukommen. Einstellbar in der Server-Ansicht unter „Rechte je Rechner“ bzw.
  mit `[sharing]` in `qross.toml`. Änderungen gelten sofort, ohne die Verbindung zu trennen; laufende
  Übertragungen, die nicht mehr erlaubt sind, brechen ab.
- Eingeschränkte Rechner tragen in der Anordnung ein Schild. Clients zeigen an, was der Server für sie
  einschränkt, und versuchen es gar nicht erst: Wohin Drag & Drop nicht darf, wechselt der Cursor mit gedrückter
  Taste auch nicht.
- **Zeichen statt Tastenposition, je Rechner:** Steht ein Rechner unter Einstellungen → Tastatur auf „Zeichen“,
  kommt dort an, was du auf dem Server tippst, auch wenn er ein anderes Tastaturlayout hat (z. B. Server
  Deutsch, Mac US): Y und Z, Umlaute, @, €, Klammern, tote Tasten wie ^ und ´ sowie alle AltGr-Zeichen. Kürzel
  wie Strg+Z gehen nach dem Zeichen und bleiben dort „Rückgängig“, Pfeile, F-Tasten und Co. bleiben Tasten.
  Standard bleibt „Tastenposition“, wie sie Spiele brauchen. In `qross.toml`: `[keys.mode]`.
- **Passwörter bleiben, wo sie kopiert wurden:** Inhalte, die ein Passwortmanager wie KeePassXC als vertraulich
  markiert, teilt Qross nicht mit anderen Rechnern, weder als Text noch als Bild oder Dateiliste, in keiner
  Richtung. Drüben bleibt der vorige Inhalt, und der nächste normale Inhalt wird wieder geteilt. Qross erkennt
  die Markierungen von Windows (Verlauf, Cloud-Zwischenablage, Zwischenablage-Programme), macOS
  (`org.nspasteboard.ConcealedType` und `TransientType`) und Linux (`x-kde-passwordManagerHint`). Ein kurzer
  Hinweis im Fenster und im Protokoll sagt, wenn etwas zurückgehalten wurde, ohne den Inhalt zu zeigen.
  Abschaltbar unter Einstellungen → Zwischenablage bzw. mit `[clipboard] share_concealed = true`.

### Behoben

- In den Einstellungen schob der Sprung zu einem Bereich die Kopfzeile des Fensters aus dem Bild.

### Wichtig

- Die bisherigen Einstellungen werden beim ersten Start zum Profil „Standard“. Die Profile stehen in
  `profiles.json` neben `gui.json`; eine ältere Version lädt `gui.json` weiter und lässt die Profile in Ruhe.
- Den WLAN-Namen zeigt Qross unter Windows und Linux (NetworkManager). macOS gibt ihn ab Version 14 nur mit
  Ortungsdiensten heraus; dort zeigt Qross das Subnetz und erkennt das Netzwerk trotzdem.
- Der Server setzt die Rechte auch gegenüber Clients mit Qross 0.5 und 0.6 durch: Was sie trotzdem schicken,
  weist er ab, ohne Reste zu hinterlassen. Anzeigen können die Rechte erst Clients ab dieser Version.
- Zeichen bekommen nur Rechner mit dem neuen Qross unter Windows, macOS oder X11. Ältere Clients und Clients
  unter Wayland bekommen weiter Tastenpositionen; die Oberfläche zeigt das beim Rechner an. Ein Server unter
  Wayland schickt immer Tastenpositionen.

## [0.6.0] - 2026-10-18

### Neu

- **Spielemodus:** Läuft auf dem Rechner mit dem Cursor ein Spiel, ein Video oder eine Präsentation im Vollbild,
  bleibt der Cursor dort, bis es endet. Die Fang-Taste gibt ihn sofort frei. Die Oberfläche zeigt an, warum der
  Cursor festgehalten wird. Abschaltbar in den Einstellungen bzw. mit `[game] enabled = false`.
- Spiele auf einem Client bekommen relative Mausbewegungen wie von einer echten Maus, auch Spiele, die den
  Zeiger einfangen. Die Kamera dreht so auch am Bildschirmrand weiter.
- **Tastenkürzel zum Wechseln:** frei belegbare Kürzel springen direkt zu einem Rechner (der Cursor landet in
  dessen Mitte) oder zum Nachbarn in einer Richtung, z. B. Strg+Alt+1 oder Strg+Alt+Pfeil links. Einstellbar
  unter Einstellungen → Tastatur bzw. `[keys.switch]` in `qross.toml`.
- **Fortschritt bei Dateiübertragungen:** Drag & Drop und kopierte Dateien zeigen Prozent, Rate und Restzeit und
  lassen sich auf beiden Seiten abbrechen, ohne Reste zu hinterlassen.
- **Linux mit Wayland** (GNOME ab 45, KDE Plasma ab 6.1): Maus und Tastatur laufen über die Desktop-Portale.
  Beim ersten Start fragt das System einmal nach der Freigabe; wo der Desktop es kann, merkt sich Qross sie.
  Als Server erfasst Qross über InputCapture (auch Hyprland ab 0.56). Ohne passendes Portal speist der Client
  über XWayland ein. Die Oberfläche erklärt, wenn das Portal fehlt oder die Freigabe abgelehnt wurde.
- **macOS: Hinweis bei fehlenden Rechten:** Fehlen Qross „Bedienungshilfen“ oder „Eingabeüberwachung“, zeigt
  es das oben im Fenster an, mit einem Knopf, der Qross in die Liste der Systemeinstellungen einträgt und die
  passende Seite öffnet. Bisher kamen Maus und Tastatur dann einfach nicht an.

### Verbessert

- Nach dem Notausgang zeigt die Oberfläche die Fang-Taste wieder als gelöst an.

### Behoben

- **macOS: Rechte bleiben nach Updates erhalten:** Qross ist jetzt mit einem eigenen, dauerhaften Zertifikat
  signiert statt nur ad hoc. Bisher galt jede neue Version für macOS als andere App: Qross blieb unter
  „Bedienungshilfen“ und „Eingabeüberwachung“ eingeschaltet, die Rechte wirkten aber nicht mehr.

### Wichtig

- Mac-Nutzer müssen die Rechte nach diesem Update ein letztes Mal neu erteilen: Qross unter Systemeinstellungen
  → Datenschutz & Sicherheit → Bedienungshilfen (und, als Server, Eingabeüberwachung) mit „−“ entfernen und
  über den Hinweis in Qross neu eintragen. Ab dann bleiben sie über Updates erhalten.
- Rechner mit Qross 0.5 bleiben verbunden. Mit ihnen gilt der Spielemodus nur für ein Vollbild auf dem Server
  selbst.
- Unter Wayland wirkt die Fang-Taste nur, solange der Cursor auf einem Client ist, Drag & Drop geht dort noch
  nicht, und unter GNOME läuft die Zwischenablage über XWayland.

## [0.5.0] - 2026-10-18

### Neu

- **Mehrere Monitore je Rechner:** Qross kennt jetzt jeden Monitor eines Rechners einzeln. Der Layout-Editor
  zeigt sie maßstäblich im Kasten des Rechners, mit den Auflösungen im Tooltip.
- **Verbindungsqualität:** Für jede Verbindung zeigt Qross Latenz, Jitter, Paketverlust und Durchsatz, dazu eine
  Stufe von „Sehr gut“ bis „Schlecht“ und einen Verlauf der letzten Minute. Beim Server steht das je Client
  unter „Verbundene Rechner“, beim Client im Status. Gemessen wird ohne zusätzlichen Netzverkehr.

### Verbessert

- Bei unterschiedlich großen oder versetzten Monitoren bleibt der Cursor auf echten Bildschirmflächen, statt in
  toten Ecken des umschließenden Rechtecks zu hängen. Gewechselt wird nur dort, wo wirklich ein Monitor an der
  Kante liegt, und der Cursor kommt auf dem nächstgelegenen Monitor an.

### Wichtig

- Die einzelnen Monitore meldet ein Client nur an einen Server ab 0.5.0. Mit älteren Servern bleibt es beim
  umschließenden Rechteck, die Verbindung funktioniert weiter.

## [0.4.0] - 2026-10-18

### Neu

- **Automatische Updates:** Qross prüft beim Start und danach täglich, ob es eine neue Version gibt. Es lädt
  sie im Hintergrund, prüft ihre Signatur und installiert sie in einem ruhigen Moment, nie mitten in einer
  Fernsteuerung. Danach startet Qross neu und setzt Server bzw. Client fort.
- **Mit dem System starten:** Qross startet bei der Anmeldung unauffällig im Tray und setzt fort, was zuletzt
  lief.
- **Einstellungen:** ein eigener Bereich für Autostart, Updates, Zwischenablage und Tastatur, der mit künftigen
  Optionen wächst.
- **Drag & Drop in beide Richtungen:** Dateien lassen sich jetzt auch vom Client auf den Server ziehen. Im
  Explorer oder Finder greifen, über die Kante ziehen und auf dem Server loslassen. Sie landen im Ordner unter
  dem Cursor (Windows) bzw. in „Downloads/Qross“.
- **Fang-Taste frei wählbar:** Statt der Rollen-Taste z. B. Strg+Alt+L, auch auf dem Mac, der keine Rollen-Taste
  hat.
- **Medientasten:** Wiedergabe/Pause, Weiter, Zurück und Stopp gehen mit. Auf dem Mac wirken Lautstärke und
  Wiedergabe jetzt tatsächlich. Auf Wunsch bleiben Medientasten immer auf dem Server.
- **Tasten je Rechner umbelegen**, z. B. Alt und Windows-Taste für einen Mac tauschen.

### Behoben

- Wer auf dem Client etwas mit gedrückter Maustaste an die Kante zum Server zog, wechselte sofort zurück, und
  der Client ließ die Taste los. Gezogene Dateien landeten dabei ungewollt am Bildschirmrand. Mit gedrückter
  Taste wird jetzt nur gewechselt, wenn Dateien gezogen werden.

### Wichtig

- Automatische Updates gibt es ab dieser Version. Wer eine ältere Version nutzt, installiert 0.4.0 einmal von
  Hand.
- Drag & Drop zum Server braucht Qross 0.4.0 auf beiden Rechnern. „Medientasten immer auf dem Server“ wirkt,
  wenn der Server unter Windows läuft.

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

[0.7.0]: https://github.com/xozy22/qross/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/xozy22/qross/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/xozy22/qross/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/xozy22/qross/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/xozy22/qross/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/xozy22/qross/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/xozy22/qross/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/xozy22/qross/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/xozy22/qross/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/xozy22/qross/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/xozy22/qross/releases/tag/v0.1.0
