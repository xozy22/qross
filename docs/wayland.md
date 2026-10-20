# Qross unter Wayland

Unter Wayland darf ein Programm Maus und Tastatur weder global mitlesen noch einspeisen. Das
übernehmen die Desktop-Portale (xdg-desktop-portal) zusammen mit libei, und der Nutzer gibt es
einmal frei. Qross nutzt dafür:

| Aufgabe | Weg | Rust-Crates |
|---|---|---|
| Einspeisen (Client) | Portal `RemoteDesktop` ab Version 2, danach `ConnectToEIS` und libei als Sender | `ashpd`, `reis` |
| Erfassen (Server) | Portal `InputCapture` mit Zeigerbarrieren an den Bildschirmkanten, danach libei als Empfänger | `ashpd`, `reis` |
| Monitore | Regionen des libei-Geräts (Client) bzw. Zonen des Portals (Server) | – |
| Zwischenablage | `arboard` über ext-/wlr-data-control, sonst über XWayland | `arboard`, `wl-clipboard-rs` |

Beide Crates sind reines Rust. Qross braucht deshalb weder libei noch libportal als C-Bibliothek,
und das Linux-Paket bleibt so portabel wie bisher.

Stand: Oktober 2026. [B] heißt: belegt (Doku, Quelltext, NEWS gelesen), [V] heißt: vermutet.

## Was welcher Desktop kann

| Desktop | Einspeisen (RemoteDesktop + libei) | Erfassen (InputCapture) | Freigabe gemerkt: Einspeisen | Freigabe gemerkt: Erfassen | Zwischenablage ohne Fenster |
|---|---|---|---|---|---|
| GNOME 45–50 (Ubuntu 24.04: 46) | ja | ja | ja | nein, fragt bei jedem Start | kein data-control → XWayland |
| GNOME 51 | ja | ja | ja | ja | kein data-control → XWayland |
| KDE Plasma 5.27 | nein | nein | – | – | wlr-data-control |
| KDE Plasma 6.0 | nein (ohne libei) | nein | – | – | wlr-data-control |
| KDE Plasma 6.1–6.6 | ja | ja | ja (über Neustarts ab 6.1.1) | nein | wlr- bzw. ext-data-control |
| KDE Plasma 6.7 | ja | ja | ja | ja | ext-data-control |
| Hyprland 0.56 mit xdg-desktop-portal-hyprland 1.4 | nein | ja | – | nicht geprüft | wlr und ext |
| Sway und andere wlroots-Desktops | nein | nein | – | – | wlr und ext |

Belege [B]:
- RemoteDesktop: `ConnectToEIS` sowie `persist_mode`/`restore_token` ab Interface-Version 2
  (xdg-desktop-portal 1.17/1.18). GNOME merkt sich Freigaben ab 45, auch für reine
  Eingabe-Sitzungen ohne Bildschirmübertragung; KDE ab 6.1 (Neustarts ab 6.1.1, KDE-Bug 480235).
  <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html>,
  <https://github.com/flatpak/xdg-desktop-portal/blob/main/NEWS.md>, <https://bugs.kde.org/show_bug.cgi?id=480235>
- InputCapture: Version 1 ab xdg-desktop-portal 1.17, Version 2 (`CreateSession2`, `Start` mit
  Restore-Token) ab 1.21.1. GNOME ab 45, mit gemerkter Freigabe erst ab 51; KDE ab 6.1, gemerkt ab
  6.7; Hyprland ab xdg-desktop-portal-hyprland 1.4. xdg-desktop-portal-wlr bietet nur Screenshot und
  ScreenCast. <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.InputCapture.html>,
  <https://github.com/emersion/xdg-desktop-portal-wlr/blob/master/wlr.portal>
