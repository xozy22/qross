//! Reine Logik für Wayland (Desktop-Portale und libei): Auswahl der Sitzung, Zeigerbarrieren an den
//! Monitorkanten, Maustasten und Mausrad. Ohne Systemaufrufe, damit sie auf jeder Plattform getestet
//! werden kann; benutzt wird sie unter Linux.

use qross_proto::{Edge, MouseButton, Rect};

// ------------------------------------------------------------------ Sitzung

/// Grafische Sitzung unter Linux
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxSession {
    X11,
    Wayland,
}

/// Welche Sitzung läuft? `QROSS_BACKEND=x11|wayland` erzwingt eine; sonst gilt Wayland, sobald
/// `WAYLAND_DISPLAY` gesetzt ist (oder `XDG_SESSION_TYPE=wayland`), auch wenn daneben XWayland
/// ein `DISPLAY` anbietet.
pub fn linux_session(
    forced: Option<&str>,
    wayland_display: Option<&str>,
    session_type: Option<&str>,
) -> LinuxSession {
    match forced.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("x11") => return LinuxSession::X11,
        Some("wayland") => return LinuxSession::Wayland,
        _ => {}
    }
    let set = |v: Option<&str>| v.is_some_and(|v| !v.trim().is_empty());
    if set(wayland_display) || session_type.is_some_and(|t| t.eq_ignore_ascii_case("wayland")) {
        LinuxSession::Wayland
    } else {
        LinuxSession::X11
    }
}

// ------------------------------------------------------------------ Barrieren

/// Eine Zeigerbarriere des InputCapture-Portals an einer äußeren Monitorkante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Barrier {
    /// Kennung für das Portal (ab 1)
    pub id: u32,
    pub edge: Edge,
    /// Monitor, an dessen Kante die Barriere liegt
    pub monitor: Rect,
    /// Linie im Format des Portals (x1, y1, x2, y2), Endpunkte eingeschlossen. Links und oben liegt
    /// sie auf der ersten Pixelreihe des Monitors, rechts und unten direkt hinter der letzten.
    pub line: (i32, i32, i32, i32),
}

/// Umschließendes Rechteck; `None` ohne Monitore.
pub fn hull(monitors: &[Rect]) -> Option<Rect> {
    let left = monitors.iter().map(|m| m.x).min()?;
    let top = monitors.iter().map(|m| m.y).min()?;
    let right = monitors.iter().map(|m| m.right()).max()?;
    let bottom = monitors.iter().map(|m| m.bottom()).max()?;
    Some(Rect::new(left, top, (right - left + 1) as u32, (bottom - top + 1) as u32))
}

/// Nächstgelegener Punkt auf einem der Monitore; ohne Monitore (x, y) selbst.
pub fn snap(monitors: &[Rect], x: i32, y: i32) -> (i32, i32) {
    monitors
        .iter()
        .map(|m| m.clamp(x, y))
        .min_by_key(|&(cx, cy)| {
            let (dx, dy) = (i64::from(cx - x), i64::from(cy - y));
            dx * dx + dy * dy
        })
        .unwrap_or((x, y))
}

/// Ein Punkt auf einem Monitor nahe der Mitte der Gesamtfläche (Notausgang)
pub fn center(monitors: &[Rect]) -> (i32, i32) {
    match hull(monitors) {
        Some(h) => {
            let (x, y) = h.center();
            snap(monitors, x, y)
        }
        None => (0, 0),
    }
}

/// Barrieren an allen Monitorkanten, die auf der äußeren Kante der Gesamtfläche liegen – genau dort,
/// wo der Server wechselt (`Screen::on_outer_edge`). Zwischen zwei Monitoren liegt nie eine
/// Barriere, das lehnt das Portal ab. Je Monitor und Seite eine Barriere, nicht über mehrere
/// Monitore hinweg zusammengefasst.
pub fn barriers(monitors: &[Rect]) -> Vec<Barrier> {
    let Some(h) = hull(monitors) else { return Vec::new() };
    let mut out: Vec<Barrier> = Vec::new();
    for m in monitors.iter().filter(|m| m.width > 0 && m.height > 0) {
        for edge in Edge::ALL {
            let line = match edge {
                Edge::Left if m.x == h.x => (m.x, m.y, m.x, m.bottom()),
                Edge::Right if m.right() == h.right() => (m.right() + 1, m.y, m.right() + 1, m.bottom()),
                Edge::Top if m.y == h.y => (m.x, m.y, m.right(), m.y),
                Edge::Bottom if m.bottom() == h.bottom() => (m.x, m.bottom() + 1, m.right(), m.bottom() + 1),
                _ => continue,
            };
            if out.iter().any(|b| b.line == line) {
                continue; // gespiegelte Monitore
            }
            out.push(Barrier { id: out.len() as u32 + 1, edge, monitor: *m, line });
        }
    }
    out
}

