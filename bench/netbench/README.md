# netbench

Misst, wie sich Paketverlust auf KVM-Eingabeverkehr (Maus, Tasten, Clipboard) auswirkt,
und vergleicht drei Transporte:

| Modus | Aufbau |
|---|---|
| `tcp` | TCP + TLS 1.3, `TCP_NODELAY`, **ein** Stream für alles, wie Synergy/Deskflow |
| `quic-stream` | QUIC, ebenfalls ein Stream für alles (isoliert den Transport-Effekt) |
| `quic` | QUIC, Mausbewegung als unzuverlässiges Datagramm, Tasten auf eigenem Stream, jede Clipboard-Übertragung auf eigenem Stream |

Sender und Empfänger laufen im selben Prozess über Loopback und teilen sich eine monotone Uhr.
Gemessen wird deshalb die echte Einweg-Latenz. Ein separater Thread erzeugt die Ereignisse taktgenau
(Standard: Maus 500 Hz, Tasten 20 Hz).

**Cursor-Latenz** = Zeit, bis beim Empfänger diese oder eine neuere Mausposition angekommen ist.
Ein verlorenes Datagramm zählt also so lange, bis die nächste Bewegung eintrifft. Das entspricht dem,
was man am Bildschirm sieht. `>50 ms` zählt spürbare Ruckler.

## Ausführen

Windows, ohne Admin-Rechte (Verlust nur für QUIC über ein UDP-Relay im Userspace):

```bash
cargo run --release -- --link relay --loss 0,1,5
```

Fairer Vergleich inklusive TCP über Linux `tc netem` in Docker:

```bash
docker build -t netbench .
```

```bash
docker run --rm --cap-add NET_ADMIN netbench --link netem --rate-mbit 100 --loss 0,0.5,1,2,5
```

Weitere Optionen: `--modes tcp,quic`, `--delay-ms`, `--jitter-ms`, `--clipboard-bytes 2000000`,
`--duration-s`, `--csv datei.csv`. Siehe `--help`.

## Grenzen

- Der Verlust ist zufällig und gleichverteilt. Echtes WLAN verliert eher in Bursts, und die
  802.11-Retransmissions verdecken einen Großteil davon.
- `netem` läuft auf `lo` im Container. Die TCP-Messung zeigt also das Verhalten des **Linux**-TCP-Stacks
  (min. RTO 200 ms). Der Windows-Stack verhält sich ähnlich (min. RTO 300 ms), wurde aber nicht gemessen.
- Gemessen wird nur das Netzwerk. Die Latenz von Hook und `SendInput` ist nicht enthalten.