- Absolute Bewegung ohne Bildschirmübertragung: mutter legt dann ein gemeinsames Gerät mit einer
  Region je logischem Monitor an (`meta_eis_enable_monitor_viewports`, seit mutter 45). KWin gibt
  dem Absolut-Gerät immer eine Region je Ausgang.
  <https://github.com/GNOME/mutter/blob/main/src/backends/meta-remote-desktop-session.c>,
  <https://github.com/KDE/kwin/blob/master/src/plugins/eis/eisbackend.cpp>
- Zwischenablage: mutter hat kein data-control. KWin bot bis 6.3 wlr-data-control, ab 6.4 beides,
  ab 6.5 nur noch ext-data-control. wl-clipboard-rs (in arboard 3.7) kann ext-data-control seit
  0.9.2. <https://github.com/YaLTeR/wl-clipboard-rs/blob/master/CHANGELOG.md>

## Wie Qross es macht

Auswahl zur Laufzeit (`crates/qross-platform/src/linux.rs`):

- `WAYLAND_DISPLAY` gesetzt (oder `XDG_SESSION_TYPE=wayland`): Wayland. Sonst X11 wie bisher.
  `QROSS_BACKEND=x11` bzw. `QROSS_BACKEND=wayland` erzwingt eine Seite.
- **Client:** Bietet das Portal RemoteDesktop ab Version 2, speist Qross über libei ein. Fehlt es
  und gibt es XWayland (`DISPLAY`), speist Qross per XTest an XWayland ein und warnt im Protokoll.
  Das erreicht je nach Compositor nur X11-Programme. XWayland ab 23.1 leitet XTest unter GNOME
  selbst über das Portal weiter, dann fragt GNOME nach der Freigabe [V].
- **Server:** nur über InputCapture. X11-Grabs unter XWayland sehen den Cursor nur über
  X11-Fenstern, ein Rückfall wäre also kaputt. Fehlt das Portal, startet der Server mit einer
  klaren Meldung nicht.
- `LIBEI_SOCKET` gesetzt: Qross verbindet sich ohne Portal direkt mit diesem EIS-Socket (für Tests
  und Compositoren, die einen solchen Socket anbieten).

Module (`crates/qross-platform/src/`):

| Datei | Inhalt |
|---|---|
| `wayland/mod.rs` | gemeinsame Tokio-Laufzeit für die D-Bus-Verbindung, Portal-Erkennung, Fehlerarten, Monitore |
| `wayland/inject.rs` | RemoteDesktop, libei-Sender, Neuaufbau nach Verbindungsverlust |
| `wayland/capture.rs` | InputCapture, Barrieren, libei-Empfänger, Fang-Taste und Notausgang |
| `wayland/tokens.rs` | Restore-Token im Datenverzeichnis |
| `wayland_logic.rs` | reine Logik (Barrieren, Rad, Maustasten, Sitzungswahl), auch unter Windows getestet |

### Einspeisen

- Absolute Bewegung über das Gerät mit `ei_pointer_absolute`. Dessen Regionen sind die Monitore,
  die der Client dem Server meldet. Positionen außerhalb aller Regionen verwirft der Compositor, Qross
  legt sie deshalb vorher auf die nächste Region.
- Ohne Absolut-Gerät bewegt Qross relativ: erst in die linke obere Ecke, dann von der zuletzt
  gesetzten Position aus. Das ist ein Notbehelf, den kein getesteter Compositor braucht.
- Tasten gehen als evdev-Codes. Der Compositor deutet sie mit seiner eigenen Tastaturbelegung, wie
  bisher bei XTest unter X11.
- Mausrad als Rasten in 120steln (`scroll_discrete`), Maustasten als `BTN_*`.

### Erfassen

Anders als unter X11 und Windows entscheidet der Compositor, wann erfasst wird. Stößt der Cursor an
eine Barriere, ist die Erfassung schon aktiv, bevor der Server von der Kante weiß:

1. Barrieren liegen an jeder Monitorkante auf dem Außenrand der Gesamtfläche, je Monitor und Seite
   eine über die volle Kantenlänge (KDE verlangt die volle Länge, mutter lehnt Barrieren zwischen
   zwei Monitoren ab). Rechts und unten liegt die Linie bei `x + Breite` bzw. `y + Höhe`.
