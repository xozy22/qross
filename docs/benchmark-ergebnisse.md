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
   Clipboard-Streams begrenzen, Eingaben bevorzugen). Stand in Qross: siehe 3c.

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
Engpasses mitwarten. BBR (in quinn experimentell) bringt hier nichts. Qross bleibt beim Standard Cubic.
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

Ergebnis: kein messbarer Vorteil, die Unterschiede liegen in der Streuung. Qross bleibt bei den
Standard-ACKs. Zum Vergleich: TCP+TLS im Synergy-Stil lag unter denselben Verlustraten bei p99 33–590 ms.

## 3c. Qross selbst: Eingaben während Dateiübertragungen und nach einem Hänger

Werkzeug: [`crates/qross-net/examples/latency_probe.rs`](../crates/qross-net/examples/latency_probe.rs),
Datum: 2026-10-20. Anders als `netbench` misst das die echten Server und Clients von Qross (Release-Build unter
Windows, beide in einem Prozess mit je eigener Laufzeit, dazwischen ein UDP-Vermittler im Userspace). Der Hook ist
nachgestellt: eine Bewegung im Millisekundentakt (unter Windows rund 620 je Sekunde), mit jeder 20. ein
Tastendruck. Gemessen wird von Ende zu Ende, vom Hook bis eingespeist; „Maus“ = bis diese oder eine neuere
Position eingespeist ist. Je Szenario zwei Läufe (Lauf 1 / Lauf 2). Alle Werte in ms.

**Dateien** (`files`): 50 Mbit/s, 64 KB Warteschlange am Engpass (gut 10 ms), 1 ms Laufzeit je Richtung. Beim
Wechsel gehen 320 kopierte Dateien zu 256 KB (84 MB) zum Client. Werte, solange die Übertragung läuft:

| Stand | Maus p50 | p95 | p99 | max | Taste p50 | p95 | p99 | max | Mbit/s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| vorher | 10,1 / 9,8 | 24,9 / 24,4 | 29,8 / 27,8 | 169 / 32 | 114 / 85 | 1.800 / 928 | 2.234 / 1.500 | 2.389 / 1.696 | 32,5 / 32,6 |
| Zwischenstand: drei Streams, reihum | 6,8 / 10,1 | 25,4 / 43,3 | 63 / 179 | 344 / 613 | 6,7 / 9,2 | 24,6 / 37,2 | 65 / 168 | 302 / 363 | 25,9 / 27,5 |
| nachher | 9,7 / 9,8 | 24,7 / 24,8 | 29,1 / 29,2 | 42 / 43 | 10,8 / 9,6 | 25,5 / 24,3 | 29,9 / 28,3 | 50 / 32 | 33,3 / 32,6 |

Ohne Übertragung lagen Maus und Tasten in allen Läufen bei p50 2,0–2,4 ms und p99 3,6–6,4 ms; ganz ohne
Vermittler (Loopback) vorher wie nachher bei p50 0,2 ms und p99 0,8–1,7 ms.

1. **Vorher warteten Tastendrücke bis zu 2,4 s**, fast ganz beim Schreiben auf den Input-Stream (Server-Anteil der
   Eingabekette): Jede Datei lief auf einem eigenen Stream, und der Sender schrieb weiter, ohne auf Bestätigung zu
   warten. Die fertig geschriebenen Streams füllten quinns Sendefenster (10 MB je Verbindung); dann kam auch der
   Input-Stream erst wieder zum Zug, wenn etwas bestätigt war. Die Maus betraf das nicht, Datagramme zählen nicht
   zum Sendefenster.
2. **Jetzt warten höchstens drei fertig geschriebene Datei-Streams** auf ihre Bestätigung, und jede Datei hat eine
   Stufe niedrigere Priorität als die vorige. Tasten liegen damit wie die Maus bei p99 29 ms.
3. **Die Grenze allein machte die Maus schlechter** (Zwischenstand): Streams gleicher Priorität teilt quinn reihum
   auf, die wartenden wurden also zugleich fertig. Bis die nächsten nachrückten, stand die Leitung kurz still, danach
   kam ein Schwall, an dessen Ende Pakete in der vollen Warteschlange verloren gingen; erst die Verlusterkennung per
   Zeitablauf löste das. Mit gestaffelter Priorität geht die älteste Datei zuerst ganz hinaus, der Strom bleibt
   gleichmäßig, und der Durchsatz ist wieder der alte.
