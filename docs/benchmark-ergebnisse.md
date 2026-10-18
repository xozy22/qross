# Benchmark: TCP+TLS (Synergy-Stil) vs. QUIC

Werkzeug: [`bench/netbench`](../bench/netbench/README.md). Datum: 2026-10-18.

**Aufbau:** Linux `tc netem` auf `lo` in Docker (Kernel 6.6, WSL2) mit 1 ms ± 0,3 ms Einweg-Verzögerung,
100 Mbit/s, MTU 1500 und zufälligem Paketverlust je Richtung. Maus 500 Hz, Tasten 20 Hz, 15 s je Lauf
(ca. 7.400 Mausereignisse). Alle Werte in ms. „Cursor“ = Zeit, bis diese oder eine neuere Mausposition
angekommen ist. „>50 ms“ = Anzahl spürbarer Ruckler.

## 1. Nur Eingaben

| Modus | Verlust | Cursor p50 | p99 | p99.9 | max | >50 ms | Taste p99 | Taste max |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| TCP+TLS (Synergy-Stil) | 0 % | 1,36 | 2,58 | 7,7 | 10,3 | 0 | 2,4 | 9,4 |
| QUIC, 1 Stream | 0 % | 1,57 | 3,95 | 13,1 | 20,0 | 0 | 3,7 | 8,4 |
| QUIC, Datagramme+Streams | 0 % | 1,55 | 3,45 | 10,2 | 15,8 | 0 | 3,9 | 14,3 |
| TCP+TLS | 0,5 % | 1,36 | 5,11 | 17,0 | 23,6 | 0 | 7,4 | 10,6 |
| QUIC, 1 Stream | 0,5 % | 1,58 | 4,11 | 8,1 | 15,1 | 0 | 3,6 | 12,6 |
| QUIC, Datagramme+Streams | 0,5 % | 1,49 | 3,14 | 6,4 | 18,3 | 0 | 3,3 | 5,4 |
| TCP+TLS | 1 % | 1,38 | **36,2** | **197** | **212** | **64** | 32,9 | 177 |
| QUIC, 1 Stream | 1 % | 1,55 | 5,98 | 13,1 | 22,0 | 0 | 4,3 | 5,7 |
| QUIC, Datagramme+Streams | 1 % | 1,52 | 3,94 | 25,8 | 39,8 | 0 | 5,0 | 7,3 |
| TCP+TLS | 2 % | 1,40 | **69,6** | **202** | **213** | **84** | 45,8 | 194 |
| QUIC, 1 Stream | 2 % | 1,54 | 6,30 | 9,8 | 16,7 | 0 | 6,9 | 11,6 |
| QUIC, Datagramme+Streams | 2 % | 1,53 | 3,82 | 8,5 | 15,5 | 0 | 5,3 | 6,3 |
| TCP+TLS | 5 % | 1,56 | **588** | **634** | **644** | **1612** | 587 | 637 |
| QUIC, 1 Stream | 5 % | 1,59 | 8,37 | 17,3 | 22,2 | 0 | 6,4 | 17,3 |
| QUIC, Datagramme+Streams | 5 % | 1,55 | 4,48 | 13,8 | 19,7 | 0 | 7,3 | 13,5 |

Eine zweite, unabhängige TCP-Messreihe (ohne Bandbreitengrenze) bestätigt den Verlauf:
bei 2 % p99 177 ms, bei 5 % p99 500–620 ms.

## 2. Eingaben + Clipboard (2 MB alle 5 s)

| Modus | Verlust | Cursor p99 | p99.9 | max | >50 ms | Taste p99 | Clipboard-Dauer |
|---|---:|---:|---:|---:|---:|---:|---:|
| TCP+TLS (Synergy-Stil) | 0 % | **116** | 182 | 196 | 140 | 108 | 182 |
| QUIC, 1 Stream | 0 % | 98,9 | 166 | 175 | 124 | 96 | 171 |
| QUIC, Datagramme+Streams | 0 % | 32,4 | 69 | 81 | 23 | 31 | 172 |
| TCP+TLS | 1 % | **248** | 336 | 357 | 359 | 236 | 312 |
| QUIC, 1 Stream | 1 % | 179 | 265 | 279 | 204 | 154 | 228 |
| QUIC, Datagramme+Streams | 1 % | 4,74 | 17,8 | 28,5 | 0 | 6,3 | 233 |

Ergänzend auf Windows nativ (QUIC über das Userspace-Relay, TCP dort nicht messbar): bei 5 % Verlust
Cursor p99 4,2 ms mit Datagrammen gegenüber 12,2 ms mit einem gemeinsamen Stream.

## 3. Was die Zahlen bedeuten

1. **Ohne Verlust gibt es keinen Grund für ein eigenes Protokoll.** Alle drei Varianten liegen bei ca. 1,5 ms.
   Kernel-TCP ist im Median sogar ca. 0,2 ms schneller als QUIC im Userspace. Das ist nicht spürbar.
2. **Ab ca. 1 % Verlust bricht TCP ein.** Typisch sind dann Hänger von ca. 200 ms (minimaler
   Retransmission-Timeout), bei 5 % über 0,5 s und in 22 % aller Mausereignisse Ruckler über 50 ms.
   QUIC bleibt in allen Fällen unter 40 ms, auch mit nur einem Stream. **Der größte Gewinn kommt also
   schon vom Transport (QUIC-Loss-Recovery ohne 200-ms-Untergrenze), nicht erst vom Datagramm-Design.**