2. Löst eine Barriere aus, meldet Qross dem Server `EdgeHit`.
3. Wechselt der Server, gehen ab jetzt alle Eingaben an den Client.
4. Wechselt er nicht (Ecke, Wechselverzögerung, kein Nachbar), gibt Qross den Cursor frei, sobald
   sich die Maus von der Kante wegbewegt oder 300 ms lang nicht gegen die Kante drückt. Drückt der
   Nutzer weiter dagegen, kommt erneut `EdgeHit`, so wirkt auch die Wechselverzögerung.
5. Zurück auf den Server: `Release` mit der Position, an der der Cursor auftauchen soll.

Weitere Punkte:
- Strg+Alt+Shift+Esc holt Maus und Tastatur zurück.
- Beendet der Compositor die Erfassung selbst (`Deactivated`), kehrt der Server zurück.
- Schaltet er sie aus (`Disabled`), baut Qross die Sitzung neu auf. mutter lässt eine
  ausgeschaltete Sitzung nicht zuverlässig wieder einschalten (xdg-desktop-portal-gnome #126, so
  auch lan-mouse).
- Bei geänderten Monitoren (`ZonesChanged`) setzt Qross die Barrieren neu.

### Freigabe und Restore-Token

- Beim ersten Start zeigt das System einen Dialog. Qross fragt mit `persist_mode = 2` („bis auf
  Widerruf“) und speichert das zurückgegebene Restore-Token in `wayland-remote-desktop.token`
  bzw. `wayland-input-capture.token` im Datenverzeichnis (`~/.config/Qross` oder `--data-dir`,
  Rechte 0600).
- Jedes Token gilt nur einmal; nach jedem Start speichert Qross das neue.
- Beim nächsten Start gibt Qross das Token mit, und das Portal fragt nicht erneut, soweit der
  Desktop das kann (Tabelle oben). Für das Erfassen fragt GNOME bis Version 50 bei jedem Start.
- Abgelehnt: Der Start schlägt mit einer klaren Meldung fehl. Qross fragt nicht immer wieder,
  erst beim nächsten Start.
- Läuft der Dialog drei Minuten ohne Antwort, bricht der Start ab.
- Hostprogramme meldet Qross beim Portal als `app.qross.desktop` an
  (`org.freedesktop.host.portal.Registry`, ab xdg-desktop-portal 1.19.1). Ältere Portale kennen
  das nicht, dann eben ohne.
- In der Oberfläche: Einstellungen → Info → „Wayland-Freigaben“ zeigt, was gemerkt ist, und
  „Vergessen“ löscht die Token.

### Monitore und Koordinaten

Unter Wayland rechnet Qross in logischen Pixeln des Compositors, also mit Skalierung. Das ist
dasselbe Koordinatensystem, in dem libei-Regionen, InputCapture-Zonen und `Release` arbeiten. Server
und Client rechnen Kantenpositionen anteilig um, verschiedene Skalierungen auf beiden Seiten stören
deshalb nicht. Den Weg über `wl_output`/`xdg-output` braucht Qross nicht.

Vor der ersten Portal-Sitzung nennt Qross die Monitore von XWayland (RandR), ohne XWayland 1920×1080.
Die Sitzung ersetzt das gleich danach. Server und Client fragen ohnehin alle 2 s nach.

### Zwischenablage

`arboard` mit dem Feature `wayland-data-control`:
- KDE, Sway, Hyprland: direkt über data-control, auch ohne Fenster.
- GNOME: mutter kennt data-control nicht, `arboard` nimmt dann die X11-Zwischenablage von XWayland.
  Nach allgemeiner Erfahrung gleicht mutter beide Zwischenablagen ab (xclip funktioniert unter GNOME)
  [V]. Ohne XWayland gibt es unter GNOME keine Zwischenablage.

