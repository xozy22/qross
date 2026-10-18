<p align="center">
  <img src="docs/assets/logo.png" width="128" alt="Glide-Logo">
</p>

<h1 align="center">Glide</h1>

<p align="center">Maus, Tastatur, Clipboard und Dateien über mehrere Rechner teilen – schnell, sicher, über QUIC.</p>

Eine eigene Implementierung in der Art von Synergy, geschrieben in Rust. Statt TCP verwendet Glide ein eigenes
Protokoll über **QUIC**. Bei Paketverlust (WLAN) bleibt der Cursor damit flüssig, wo ein TCP-Transport wie der von
Synergy 200–600 ms hängt ([Benchmark](docs/benchmark-ergebnisse.md)). Große Dateien wandern per **Drag & Drop** über
die Bildschirmkante, ohne dass Maus und Tastatur ins Stocken geraten.

<p align="center">
  <img src="docs/assets/screenshots/server.png" width="820" alt="Server-Ansicht mit Layout-Editor, verbundenen Rechnern und Freigabe-Anfrage">
</p>

<table>
  <tr>
    <td align="center"><img src="docs/assets/screenshots/client.png" alt="Client verbunden, Cursor auf diesem Rechner"><br><sub>Client: verbunden, der Cursor ist gerade hier</sub></td>
    <td align="center"><img src="docs/assets/screenshots/freigabe.png" alt="Freigabe-Dialog mit Fingerprint"><br><sub>Neuer Rechner: Freigabe erst nach Fingerprint-Vergleich</sub></td>
  </tr>
</table>

<sub>Die Screenshots zeigen Demo-Daten.</sub>

## Warum Glide statt Synergy?

| | Synergy 3 | Glide |
|---|---|---|
| Transport | TCP + TLS, ein Kanal für alles | QUIC: Maus als Datagramm, Tasten, Clipboard und Dateien auf getrennten Streams |
| Cursor bei 1 % / 5 % Paketverlust (p99)¹ | 36 ms / 588 ms | 4 ms / 4,5 ms |
| Eingaben während eines 2-MB-Clipboards (p99)¹ | 116 ms (0 % Verlust) / 248 ms (1 %) | 32 ms / 5 ms, keine Ruckler über 50 ms |
| Dateien per Drag & Drop | über dieselbe Verbindung wie die Eingaben | eigener Stream je Datei, SHA-256-geprüft, auch GB-große Dateien und Ordner |
| Vertrauen | TLS mit Fingerprint-Bestätigung | gegenseitige Zertifikate: der Server gibt Clients explizit frei, der Client merkt sich den Server |
| Versehentliches Wechseln an Ecken | Ecken-Schutz muss konfiguriert werden | Ecken-Totzone ab Werk, optionale Wechselverzögerung |
| PC ↔ Mac | Modifier von Hand umbelegen | Strg/Cmd wird automatisch getauscht |
| Verbindungsabbruch mitten in einer Eingabe | Tasten können hängen bleiben | Client lässt alles los; Notausgang Strg+Alt+Shift+Esc |

¹ Gemessen mit [`netbench`](bench/netbench/README.md): Linux-netem, 100 Mbit/s, 1 ms Verzögerung. Die Spalte
„Synergy 3“ ist eine Nachbildung von Synergys Transport (TCP+TLS, ein gemeinsamer Kanal), nicht das Original-
programm. Details im [Benchmark](docs/benchmark-ergebnisse.md).

## Funktionen und Stand

| | Windows | Linux (X11) | macOS |
|---|---|---|---|
| Server: Eingaben erfassen, Kanten pro Monitor | ✔ | ✔ | ✔¹ |
| Client: Eingaben einspeisen | ✔ | ✔ | ✔¹ |
| Text-Clipboard in beide Richtungen | ✔ | ✔ | ✔¹ |
| Cursor auf dem Server ausblenden | ✔ | ✔ | soweit macOS es zulässt¹ |
| Dateien per Drag & Drop **senden** (Server) | ✔ | – | – |
| Dateien per Drag & Drop **empfangen** | ✔ Explorer/Desktop unter dem Cursor | ✔ in `~/Downloads/Glide` | ✔ in `~/Downloads/Glide`¹ |
| Grafische Oberfläche mit Layout-Editor | ✔ | ✔² | ✔² |

¹ macOS ist nur per `cargo check` geprüft, weil kein Mac zur Verfügung stand. Glide braucht dort die Rechte
„Bedienungshilfen“ und „Eingabeüberwachung“.
² Die Oberfläche basiert auf Tauri; gebaut und geprüft wurde sie unter Windows.