3. **Datagramme für die Maus halbieren das p99 zusätzlich** (z. B. 2 %: 3,8 statt 6,3 ms). Verlorene
   Bewegungen fallen nicht auf, weil die nächste 2 ms später kommt.
4. **Clipboard auf demselben Stream wie die Eingaben ist der zweite große Fehler im Synergy-Design.**
   Ein 2-MB-Clipboard blockiert Maus und Tastatur für 100–250 ms, und zwar schon **ohne** Paketverlust.
   Getrennte Streams lösen das bei Verlust komplett. Ohne Verlust bleibt ein Rest (p99 32 ms), weil der
   Clipboard-Transfer die Leitung füllt und die Eingaben in der Netzwerk-Warteschlange mitwarten.
   → Bulk-Transfers müssen wir zusätzlich **drosseln bzw. priorisieren** (z. B. Senderate des
   Clipboard-Streams begrenzen, Eingaben bevorzugen).

## 3a. Congestion-Controller bei großen Dateien

QUIC mit Datagrammen+Streams, 100 Mbit/s, alle 5 s eine 20-MB-Übertragung, Datum 2026-10-18
(`--cc`, Rohdaten in `bench/netbench/results-cc.csv`).

| Controller | Verlust | Cursor p50 | p99 | p99.9 | max | >50 ms | Dauer 20 MB |
|---|---:|---:|---:|---:|---:|---:|---:|
| Cubic (Standard) | 0 % | 1,77 | 20,4 | 23,8 | 37,8 | 0 | 1,70 s |
| Cubic | 1 % | 1,77 | 7,6 | 28,1 | 34,6 | 0 | 2,30 s |
| BBR | 0 % | 1,64 | 33,8 | 36,1 | 42,0 | 0 | 1,71 s |
| BBR | 1 % | 1,68 | 12,2 | 26,5 | 31,9 | 0 | 1,70 s |
| NewReno | 0 % | 1,67 | 15,0 | 21,5 | 35,5 | 0 | 1,70 s |
| NewReno | 1 % | 1,67 | 6,3 | 25,9 | 30,4 | 0 | 2,22 s |

Ergebnis: Auch während eine Datei die Leitung voll auslastet (ca. 94 Mbit/s), gibt es keinen einzigen
Ruckler über 50 ms. Das p99 der Eingaben steigt auf 15–34 ms, weil sie in der Warteschlange des
Engpasses mitwarten. BBR (in quinn experimentell) bringt hier nichts. Glide bleibt beim Standard Cubic.
NewReno ist minimal besser, liegt aber innerhalb der Streuung eines einzelnen Laufs.

## 3b. Sofortige ACKs (ACK-Frequency-Erweiterung)

Idee: Verlorene Tastendrücke schneller erkennen, indem der Empfänger jedes Paket sofort bestätigt,
statt bis zu 25 ms zu warten (`--fast-ack`). Messung unter Windows über das Verlust-Relay, QUIC mit
Datagrammen+Streams, 200 Tastendrücke/s (ca. 4.000 je Lauf), 20 s.

| ACKs | Verlust | Taste p50 | p99 | max |
|---|---:|---:|---:|---:|
| Standard | 1 % | 2,41 | 6,86 | 13,3 |
| sofort | 1 % | 2,29 | 7,83 | 17,1 |
| Standard | 2 % | 2,23 | 7,63 | 13,7 |
| sofort | 2 % | 2,46 | 7,90 | 14,1 |
| Standard | 5 % | 2,19 | 9,18 | 19,4 |
| sofort | 5 % | 2,66 | 8,49 | 20,4 |

Ergebnis: kein messbarer Vorteil, die Unterschiede liegen in der Streuung. Glide bleibt bei den
Standard-ACKs. Zum Vergleich: TCP+TLS im Synergy-Stil lag unter denselben Verlustraten bei p99 33–590 ms.

## 4. Entscheidung (Empfehlung)

**Eigenes Protokoll auf QUIC** (`quinn` + `rustls`):
- Maus-Bewegung: QUIC-Datagramme mit Sequenznummer, neueste gewinnt
- Tasten/Maustasten/Rad/Enter/Leave: eigener zuverlässiger Stream
- Clipboard/Dateien: je Übertragung ein eigener Stream, Senderate gedrosselt
- Control: Hello, Bildschirm-Info, Keepalive

Optional später ein Kompatibilitäts-Adapter für das Synergy/Deskflow-Protokoll (TCP 24800).

## 5. Grenzen der Messung

- Zufälliger, gleichverteilter Verlust. Echtes WLAN verliert eher in Bursts, und 802.11 verdeckt durch eigene
  Retransmissions viel davon. 1–2 % IP-Verlust ist ein schlechtes, aber realistisches WLAN.
- Die TCP-Werte stammen vom Linux-TCP-Stack (min. RTO 200 ms). Windows liegt bei 300 ms und dürfte eher schlechter abschneiden. Das ist nicht gemessen.
- Gemessen wird nur der Netzwerkpfad, ohne Hook- und `SendInput`-Latenz. Nur ein Lauf je Konfiguration
  (außer TCP), es gibt also Lauf-zu-Lauf-Streuung im einstelligen ms-Bereich bei p99.9/max.
- Rohdaten: `bench/netbench/results-netem.csv`, `results-netem-clipboard.csv`, `final-run.log`.