/// Die Barriere, an der der Cursor bei (x, y) steht (falls das Portal keine Kennung mitschickt).
pub fn barrier_near(barriers: &[Barrier], x: i32, y: i32) -> Option<&Barrier> {
    barriers.iter().min_by_key(|b| {
        let (x1, y1, x2, y2) = b.line;
        let (cx, cy) = (x.clamp(x1.min(x2), x1.max(x2)), y.clamp(y1.min(y2), y1.max(y2)));
        let (dx, dy) = (i64::from(cx - x), i64::from(cy - y));
        dx * dx + dy * dy
    })
}

/// Die Cursorposition beim Auslösen einer Barriere auf den Monitor dahinter legen: Rechts und
/// unten meldet das Portal die Position auf der Barrierenlinie, also ein Pixel außerhalb.
pub fn position_on(barrier: &Barrier, x: f64, y: f64) -> (i32, i32) {
    let (x, y) = (x.floor() as i32, y.floor() as i32);
    barrier.monitor.clamp(x, y)
}

/// Zeigt eine relative Bewegung weiter in die Kante hinein?
pub fn pushes_into(edge: Edge, dx: f64, dy: f64) -> bool {
    match edge {
        Edge::Left => dx < 0.0,
        Edge::Right => dx > 0.0,
        Edge::Top => dy < 0.0,
        Edge::Bottom => dy > 0.0,
    }
}

// ------------------------------------------------------------------ Maustasten

/// evdev-Codes der Maustasten (`BTN_*` aus `input-event-codes.h`), die libei verwendet
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
pub const BTN_SIDE: u32 = 0x113;
pub const BTN_EXTRA: u32 = 0x114;
/// Manche Geräte melden „Zurück“/„Vor“ als `BTN_BACK`/`BTN_FORWARD`
pub const BTN_FORWARD: u32 = 0x115;
pub const BTN_BACK: u32 = 0x116;

pub fn evdev_button(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::Back => BTN_SIDE,
        MouseButton::Forward => BTN_EXTRA,
    }
}

pub fn button_from_evdev(code: u32) -> Option<MouseButton> {
    match code {
        BTN_LEFT => Some(MouseButton::Left),
        BTN_RIGHT => Some(MouseButton::Right),
        BTN_MIDDLE => Some(MouseButton::Middle),
        BTN_SIDE | BTN_BACK => Some(MouseButton::Back),
        BTN_EXTRA | BTN_FORWARD => Some(MouseButton::Forward),
        _ => None,
    }
}

// ------------------------------------------------------------------ Mausrad

/// libei zählt Rasten wie Windows in 120stel, aber mit umgekehrtem Vorzeichen für die senkrechte
/// Achse: Qross (und Windows) melden „nach oben“ positiv, libei „nach unten“.
pub fn ei_discrete(dx: i16, dy: i16) -> (i32, i32) {
    (i32::from(dx), -i32::from(dy))
}

/// Gegenrichtung von [`ei_discrete`]: libei-Rasten (120stel) in das Qross-Rad.
pub fn wheel_from_ei_discrete(dx: i32, dy: i32) -> (i16, i16) {
    let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
    (clamp(dx), clamp(dy.saturating_neg()))
}

/// Wie viele Einheiten des stufenlosen Scrollens (libinput: etwa Pixel) einer Raste entsprechen
pub const SMOOTH_PER_NOTCH: f64 = 15.0;

/// Stufenloses Scrollen (Touchpad) in 120stel-Rasten umrechnen; Reste werden aufgehoben.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SmoothWheel {
    rest: (f64, f64),
}

