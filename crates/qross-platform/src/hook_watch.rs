//! Wächter für die Low-Level-Hooks unter Windows: reine Logik ohne Systemaufrufe, damit sie sich mit
//! nachgestellter Zeit testen lässt. Die Systemseite steht in `win::watchdog`.
//!
//! Windows entfernt `WH_KEYBOARD_LL` und `WH_MOUSE_LL` ohne jede Meldung, sobald ein Aufruf länger als
//! `LowLevelHooksTimeout` braucht, etwa wenn der Rechner kurz unter Last steht. Danach kommt keine
//! Eingabe mehr an, und Qross wechselt nicht mehr. Der Wächter vergleicht deshalb regelmäßig den
//! Zeitpunkt der letzten Eingabe im System (`GetLastInputInfo`) mit den Aufrufen der Hooks. Gab es
//! eine Eingabe, aber keinen Aufruf, schickt er zur Probe eine wirkungslose eigene Maus- und
//! Tastatureingabe. Sieht einer der beiden Hooks sie nicht, ist er verloren, und beide werden neu
//! gesetzt.
//!
//! Gegen Fehlalarme: Auf dem Sperrbildschirm, dem UAC-Desktop und in Fenstern mit höheren Rechten
//! erreichen Eingaben die Hooks nicht, obwohl sie leben. Das meldet `reachable`, und die erste Eingabe
//! nach der Rückkehr zählt nicht, sie kann noch drüben gelandet sein. Lehnt Windows die Probe ab
//! (gesperrte oder getrennte Sitzung), zählt sie nicht. Eigene Eingaben (`SendInput`) laufen durch die
//! eigenen Hooks und zählen dort mit. Nach dem Ruhezustand beginnt der Vergleich neu. Was die Hooks
//! aus anderen Gründen nie erreicht (z. B. Touch), entlarvt die Probe; danach probt der Wächter
//! seltener.

/// Abstand der Prüfungen in ms
pub(crate) const POLL_MS: u64 = 250;
/// So lange darf eine Eingabe ungesehen bleiben, bevor die Probe kommt. Ein lebender Hook sieht sie
/// nach Millisekunden.
const GRACE: u64 = 500;
/// So lange haben die Hooks Zeit, die Probe zu sehen: länger als `LowLevelHooksTimeout` (höchstens
/// eine Sekunde). Wer länger braucht, den hat Windows ohnehin entfernt.
const PROBE_WAIT: u64 = 1250;
/// Liegen zwei Prüfungen weiter auseinander, schlief der Rechner (oder der Wächter kam nicht dran):
/// Was dazwischen geschah, lässt sich nicht mehr zuordnen.
const MAX_GAP: u64 = 2000;
/// Höchstens so viel länger wartet der Wächter nach Fehlalarmen
const MAX_BACKOFF: u64 = 60_000;
/// So lange ohne neuen Fehlalarm, dann gilt wieder die kurze Wartezeit
const CALM: u64 = 10 * 60_000;

/// Aufrufe der beiden Hooks bisher (dürfen überlaufen)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Calls {
    pub(crate) mouse: u32,
    pub(crate) keyboard: u32,
}

/// Was der Wächter nach einer Prüfung tun soll
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Wait,
    /// Eine wirkungslose Maus- und Tastatureingabe schicken: Sehen beide Hooks sie, leben sie.
    Probe,
    /// Mindestens ein Hook ist verloren: neu setzen.
    Rehook,
}

#[derive(Clone, Copy)]
struct Sample {
    at: u64,
    input: u32,
    calls: Calls,
}

pub(crate) struct HookWatch {
    last: Option<Sample>,
    /// Seit wann eine Eingabe aussteht, die kein Hook gesehen hat
    unseen_since: Option<u64>,
    /// Wann die Probe abging, und die Aufrufe bis dahin
    probe: Option<(u64, Calls)>,
    /// Konnte die zuletzt geprüfte Eingabe die Hooks erreichen?
    reachable: bool,
    /// Neu gesetzt, und seitdem kein Aufruf
    rehooked: bool,
    /// Zusätzliche Wartezeit vor der Probe nach Fehlalarmen
    backoff: u64,
    /// Zeitpunkt des letzten Fehlalarms
    alarmed_at: Option<u64>,
}

impl HookWatch {
    pub(crate) const fn new() -> Self {
        Self {
            last: None,
            unseen_since: None,
            probe: None,
            reachable: true,
            rehooked: false,
            backoff: 0,
            alarmed_at: None,
        }
    }