Passwörter: KeePassXC markiert sie mit dem MIME-Typ `x-kde-passwordManagerHint` und dem Inhalt
`secret` [B]. `arboard` liest den nicht; Qross fragt ihn auf demselben Weg selbst ab
(`clipboard_hint.rs`): über data-control mit `wl-clipboard-rs`, sonst über die Ziele (`TARGETS`) der
X11-Zwischenablage. Dass mutter den MIME-Typ beim Abgleich an XWayland weitergibt, ist vermutet [V].

Die saubere Lösung für GNOME wäre das Clipboard-Portal. Das gibt es nur zusammen mit einer
RemoteDesktop- bzw. InputCapture-Sitzung (GNOME ab 45 bzw. 51, KDE ab 6.4 bzw. 6.7). Deskflow nutzt es
seit 1.27. Für Qross ist das ein eigener nächster Schritt.

## Grenzen

- **Fang-Taste:** Sie wirkt nur, solange der Cursor auf einem Client ist. Lokal sieht Qross unter
  Wayland keine Tasten; dafür bräuchte es das Portal GlobalShortcuts.
- **Medientasten:** Sie gehen immer an den aktiven Rechner, wie unter X11.
- **Drag & Drop:** Weder vom Server noch zum Server. Der Server sieht beim Auslösen einer Barriere
  nicht, ob eine Maustaste gedrückt ist; Qross meldet die Kante deshalb wie ohne Taste.
- **Eingaben beim Warten:** Eine Taste oder ein Klick, während eine ausgelöste Barriere auf die
  Entscheidung des Servers wartet, gibt den Cursor frei und geht verloren (höchstens 300 ms nach
  dem Anstoßen an die Kante).
- **wlroots (Sway u. a.):** Weder Einspeisen noch Erfassen über Portale. Möglich wären die
  Wayland-Protokolle virtual-pointer/virtual-keyboard zum Einspeisen und eine layer-shell-Fläche an
  der Kante zum Erfassen (so macht es lan-mouse). Das ist nicht umgesetzt. Bis dahin: X11-Sitzung,
  oder als Client der XWayland-Rückfall für X11-Programme.
- **GNOME bis 50 als Server:** Der Freigabe-Dialog erscheint bei jedem Start des Servers und nach
  jedem Neuaufbau der Sitzung, z. B. wenn ein Monitor angesteckt wird.
- **Feststelltaste und Num:** Als Client übernimmt Qross den Stand des Servers, indem es die Taste
  über libei drückt; direkt setzen lässt er sich dort nicht. Wie die Tasten stehen, erfährt Qross aus
  `ei_keyboard.modifiers`, sofern der Compositor das meldet. Bis dahin gilt der zuletzt vorgegebene
  Stand als erreicht: Eine Änderung am Server schaltet mit, stand der Client aber schon anders, bleibt
  er verkehrt herum, bis der Compositor seinen Stand meldet. Num erkennt Qross auf Mod2, wie in den
  üblichen Belegungen; Rollen ist unter Wayland kein Modifier und bleibt außen vor. Als **Server**
  liest Qross den Stand nicht (InputCapture gibt ihn nicht heraus, und ob der Compositor beim
  Erfassen selbst umschaltet, ist offen). Die Taste geht dann wie früher nur an den Client.