impl SmoothWheel {
    /// libei-Werte (nach unten positiv) → Qross-Rad (nach oben positiv), `None` ohne ganze Einheit
    pub fn add(&mut self, dx: f64, dy: f64) -> Option<(i16, i16)> {
        let scale = 120.0 / SMOOTH_PER_NOTCH;
        self.rest.0 += dx * scale;
        self.rest.1 -= dy * scale;
        let take = |v: &mut f64| {
            let whole = v.trunc();
            *v -= whole;
            whole.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
        };
        let out = (take(&mut self.rest.0), take(&mut self.rest.1));
        (out != (0, 0)).then_some(out)
    }

    pub fn reset(&mut self) {
        self.rest = (0.0, 0.0);
    }
}

/// Relative Bewegung als ganze Pixel; die Nachkommastellen wandern in die nächste Bewegung.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    rest: (f64, f64),
}

impl Motion {
    pub fn add(&mut self, dx: f64, dy: f64) -> Option<(i32, i32)> {
        self.rest.0 += dx;
        self.rest.1 += dy;
        let take = |v: &mut f64| {
            let whole = v.trunc();
            *v -= whole;
            whole as i32
        };
        let out = (take(&mut self.rest.0), take(&mut self.rest.1));
        (out != (0, 0)).then_some(out)
    }