    /// Eine Prüfung. `now`: monotone Zeit in ms, die im Ruhezustand weiterläuft; `input`: Zeitstempel
    /// der letzten Eingabe im System (nur auf Änderung verglichen, darf überlaufen); `calls`: Aufrufe
    /// der Hooks bisher. `reachable` sagt, ob Eingaben die Hooks gerade erreichen können, und wird nur
    /// bei Bedarf gefragt.
    pub(crate) fn poll(&mut self, now: u64, input: u32, calls: Calls, reachable: impl FnOnce() -> bool) -> Step {
        let Some(last) = self.last.replace(Sample { at: now, input, calls }) else {
            return Step::Wait;
        };
        if self.alarmed_at.is_some_and(|at| now.saturating_sub(at) >= CALM) {
            self.backoff = 0;
            self.alarmed_at = None;
        }
        if now.saturating_sub(last.at) > MAX_GAP {
            self.unseen_since = None;
            self.probe = None;
            return Step::Wait;
        }
        if let Some((at, before)) = self.probe {
            if calls.mouse != before.mouse && calls.keyboard != before.keyboard {
                // Beide sahen die Probe: Sie leben, nur die Eingabe davor erreichte sie nicht. Fehlalarm,
                // künftig länger warten.
                self.probe = None;
                self.unseen_since = None;
                self.reachable = true;
                self.rehooked = false;
                self.raise_backoff(now);
                return Step::Wait;
            }
            if now.saturating_sub(at) < PROBE_WAIT {
                return Step::Wait;
            }
            self.probe = None;
            self.unseen_since = None;
            if !reachable() {
                // Inzwischen auf dem Sperrbildschirm o. Ä.: Die Probe kam womöglich gar nicht durch.
                self.reachable = false;
                return Step::Wait;
            }
            // Hilft das Neusetzen nicht (kein Aufruf danach), nicht im Sekundentakt wiederholen.
            self.rehooked = true;
            self.raise_backoff(now);
            return Step::Rehook;
        }
        if calls != last.calls {
            // Die Hooks leben.
            if std::mem::take(&mut self.rehooked) {
                // Neusetzen hat geholfen
                self.backoff = 0;
                self.alarmed_at = None;
            }
            self.unseen_since = None;
            self.reachable = true;
            return Step::Wait;
        }
        if input != last.input && self.unseen_since.is_none() {
            let reachable = reachable();
            if reachable && self.reachable {
                self.unseen_since = Some(now);
            }
            self.reachable = reachable;
            return Step::Wait;
        }
        let Some(since) = self.unseen_since else { return Step::Wait };
        if now.saturating_sub(since) < GRACE + self.backoff {
            return Step::Wait;
        }
        if !reachable() {
            // `SendInput` käme jetzt nicht an (z. B. UAC-Desktop): keine Probe, die scheitern muss
            self.unseen_since = None;
            self.reachable = false;
            return Step::Wait;
        }
        self.probe = Some((now, calls));
        Step::Probe
    }

    /// Windows hat die Probe abgelehnt (`SendInput` gesperrt, etwa in einer gesperrten oder getrennten
    /// Sitzung): Dann erreichen auch andere Eingaben die Hooks gerade nicht.
    pub(crate) fn probe_refused(&mut self) {
        self.probe = None;
        self.unseen_since = None;
        self.reachable = false;
    }

