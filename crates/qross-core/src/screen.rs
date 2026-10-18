//! Bildschirmfläche eines Rechners: umschließendes Rechteck und, falls bekannt, die einzelnen
//! Monitore. Der Cursor bewegt sich nur auf Monitorflächen; gewechselt wird nur an Kantenstücken,
//! an denen ein Monitor an der äußeren Kante liegt. Ohne Monitorliste gilt das Rechteck als ein
//! einziger Monitor (Verhalten bis Qross 0.3).

use qross_proto::{Edge, Rect, MAX_MONITORS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Screen {
    /// Gemeldetes umschließendes Rechteck (`Hello`/`ScreenChanged` bzw. lokale Abfrage)
    bounds: Rect,
    /// Gemeldete Monitore; leer, solange keine Liste bekannt ist
    reported: Vec<Rect>,
    /// Monitore, auf `bounds` zugeschnitten; nie leer (ohne Liste: `bounds` selbst)
    monitors: Vec<Rect>,
    /// Umschließendes Rechteck von `monitors`: an seinem Rand wird gewechselt
    hull: Rect,
}

impl Screen {
    pub fn new(bounds: Rect) -> Self {
        let mut screen = Self { bounds, reported: Vec::new(), monitors: Vec::new(), hull: bounds };
        screen.set_bounds(bounds);
        screen
    }

    pub fn set_bounds(&mut self, bounds: Rect) {
        // Eine leere Fläche (fehlerhafte Meldung) wird zu einem Pixel, damit das Klemmen definiert bleibt.
        self.bounds = Rect { width: bounds.width.max(1), height: bounds.height.max(1), ..bounds };
        self.refresh();
    }

    pub fn set_monitors(&mut self, monitors: Vec<Rect>) {
        self.reported = monitors;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.monitors =
            self.reported.iter().take(MAX_MONITORS).filter_map(|m| intersection(*m, self.bounds)).collect();
        if self.monitors.is_empty() {
            self.monitors.push(self.bounds);
        }
        self.hull = hull(&self.monitors);
    }

    /// Hat der Rechner eine Monitorliste gemeldet?
    pub fn knows_monitors(&self) -> bool {
        !self.reported.is_empty()
    }

    /// Umschließendes Rechteck der Monitore (ohne Liste: die gemeldete Fläche)
    pub fn hull(&self) -> Rect {
        self.hull
    }

    /// Monitor, auf dem (x, y) liegt, sonst der nächstgelegene
    pub fn monitor_at(&self, x: i32, y: i32) -> Rect {
        // `min_by_key` liefert bei Gleichstand den ersten, also bei Überlappung einen enthaltenden.
        self.monitors.iter().copied().min_by_key(|m| distance(*m, x, y)).unwrap_or(self.bounds)
    }

    /// Nächstgelegener Punkt auf einem Monitor
    pub fn snap(&self, x: i32, y: i32) -> (i32, i32) {
        self.monitor_at(x, y).clamp(x, y)
    }

    /// Ein Punkt auf einem Monitor nahe der Mitte (z. B. zum Zurückholen des Cursors)
    pub fn center(&self) -> (i32, i32) {
        let (x, y) = self.hull.center();
        self.snap(x, y)
    }

    /// Bewegung von `from` nach `to`: Liegt das Ziel auf einem Monitor, gilt es (auch beim Übergang
    /// zwischen sich berührenden Monitoren). Sonst bleibt der Cursor am Rand des Monitors stehen,
    /// auf dem er war.
    pub fn step(&self, from: (i32, i32), to: (i32, i32)) -> (i32, i32) {
        if self.monitors.iter().any(|m| m.contains(to.0, to.1)) {
            return to;
        }
        self.monitor_at(from.0, from.1).clamp(to.0, to.1)
    }

    /// Liegt (x, y) an der äußeren Kante `edge` auf einem Monitor, der bis dorthin reicht?
    pub fn on_outer_edge(&self, edge: Edge, x: i32, y: i32) -> bool {
        let h = self.hull;
        let on_line = match edge {
            Edge::Left => x <= h.x,
            Edge::Right => x >= h.right(),
            Edge::Top => y <= h.y,
            Edge::Bottom => y >= h.bottom(),
        };
        on_line && self.monitors.iter().any(|m| m.contains(x, y))
    }

    /// Eintritt über die Kante `edge` dieses Bildschirms: (x, y) auf den nächstgelegenen Monitor
    /// legen, der an dieser Kante liegt.
    pub fn entry_point(&self, edge: Edge, x: i32, y: i32) -> (i32, i32) {
        let h = self.hull;
        self.monitors
            .iter()
            .filter(|m| match edge {
                Edge::Left => m.x == h.x,
                Edge::Right => m.right() == h.right(),
                Edge::Top => m.y == h.y,
                Edge::Bottom => m.bottom() == h.bottom(),
            })
            .map(|m| m.clamp(x, y))
            .min_by_key(|&(cx, cy)| squared(cx, cy, x, y))
            .unwrap_or_else(|| self.snap(x, y))
    }
}

/// Liegt (x, y) jenseits der Kante `edge` von `rect`?
pub(crate) fn beyond(rect: Rect, edge: Edge, x: i32, y: i32) -> bool {
    match edge {
        Edge::Left => x < rect.x,
        Edge::Right => x > rect.right(),
        Edge::Top => y < rect.y,
        Edge::Bottom => y > rect.bottom(),
    }
}

/// Quadrat des Abstands von (x, y) zum nächsten Punkt von `rect` (0, wenn darin)
fn distance(rect: Rect, x: i32, y: i32) -> i64 {
    let (cx, cy) = rect.clamp(x, y);
    squared(cx, cy, x, y)
}

fn squared(ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
    let (dx, dy) = (i64::from(ax) - i64::from(bx), i64::from(ay) - i64::from(by));
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let (left, top) = (a.x.max(b.x), a.y.max(b.y));
    let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let bottom = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
    let (width, height) = (right - i64::from(left), bottom - i64::from(top));
    (width > 0 && height > 0).then(|| Rect::new(left, top, width as u32, height as u32))
}

/// Umschließendes Rechteck (mindestens ein Eintrag)
fn hull(rects: &[Rect]) -> Rect {
    let left = rects.iter().map(|r| r.x).min().unwrap_or(0);
    let top = rects.iter().map(|r| r.y).min().unwrap_or(0);
    let right = rects.iter().map(|r| i64::from(r.x) + i64::from(r.width)).max().unwrap_or(1);
    let bottom = rects.iter().map(|r| i64::from(r.y) + i64::from(r.height)).max().unwrap_or(1);
    Rect::new(left, top, (right - i64::from(left)) as u32, (bottom - i64::from(top)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MacBook links (niedriger, unten bündig), externer Monitor rechts, höher und weiter oben
    const LAPTOP: Rect = Rect::new(0, 418, 1512, 982);
    const EXTERNAL: Rect = Rect::new(1512, -400, 3744, 1800);
    const BOUNDS: Rect = Rect::new(0, -400, 5256, 1800);

    fn mac() -> Screen {
        let mut screen = Screen::new(BOUNDS);
        screen.set_monitors(vec![LAPTOP, EXTERNAL]);
        screen
    }

    #[test]
    fn without_monitor_list_the_bounds_are_one_monitor() {
        let screen = Screen::new(BOUNDS);
        assert!(!screen.knows_monitors());
        assert_eq!(screen.hull(), BOUNDS);
        assert_eq!(screen.step((10, 0), (-50, -9999)), (0, -400));
        assert!(screen.on_outer_edge(Edge::Left, 0, 0));
        assert_eq!(screen.entry_point(Edge::Left, 1, -300), (1, -300));
    }

    #[test]
    fn cursor_stays_on_monitor_areas() {
        let s = mac();
        // Über dem MacBook ist keine Fläche: der Cursor bleibt an dessen Oberkante.
        assert_eq!(s.step((100, 500), (100, 0)), (100, 418));
        // Seitlich in die tote Fläche über dem MacBook: Rand des externen Monitors
        assert_eq!(s.step((1600, 0), (1400, 0)), (1512, 0));
        // Übergang zwischen den sich berührenden Monitoren ist frei
        assert_eq!(s.step((1500, 1000), (1530, 990)), (1530, 990));
        assert_eq!(s.step((1530, 990), (1490, 1000)), (1490, 1000));
    }

    #[test]
    fn only_monitors_at_the_outer_edge_count() {
        let s = mac();
        // Oben liegt nur der externe Monitor am Rand
        assert!(s.on_outer_edge(Edge::Top, 3000, -400));
        assert!(!s.on_outer_edge(Edge::Top, 100, -400), "tote Fläche");
        assert!(!s.on_outer_edge(Edge::Top, 100, 418), "Oberkante des MacBooks ist keine Außenkante");
        // Links nur das MacBook, unten beide
        assert!(s.on_outer_edge(Edge::Left, 0, 900));
        assert!(!s.on_outer_edge(Edge::Left, 0, 0));
        assert!(s.on_outer_edge(Edge::Bottom, 100, 1399) && s.on_outer_edge(Edge::Bottom, 4000, 1399));
    }

    #[test]
    fn entry_goes_to_the_nearest_monitor_at_the_edge() {
        let s = mac();
        // Von links oben hereinkommend: auf das MacBook, an dessen Oberkante
        assert_eq!(s.entry_point(Edge::Left, 1, -300), (1, 418));
        assert_eq!(s.entry_point(Edge::Left, 1, 900), (1, 900));
        // Von oben über dem MacBook: auf den externen Monitor
        assert_eq!(s.entry_point(Edge::Top, 700, -399), (1512, -399));
        // Von rechts: nur der externe Monitor
        assert_eq!(s.entry_point(Edge::Right, 5254, 1000), (5254, 1000));
    }

    #[test]
    fn monitors_are_clipped_to_the_bounds() {
        let mut s = Screen::new(Rect::new(0, 0, 1920, 1080));
        s.set_monitors(vec![Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 1280, 1024)]);
        assert_eq!(s.hull(), Rect::new(0, 0, 1920, 1080), "zweiter Monitor liegt (noch) außerhalb");
        s.set_bounds(Rect::new(0, 0, 3200, 1080));
        assert_eq!(s.hull(), Rect::new(0, 0, 3200, 1080));
        assert_eq!(s.snap(2500, 1070), (2500, 1023));
        s.set_monitors(vec![Rect::new(5000, 0, 10, 10)]);
        assert_eq!(s.hull(), Rect::new(0, 0, 3200, 1080), "nichts Brauchbares: wieder das Rechteck");
        assert_eq!(Screen::new(Rect::new(5, 5, 0, 0)).snap(0, 0), (5, 5), "leere Meldung");
    }
}