4. **Was bleibt**, ist die Warteschlange am Engpass: Füllt die Übertragung die Leitung, warten Maus und Tasten darin
   mit (hier p95 25 ms). Große Inhalte der Zwischenablage gehen deshalb erst, wenn die Maus nach dem Wechsel 200 ms
   ruht, höchstens 1 s danach; im Test ruht sie nie, die Übertragung beginnt dort nach 1 s statt sofort.

**Hänger** (`hang`): ohne Engpass, 1 ms Laufzeit je Richtung; zweimal 2 s lang kommt nichts durch (Funkloch). Je
Lauf zwei Hänger, also vier Werte je Stand. „Veraltet“: beim Einspeisen über 100 ms alt.

| Stand | danach veraltet nachgespielt | die älteste | erste Bewegung nach dem Hänger | wieder aktuell |
|---|---:|---:|---:|---:|
| vorher | 1.593–1.762 Positionen | 2,6–2,8 s | 1,9–2,2 s | 1,95–2,23 s |
| nachher | 0–1 Position | 0,2 s | 2,0–2,2 s | 2,0–2,2 s |

Vorher hielt quinns Warteschlange für Datagramme (1 MB, über 20 s Bewegungen) alles fest, was während des Hängers
anfiel, und schickte es danach der Reihe nach: Der Cursor fuhr die alte Bahn nach. Jetzt fasst sie 4 KB (etwa 100
Bewegungen), und der Client fasst wartende Bewegungen zusammen.

**Nicht behoben:** Bis nach einem Hänger überhaupt wieder etwas ankommt, vergeht etwa noch einmal so viel Zeit,
wie er dauerte. Solange nichts bestätigt wird, verdoppelt quinn die Wartezeit bis zur nächsten Sondierung (PTO);
die erste nach dem Ende des Hängers kommt entsprechend spät.

## 4. Entscheidung (Empfehlung)

**Eigenes Protokoll auf QUIC** (`quinn` + `rustls`):
- Maus-Bewegung: QUIC-Datagramme mit Sequenznummer, neueste gewinnt
- Tasten/Maustasten/Rad/Enter/Leave: eigener zuverlässiger Stream
- Clipboard/Dateien: je Übertragung ein eigener Stream, Senderate gedrosselt. Stand 2026-10-20: Die Rate ist nicht
  begrenzt (dafür müsste Qross die Kapazität der Leitung kennen, und die schwankt im WLAN). Stattdessen warten
  höchstens drei Datei-Streams auf ihre Bestätigung, nacheinander priorisiert, und große Inhalte der
  Zwischenablage gehen erst, wenn die Maus nach dem Wechsel ruht (siehe 3c).
- Control: Hello, Bildschirm-Info, Keepalive

Optional später ein Kompatibilitäts-Adapter für das Synergy/Deskflow-Protokoll (TCP 24800).

## 5. Grenzen der Messung

- Zufälliger, gleichverteilter Verlust. Echtes WLAN verliert eher in Bursts, und 802.11 verdeckt durch eigene
  Retransmissions viel davon. 1–2 % IP-Verlust ist ein schlechtes, aber realistisches WLAN.
- Die TCP-Werte stammen vom Linux-TCP-Stack (min. RTO 200 ms). Windows liegt bei 300 ms und dürfte eher schlechter abschneiden. Das ist nicht gemessen.
- Gemessen wird nur der Netzwerkpfad, ohne Hook- und `SendInput`-Latenz. Nur ein Lauf je Konfiguration
  (außer TCP), es gibt also Lauf-zu-Lauf-Streuung im einstelligen ms-Bereich bei p99.9/max.
- Rohdaten: `bench/netbench/results-netem.csv`, `results-netem-clipboard.csv`, `final-run.log`.
- 3c: ein Rechner, Vermittler im Userspace (Zeitgeber mit etwa 1 ms Auflösung), Hook und Einspeisen nachgestellt,
  zwei Läufe je Szenario. Der Engpass verliert nur, wenn seine Warteschlange voll ist, nicht zufällig.