    fn raise_backoff(&mut self, now: u64) {
        self.backoff = (self.backoff * 2).clamp(GRACE, MAX_BACKOFF);
        self.alarmed_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nachgestellte Welt: Uhr, letzte Eingabe im System, Hook-Aufrufe, Erreichbarkeit
    struct Sim {
        watch: HookWatch,
        now: u64,
        input: u32,
        calls: Calls,
        reachable: bool,
        asked: u32,
    }

    impl Sim {
        fn new() -> Self {
            let mut sim =
                Self { watch: HookWatch::new(), now: 1_000_000, input: 7, calls: Calls::default(), reachable: true, asked: 0 };
            assert_eq!(sim.poll(), Step::Wait, "die erste Prüfung legt nur den Stand fest");
            sim
        }

        fn poll(&mut self) -> Step {
            self.now += POLL_MS;
            let (reachable, asked) = (self.reachable, &mut self.asked);
            self.watch.poll(self.now, self.input, self.calls, || {
                *asked += 1;
                reachable
            })
        }

        /// Eine Eingabe im System, die kein Hook sieht
        fn unseen(&mut self) {
            self.input = self.input.wrapping_add(16);
        }

        /// Eine Mausbewegung, die der Maus-Hook sieht
        fn seen(&mut self) {
            self.unseen();
            self.calls.mouse = self.calls.mouse.wrapping_add(1);
        }

        /// Beide Hooks sehen die Probe.
        fn answer(&mut self) {
            self.seen();
            self.calls.keyboard = self.calls.keyboard.wrapping_add(1);
        }

        /// Prüft, bis der Wächter etwas anderes als warten will; vor jeder Prüfung `each`.
        /// Liefert den Schritt und die vergangene Zeit, `None` nach `max_ms`.
        fn until_step(&mut self, max_ms: u64, mut each: impl FnMut(&mut Self)) -> Option<(Step, u64)> {
            let start = self.now;
            while self.now - start < max_ms {
                each(self);
                match self.poll() {
                    Step::Wait => {}
                    step => return Some((step, self.now - start)),
                }
            }
            None
        }
    }

    #[test]
    fn living_hooks_never_alarm() {
        let mut sim = Sim::new();
        for i in 0..2000 {
            match i % 5 {
                0 => sim.seen(),
                // Eingabe kurz vor der Prüfung, der Hook kommt erst danach dran
                1 => sim.unseen(),
                2 => sim.calls.keyboard += 1,
                3 => sim.answer(),
                _ => {}
            }
            assert_eq!(sim.poll(), Step::Wait, "Prüfung {i}");
        }
    }

    #[test]
    fn lost_hooks_are_probed_then_set_again() {
        let mut sim = Sim::new();
        let (step, after) = sim.until_step(10_000, Sim::unseen).unwrap();
        assert_eq!(step, Step::Probe);
        assert!(after <= GRACE + 2 * POLL_MS, "Probe nach {after} ms");
        sim.unseen(); // die Probe selbst
        let (step, after) = sim.until_step(10_000, Sim::unseen).unwrap();
        assert_eq!(step, Step::Rehook);
        assert!((PROBE_WAIT..=PROBE_WAIT + POLL_MS).contains(&after), "neu gesetzt nach {after} ms");
    }

    #[test]
    fn a_single_unseen_input_is_enough() {
        let mut sim = Sim::new();
        sim.unseen();
        assert_eq!(sim.until_step(10_000, |_| {}).unwrap().0, Step::Probe);
        assert_eq!(sim.until_step(10_000, |_| {}).unwrap().0, Step::Rehook);
        assert_eq!(sim.until_step(30_000, |_| {}), None, "ohne neue Eingabe nichts weiter");
    }

    #[test]
    fn a_probe_seen_by_only_one_hook_still_rehooks() {
        // Windows hat nur den Tastatur-Hook entfernt; die Maus läuft weiter.
        let mut sim = Sim::new();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
        let (step, after) = sim.until_step(10_000, Sim::seen).unwrap();
        assert_eq!(step, Step::Rehook);
        assert!(after <= PROBE_WAIT + POLL_MS);
    }

    #[test]
    fn without_input_nothing_is_checked() {
        let mut sim = Sim::new();
        assert_eq!(sim.until_step(60_000, |_| {}), None);
        assert_eq!(sim.asked, 0, "die Erreichbarkeit kostet Systemaufrufe und wird nur bei Bedarf gefragt");
    }

    #[test]
    fn an_answered_probe_is_a_false_alarm_and_the_next_one_waits_longer() {
        let mut sim = Sim::new();
        let (step, first) = sim.until_step(10_000, Sim::unseen).unwrap();
        assert_eq!(step, Step::Probe);
        sim.answer(); // Die Hooks sehen die Probe: Sie leben, die Eingaben davor erreichten sie nur nicht (Touch).
        assert_eq!(sim.poll(), Step::Wait);
        let (step, second) = sim.until_step(10_000, Sim::unseen).unwrap();
        assert_eq!(step, Step::Probe);
        sim.answer();
        sim.poll();
        let (_, third) = sim.until_step(10_000, Sim::unseen).unwrap();
        assert!(first < second && second < third, "{first} < {second} < {third}");
        // Lange Ruhe: wieder die kurze Wartezeit
        sim.answer();
        sim.poll();
        sim.until_step(CALM, |_| {});
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().1, first);
    }

