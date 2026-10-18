# Autostart

Qross kann bei der Anmeldung von selbst starten, auf Windows, macOS und Linux (X11 und Wayland). Dann
läuft es unsichtbar im Tray und setzt fort, was beim letzten Beenden lief: Server oder Client
(Einstellungen → Allgemein → „Server oder Client beim Start fortsetzen“).

## In der App

Einstellungen → Allgemein → **„Mit dem System starten“**. Beim allerersten Start schaltet Qross das selbst
ein; danach gilt nur noch, was du dort einstellst. Unter dem Schalter steht, wo das System den Eintrag
zeigt, oder warum er sich gerade nicht setzen lässt.

Qross trägt sich nur für den angemeldeten Nutzer ein, ohne Administratorrechte:

| System | Eintrag | Im System zu finden |
|---|---|---|
| Windows | Wert `Qross` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | Einstellungen → Apps → Autostart, Task-Manager → Autostart-Apps |
| macOS | LaunchAgent `~/Library/LaunchAgents/Qross.plist` | Systemeinstellungen → Allgemein → Anmeldeobjekte (ab macOS 15 „Anmeldeobjekte & Erweiterungen“), Abschnitt „Im Hintergrund erlauben“ |
| Linux | `~/.config/autostart/Qross.desktop` (XDG-Autostart) | GNOME: Tweaks → Startprogramme, KDE: Systemeinstellungen → Autostart, Xfce: Sitzung und Startverhalten |

Gestartet wird `qross-gui --autostart`; der Schalter `--autostart` sorgt dafür, dass das Fenster zu bleibt.
Ohne Tray (etwa GNOME ohne AppIndicator-Erweiterung) erscheint es minimiert.

### Gut zu wissen

- **Im System ausgeschaltet:** Schaltest du Qross unter Windows im Task-Manager bzw. in den Einstellungen
  ab, oder unter Linux in GNOME Tweaks/KDE, zeigt Qross den Autostart als aus und lässt ihn so. Schaltest
  du ihn in Qross wieder ein, gilt er wieder. Den Schalter „Im Hintergrund erlauben“ in den
  macOS-Anmeldeobjekten kann Qross nicht lesen; ist er aus, startet Qross trotz Häkchen nicht.
- **Verschoben:** Der Eintrag zeigt auf die Programmdatei, aus der Qross gerade läuft, unter Linux auf die
  AppImage selbst. Verschiebst du Qross oder die AppImage, bringt der nächste Start den Eintrag auf den neuen
  Ort. Updates behalten den Ort ohnehin.
- **macOS: erst in „Programme“:** Startet Qross direkt aus dem DMG oder aus der Quarantäne-Kopie, die macOS
  für frisch geladene Apps anlegt, gibt es diesen Ort nach dem Neustart nicht mehr. Dann trägt sich Qross
  nicht ein und sagt das in den Einstellungen. Zieh Qross in den Ordner „Programme“ und starte es dort;
  beim ersten Start von dort richtet es den Autostart ein.
- **Entwicklungsbuilds** (`cargo run -p qross-gui`) tragen sich nie ein, damit kein Testbuild bei der
  Anmeldung startet.
- **Deinstallieren:** Am saubersten vorher in Qross ausschalten. Bleibt ein Eintrag stehen, lässt er sich
  von Hand löschen (Tabelle oben); er zeigt dann ins Leere und richtet keinen Schaden an.
- **Bis Qross 0.7.0** kam der Eintrag von `tauri-plugin-autostart`, mit denselben Namen und Orten. Qross
  übernimmt ihn und bringt ihn beim ersten Start auf den neuen Stand (unter Windows mit Anführungszeichen um
  den Pfad, unter macOS mit Bundle-ID, damit die Anmeldeobjekte „Qross“ zeigen).

## Ohne Oberfläche: `qross` als Dienst des Nutzers

Wer nur die Kommandozeile nutzt (etwa auf einem Linux-Rechner ohne Tray), startet `qross` über die
Dienstverwaltung des Systems. Nicht beides zugleich: Die App und ein `qross server` würden sich um
UDP-Port 24900 streiten.

Pfade anpassen: Programm, Konfiguration und, für den Client, die Server-Adresse (ohne sucht er im LAN).

### Linux: systemd-Nutzerdienst

`~/.config/systemd/user/qross.service`:

```ini
[Unit]
Description=Qross – Maus und Tastatur teilen
# Erst mit der grafischen Sitzung: Qross braucht DISPLAY bzw. WAYLAND_DISPLAY
After=graphical-session.target
PartOf=graphical-session.target

[Service]
ExecStart=/usr/local/bin/qross server --config %h/.config/Qross/qross.toml
# Client statt Server: ExecStart=/usr/local/bin/qross client
Restart=on-failure
RestartSec=5

[Install]
WantedBy=graphical-session.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable --now qross.service
journalctl --user -u qross -f
```

GNOME und KDE geben DISPLAY bzw. WAYLAND_DISPLAY an systemd weiter. Fehlen sie (`systemctl --user
show-environment`), hilft `systemctl --user import-environment DISPLAY WAYLAND_DISPLAY XAUTHORITY` im
Autostart der Sitzung.

### macOS: LaunchAgent

`~/Library/LaunchAgents/app.qross.cli.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>app.qross.cli</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/qross</string>
    <string>client</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardErrorPath</key>
  <string>/tmp/qross.log</string>
</dict>
</plist>
```

```bash
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/app.qross.cli.plist
```

Die Rechte unter Datenschutz & Sicherheit (Bedienungshilfen, als Server auch Eingabeüberwachung) braucht
dann die Datei `qross` selbst, nicht das Terminal. Abschalten: `launchctl bootout gui/$(id -u)/app.qross.cli`.

### Windows: Aufgabenplanung

In der Eingabeaufforderung (`cmd`):

```bat
schtasks /Create /TN "Qross CLI" /SC ONLOGON /RL LIMITED /TR "\"C:\Tools\qross.exe\" client"
```

Startet bei der Anmeldung im Konto des Nutzers, ohne Administratorrechte. `qross.exe` ist ein
Konsolenprogramm, sein Fenster bleibt offen, solange es läuft; für den Schreibtisch ist die App die bessere
Wahl. Abschalten: `schtasks /Delete /TN "Qross CLI" /F`.