    pub fn reset(&mut self) {
        self.rest = (0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_from_environment() {
        use LinuxSession::*;
        assert_eq!(linux_session(None, None, None), X11);
        assert_eq!(linux_session(None, Some("wayland-0"), None), Wayland);
        assert_eq!(linux_session(None, Some(""), None), X11);
        assert_eq!(linux_session(None, None, Some("wayland")), Wayland);
        assert_eq!(linux_session(None, None, Some("x11")), X11);
        assert_eq!(linux_session(Some("X11"), Some("wayland-0"), Some("wayland")), X11);
        assert_eq!(linux_session(Some("wayland"), None, None), Wayland);
        assert_eq!(linux_session(Some("unsinn"), Some("wayland-0"), None), Wayland);
    }

    #[test]
    fn one_monitor_gets_four_barriers() {
        let m = Rect::new(0, 0, 1920, 1080);
        let b = barriers(&[m]);
        let lines: Vec<_> = b.iter().map(|b| (b.edge, b.line)).collect();
        assert_eq!(
            lines,
            [
                (Edge::Left, (0, 0, 0, 1079)),
                (Edge::Right, (1920, 0, 1920, 1079)),
                (Edge::Top, (0, 0, 1919, 0)),
                (Edge::Bottom, (0, 1080, 1919, 1080)),
            ]
        );
        assert_eq!(b.iter().map(|b| b.id).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn no_barrier_between_monitors() {
        // Links 1920×1080, rechts höher 2560×1440, oben bündig
        let left = Rect::new(0, 0, 1920, 1080);
        let right = Rect::new(1920, 0, 2560, 1440);
        let b = barriers(&[left, right]);
        assert!(b.iter().all(|b| b.line.0 != 1920 || b.edge == Edge::Top || b.edge == Edge::Bottom), "{b:?}");
        let edges = |m: Rect| b.iter().filter(|b| b.monitor == m).map(|b| b.edge).collect::<Vec<_>>();
        // Der linke Monitor reicht nicht bis zur unteren Kante der Gesamtfläche
        assert_eq!(edges(left), [Edge::Left, Edge::Top]);
        assert_eq!(edges(right), [Edge::Right, Edge::Top, Edge::Bottom]);
        assert_eq!(b.iter().find(|b| b.edge == Edge::Bottom).unwrap().line, (1920, 1440, 4479, 1440));
    }

    #[test]
    fn mirrored_monitors_share_barriers() {
        let m = Rect::new(0, 0, 1920, 1080);
        assert_eq!(barriers(&[m, m]).len(), 4);
        assert!(barriers(&[]).is_empty());
    }

    #[test]
    fn negative_offsets() {
        // Monitor links vom Hauptmonitor (GNOME/KDE erlauben negative Koordinaten nicht immer,
        // die Zonen können aber so kommen)
        let b = barriers(&[Rect::new(-1280, 0, 1280, 1024), Rect::new(0, 0, 1920, 1080)]);
        assert_eq!(b.iter().find(|b| b.edge == Edge::Left).unwrap().line, (-1280, 0, -1280, 1023));
        assert_eq!(b.iter().find(|b| b.edge == Edge::Right).unwrap().line, (1920, 0, 1920, 1079));
    }

    #[test]
    fn snapping_and_center() {
        let left = Rect::new(0, 0, 1920, 1080);
        let right = Rect::new(1920, 0, 2560, 1440);
        // Lücke unter dem linken Monitor: auf den nächsten Monitor
        assert_eq!(snap(&[left, right], 100, 1300), (100, 1079));
        assert_eq!(snap(&[left, right], 3000, 1300), (3000, 1300));
        assert_eq!(snap(&[], 5, 6), (5, 6));
        assert_eq!(center(&[left, right]), (2240, 720));
        assert_eq!(center(&[]), (0, 0));
    }

    #[test]
    fn finds_the_barrier_at_the_cursor() {
        let b = barriers(&[Rect::new(0, 0, 1920, 1080)]);
        assert_eq!(barrier_near(&b, 1920, 500).unwrap().edge, Edge::Right);
        assert_eq!(barrier_near(&b, 0, 500).unwrap().edge, Edge::Left);
        assert_eq!(barrier_near(&b, 700, 1080).unwrap().edge, Edge::Bottom);
        assert_eq!(barrier_near(&b, 700, 0).unwrap().edge, Edge::Top);
        assert!(barrier_near(&[], 0, 0).is_none());
    }

    #[test]
    fn activation_position_lands_on_the_monitor() {
        let b = barriers(&[Rect::new(0, 0, 1920, 1080)]);
        let right = b.iter().find(|b| b.edge == Edge::Right).unwrap();
        assert_eq!(position_on(right, 1920.0, 500.7), (1919, 500));
        let bottom = b.iter().find(|b| b.edge == Edge::Bottom).unwrap();
        assert_eq!(position_on(bottom, 10.2, 1080.0), (10, 1079));
    }

    #[test]
    fn push_direction() {
        assert!(pushes_into(Edge::Right, 2.0, -5.0));
        assert!(!pushes_into(Edge::Right, -0.5, 0.0));
        assert!(pushes_into(Edge::Left, -1.0, 0.0));
        assert!(pushes_into(Edge::Top, 0.0, -1.0));
        assert!(pushes_into(Edge::Bottom, 0.0, 3.0));
        assert!(!pushes_into(Edge::Bottom, 4.0, 0.0));
    }

    #[test]
    fn buttons_roundtrip() {
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward] {
            assert_eq!(button_from_evdev(evdev_button(button)), Some(button));
        }
        assert_eq!(button_from_evdev(BTN_BACK), Some(MouseButton::Back));
        assert_eq!(button_from_evdev(BTN_FORWARD), Some(MouseButton::Forward));
        assert_eq!(button_from_evdev(0x120), None);
    }

    #[test]
    fn wheel_signs() {
        // Eine Raste nach oben: Qross +120, libei −120
        assert_eq!(ei_discrete(0, 120), (0, -120));
        assert_eq!(ei_discrete(-120, 0), (-120, 0));
        assert_eq!(wheel_from_ei_discrete(0, -120), (0, 120));
        assert_eq!(wheel_from_ei_discrete(240, 0), (240, 0));
        assert_eq!(wheel_from_ei_discrete(0, i32::MIN), (0, i16::MAX));
    }

    #[test]
    fn smooth_scrolling_accumulates() {
        let mut wheel = SmoothWheel::default();
        // Eine halbe Raste nach unten (7,5 Pixel): −60 Einheiten
        assert_eq!(wheel.add(0.0, 7.5), Some((0, -60)));
        assert_eq!(wheel.add(0.0, 0.05), None);
        assert_eq!(wheel.add(0.0, 0.1), Some((0, -1)));
        wheel.reset();
        assert_eq!(wheel.add(-15.0, 0.0), Some((-120, 0)));
    }

    #[test]
    fn motion_keeps_fractions() {
        let mut motion = Motion::default();
        assert_eq!(motion.add(0.4, -0.4), None);
        assert_eq!(motion.add(0.7, -0.7), Some((1, -1)));
        assert_eq!(motion.add(0.0, 0.0), None);
        assert_eq!(motion.add(2.95, 0.0), Some((3, 0)));
    }
}