- **Tastaturbelegung:** Der Client deutet Tasten mit seiner Belegung. Bei verschiedenen Belegungen
  auf Server und Client gilt dasselbe wie unter X11. Deskflow meldet Probleme mit AltGr unter
  Wayland [B, Diskussion #7499]; ob Qross das betrifft, ist nicht geprüft.

## Wie es andere lösen

- **Deskflow 1.27** (01.10.2026): Client über RemoteDesktop mit gemerkter Freigabe, Server über
  InputCapture (mit Token ab libportal 0.10). Braucht libei ≥ 1.3 und libportal ≥ 0.9.1 als
  C-Bibliotheken. Wayland-Zwischenablage über das Clipboard-Portal ist neu in 1.27.
  <https://github.com/deskflow/deskflow/blob/master/src/lib/platform/PortalInputCapture.cpp>,
  <https://github.com/deskflow/deskflow/discussions/7499>
- **Synergy 3:** „experimentell, GNOME 46+, Plasma 6.1+, keine Wayland-Zwischenablage“ (Hilfeseite;
  ob sie aktuell ist, ist offen). <https://symless.com/synergy/help/wayland-support-linux>
- **Input Leap 3.0:** Wayland über libei; 3.0.3 behebt einen Fehler mit der XKB-Belegung aus libei.
  <https://github.com/input-leap/input-leap/releases>
- **lan-mouse** (Rust, ashpd + reis wie Qross): Einspeisen über libei, das RemoteDesktop-Portal,
  wlroots virtual-pointer/-keyboard oder X11. Erfassen über layer-shell oder libei (InputCapture v1
  ohne Token). Baut die InputCapture-Sitzung nach jedem Ausschalten neu auf.
  <https://github.com/feschber/lan-mouse>

## Was geprüft ist

- Reine Logik (`wayland_logic.rs`): Barrieren für einen, zwei und gespiegelte Monitore, negative
  Koordinaten, Kantenzuordnung, Rad- und Tastenumrechnung, Sitzungswahl, welche Sperrtasten zu drücken
  sind. Unit-Tests laufen auf allen Plattformen.
- Einspeisen gegen eine EIS-Gegenstelle aus `reis` im Testprozess (`tests/wayland_ei.rs`, läuft in
  der normalen Linux-CI): Handshake, Geräte, Regionen als Monitore, Bewegung, Tasten, Rad.
- Einspeisen in einen echten mutter ohne Bildschirm (`tests/wayland_mutter.rs`, eigener CI-Job,
  experimentell): Die Sitzung kommt über mutters D-Bus-Schnittstelle statt über das Portal.
  Geprüft wird, dass mutter das Absolut-Gerät mit dem virtuellen Monitor als Region anbietet und die
  Eingaben ohne Protokollfehler annimmt. Ob der Cursor wirklich wandert, sieht der Test nicht.
- Übersetzt mit `cargo clippy --target x86_64-unknown-linux-gnu`. Lokal liefen die Linux-Tests nicht
  (Windows ohne Docker/WSL). Die Tests oben laufen erst in der GitHub-CI.

**Nicht geprüft:** das Desktop-Portal selbst (Dialog, Restore-Token, InputCapture mit Barrieren),
echte GNOME- und KDE-Sitzungen, die Zwischenablage unter Wayland, der XWayland-Rückfall.

### Warum die CI das Portal nicht testet

- Den Freigabe-Dialog von GNOME kann in der CI niemand bestätigen, einen offiziellen Weg dafür
  gibt es nicht.
- KWin aus Ubuntu 24.04 ist 5.27 und kann kein libei. Mit neuerem Plasma ließe sich die Freigabe
  vorab setzen (`flatpak permission-set kde-authorized remote-desktop <app-id> yes`, ab
  xdg-desktop-portal-kde 6.3). Das bräuchte einen Container mit aktuellem Plasma.
- Möglich wäre ein echtes xdg-desktop-portal mit einem Mock-Backend (Vorlagen in
  `tests/templates/` von xdg-desktop-portal). Das testet dann aber vor allem den Mock.
- Deshalb testet die CI die libei-Seite gegen mutter direkt und gegen eine eigene Gegenstelle.

## Testanleitung für einen echten Linux-Rechner

Voraussetzungen: GNOME ≥ 46 oder KDE Plasma ≥ 6.1 in einer Wayland-Sitzung (`echo $XDG_SESSION_TYPE`
zeigt `wayland`). Dazu ein zweiter Rechner mit Qross als Gegenstelle. Die Befehle zeigen das
Protokoll ausführlich.

1. **Portale vorhanden?**
   ```bash
   busctl --user get-property org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop org.freedesktop.portal.RemoteDesktop version
   ```
   ```bash
   busctl --user get-property org.freedesktop.portal.Desktop /org/freedesktop/portal/desktop org.freedesktop.portal.InputCapture version
   ```
   Erwartet: RemoteDesktop ≥ 2. InputCapture vorhanden (1 oder 2).
2. **Client unter Wayland** (anderer Rechner ist Server):
   ```bash
   qross -v client <server-adresse>
   ```
   - Erwartet: Dialog „Fernsteuerung/Remote Desktop“. Bei GNOME „Merken“ anhaken, dann erlauben.
     Im Protokoll stehen „Wayland injection via the portal (permission remembered)“ und
     „Wayland monitors: …“ mit den echten Monitoren in logischen Pixeln.
   - Cursor vom Server herüberschieben: Er bewegt sich auf allen Monitoren. Klicks, Rad (beide
     Richtungen), Tippen mit Umlauten, Strg+C/V funktionieren in einem Wayland-Programm (z. B.
     Texteditor) und in einem X11-Programm (z. B. `xterm`).
   - Feststelltaste am Server einschalten, Cursor herüberschieben: Der Client schreibt groß. Auf dem
     Client die Feststelltaste drücken: Server und Client schalten aus. Im Protokoll steht
     „libei: modifier state“; fehlt das, meldet der Compositor seinen Stand nicht, und der Client
     schaltet nur bei Änderungen mit.
   - Client beenden und neu starten: Diesmal kommt kein Dialog.
   - Freigabe über das System beenden (GNOME: Symbol oben rechts, „Stopp“): Im Protokoll steht
     „Wayland injection interrupted“, danach baut Qross die Verbindung ohne Dialog neu auf.
   - Dialog ablehnen (vorher in der GUI „Wayland-Freigaben → Vergessen“): Der Start meldet
     „the permission was denied“, die GUI zeigt den Hinweis mit „Erneut versuchen“.
3. **Server unter Wayland** (anderer Rechner ist Client):
   ```bash
   qross -v server --config qross.toml
   ```
   - Erwartet: Dialog zum Erfassen der Eingaben (GNOME bis 50 bei jedem Start). Im Protokoll steht
     „Wayland capture ready, monitors …, edges …“.
   - Cursor an die Kante zum Client: Er wechselt. Maus, Tasten, Rad, Fang-Taste (nur auf dem Client)
     und Strg+Alt+Shift+Esc funktionieren.
   - In eine Ecke (Totzone) oder an eine Kante ohne Nachbarn: Der Cursor bleibt höchstens kurz
     hängen und bewegt sich dann normal weiter.
   - Mit eingestellter Wechselverzögerung: Erst längeres Drücken gegen die Kante wechselt.
   - Zurück vom Client: Der Cursor erscheint an der richtigen Stelle der Kante.
   - Monitor an- oder abstecken: Im Protokoll steht „monitors changed, setting the barriers again“,
     bzw. die Sitzung wird neu aufgebaut. Danach wechselt der Cursor weiter.
4. **Zwischenablage:** Text, Bild (Screenshot) und eine Datei aus dem Dateimanager in beide
   Richtungen kopieren. Unter GNOME braucht das XWayland. Ein Passwort aus KeePassXC kommt nicht an,
   im Protokoll steht „clipboard: confidential content … is not shared“.
5. **Ohne Portal** (z. B. Sway): Der Client speist mit Warnung über XWayland ein. Der Server meldet
   „Capturing input (server) does not work under Wayland …“, die GUI zeigt den Hinweis.
6. **Erzwingen:** Mit `QROSS_BACKEND=x11` läuft unter XWayland der alte Weg. Das ist nur zum
   Vergleichen gedacht, erfassen kann der Server so nicht.

Wenn etwas nicht geht, helfen diese Protokolle:
```bash
journalctl --user -b -u xdg-desktop-portal -u xdg-desktop-portal-gnome -u plasma-xdg-desktop-portal-kde
```
Dazu das Qross-Protokoll mit `-v` bzw. `--log-file`.