    #[test]
    fn the_backoff_has_a_ceiling() {
        let mut sim = Sim::new();
        for _ in 0..20 {
            assert_eq!(sim.until_step(2 * MAX_BACKOFF, Sim::unseen).unwrap().0, Step::Probe);
            sim.answer();
            sim.poll();
        }
        let (_, waited) = sim.until_step(2 * MAX_BACKOFF, Sim::unseen).unwrap();
        assert!(waited <= GRACE + MAX_BACKOFF + 2 * POLL_MS, "{waited} ms");
    }

    #[test]
    fn no_alarm_while_input_cannot_reach_the_hooks() {
        let mut sim = Sim::new();
        // Gesperrter Bildschirm, UAC oder ein Fenster mit Administratorrechten vorne
        sim.reachable = false;
        assert_eq!(sim.until_step(30_000, Sim::unseen), None);
        // Zurück: Die letzte Eingabe drüben (Enter nach der PIN) fällt in die erste Prüfung danach.
        sim.reachable = true;
        sim.unseen();
        assert_eq!(sim.until_step(30_000, |_| {}), None);
        // Danach wacht er wieder.
        sim.seen();
        sim.poll();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
    }

    #[test]
    fn no_probe_when_input_stops_reaching_the_hooks_meanwhile() {
        let mut sim = Sim::new();
        sim.unseen();
        sim.poll();
        sim.reachable = false;
        assert_eq!(sim.until_step(30_000, |_| {}), None, "SendInput käme nicht an");
    }

    #[test]
    fn no_rehook_when_the_probe_may_not_have_arrived() {
        let mut sim = Sim::new();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
        sim.reachable = false;
        assert_eq!(sim.until_step(30_000, |_| {}), None);
    }

    #[test]
    fn a_refused_probe_counts_nothing() {
        // Gesperrte oder getrennte Sitzung: Windows lehnt `SendInput` ab.
        let mut sim = Sim::new();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
        sim.watch.probe_refused();
        assert_eq!(sim.until_step(30_000, |_| {}), None);
        // Die erste Eingabe danach zählt noch nicht, die nächste wieder.
        sim.unseen();
        assert_eq!(sim.until_step(30_000, |_| {}), None);
        sim.seen();
        sim.poll();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
    }

    #[test]
    fn after_sleep_the_comparison_starts_over() {
        let mut sim = Sim::new();
        // Ruhezustand: Die Uhr springt, beim Aufwachen setzt Windows die letzte Eingabe neu.
        sim.now += 3_600_000;
        sim.unseen();
        assert_eq!(sim.until_step(30_000, |_| {}), None);
        // Auch eine ungesehene Eingabe kurz vor dem Schlafen zählt nicht.
        sim.unseen();
        sim.now += 3_600_000;
        assert_eq!(sim.until_step(30_000, |_| {}), None);
        // Danach wacht er wieder.
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
    }

    #[test]
    fn a_probe_interrupted_by_sleep_does_not_count() {
        let mut sim = Sim::new();
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
        sim.now += 3_600_000;
        assert_eq!(sim.until_step(30_000, |_| {}), None);
    }

    #[test]
    fn repeated_rehooks_without_effect_slow_down_and_reset_once_it_helps() {
        let mut sim = Sim::new();
        let rehook = |sim: &mut Sim| {
            let start = sim.now;
            assert_eq!(sim.until_step(4 * MAX_BACKOFF, Sim::unseen).unwrap().0, Step::Probe);
            assert_eq!(sim.until_step(4 * MAX_BACKOFF, Sim::unseen).unwrap().0, Step::Rehook);
            sim.now - start
        };
        let first = rehook(&mut sim);
        let second = rehook(&mut sim);
        let third = rehook(&mut sim);
        assert!(first < second && second < third, "{first} < {second} < {third}");
        // Diesmal hilft es: Die Hooks sehen wieder Eingaben.
        sim.seen();
        sim.poll();
        assert_eq!(rehook(&mut sim), first);
    }

    #[test]
    fn counters_and_timestamps_may_wrap() {
        let mut sim = Sim::new();
        sim.input = u32::MAX - 3;
        sim.calls = Calls { mouse: u32::MAX, keyboard: u32::MAX };
        sim.poll();
        for _ in 0..100 {
            sim.seen();
            assert_eq!(sim.poll(), Step::Wait);
        }
        assert_eq!(sim.until_step(10_000, Sim::unseen).unwrap().0, Step::Probe);
        sim.answer();
        assert_eq!(sim.poll(), Step::Wait, "über den Überlauf hinweg beantwortet");
    }
}