**Wie geprüft wurde:**
- Plattformunabhängige Logik, Protokoll und Netzwerk: Unit-Tests und Ende-zu-Ende-Tests über echtes QUIC
  (Wechsel, Eingaben, Clipboard, Drag & Drop mit 8 MB und Ordnern, Ablehnung unbekannter Clients, LAN-Suche).
- Linux: echte Tests gegen einen X-Server (Xvfb in Docker).
- Windows: Bau- und Smoke-Tests sowie die Explorer-Ordnererkennung. Hooks und Einspeisung wurden noch nicht
  mit zwei echten Rechnern getestet.

**Bedienung:**
- **Rollen-Taste** hält den Cursor auf dem aktuellen Bildschirm fest, z. B. für Spiele.
- **Strg+Alt+Shift+Esc** holt Maus und Tastatur sofort zurück (macOS: Strg+Option+Shift+Esc).

## Einrichten

Am einfachsten über die grafische Oberfläche (`glide-gui`). Auf der Kommandozeile geht es so:

**Auf dem Server** (Rechner mit Maus und Tastatur): `glide.example.toml` nach `glide.toml` kopieren,
Namen und Anordnung eintragen und starten:

```bash
glide server
```

**Auf jedem Client:** Ohne Adresse sucht Glide den Server im LAN selbst.

```bash
glide client
```

Beim ersten Mal lehnt der Server den Client ab und zeigt im Log, wie er freigegeben wird. Auf dem Server
einmalig ausführen:

```bash
glide trust laptop <fingerprint-aus-dem-log>
```

Der Client merkt sich seinerseits den Fingerprint des Servers beim ersten Kontakt. Zum Vergleich
zeigt `glide fingerprint` auf dem Server den richtigen Wert.

Hinweise:
- Die Firewall muss auf dem Server **UDP 24900** (Verbindung) und **UDP 24901** (LAN-Suche) eingehend erlauben.
  Beim ersten Start fragt Windows nach.
- `glide discover` listet alle Glide-Server im LAN, `glide client <adresse>` verbindet gezielt.
- `[switching]` in `glide.toml`: `corner_size` (Ecken-Totzone in Pixeln, Standard 8) und `delay_ms`
  (so lange gegen die Kante drücken, bevor gewechselt wird, Standard 0).
- Solange kein Client verbunden ist, verhält sich der Server völlig unauffällig.
- `glide client --dry-run` protokolliert Eingaben nur. Damit lässt sich testen, ohne einen zweiten Rechner fernzusteuern.
- `--log-file <datei>` und `-v` für ausführliche Logs.
- Daten (Identität, Vertrauenslisten) liegen in `%APPDATA%\Glide` bzw. `~/.config/Glide`, abweichend per `--data-dir`.

## Aufbau

| Crate | Aufgabe |
|---|---|
| `glide-proto` | Wire-Protokoll: Nachrichten, Framing ([Beschreibung](docs/protokoll.md)) |
| `glide-core` | Plattformunabhängige Logik: Layout, Zustandsmaschinen von Server und Client, Drag & Drop |
| `glide-platform` | Betriebssystem: Windows (Hooks, SendInput, OLE/Shell), Linux (X11/XTest), macOS (CGEventTap) |
| `glide-net` | QUIC/TLS, Fingerprint-Vertrauen, LAN-Suche, Dateiübertragung, Laufzeit von Server und Client |
| `glide` | Kommandozeilenprogramm |
| `glide-gui` | Grafische Oberfläche (Tauri) |
| `bench/netbench` | Benchmark TCP+TLS vs. QUIC (eigener Cargo-Workspace) |

Hintergrund: [Analyse von Synergy 3](docs/analyse-synergy.md) · [Benchmark](docs/benchmark-ergebnisse.md) ·
[Protokoll](docs/protokoll.md)

## Bauen und testen

Voraussetzung: Rust 1.99 oder neuer.

```bash
cargo build --release
```

```bash
cargo test --workspace
```

Linux-Tests mit virtuellem X-Server (Docker):

```bash
docker build -f docker/linux-test.Dockerfile -t glide-linux-test docker
```

```bash
docker run --rm -v "%CD%:/src" -v glide-cargo:/usr/local/cargo/registry -v glide-target:/target glide-linux-test
```

Diagnose für Drag & Drop: `cargo run -p glide-platform --example folder_at -- <x> <y>` zeigt den Ablageordner
an einer Bildschirmposition.
