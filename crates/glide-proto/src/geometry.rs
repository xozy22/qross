use serde::{Deserialize, Serialize};

/// Bildschirmkante
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    pub fn opposite(self) -> Self {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Rechteck in Pixeln; `right()`/`bottom()` sind inklusiv.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32 - 1
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32 - 1
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width as i32 / 2, self.y + self.height as i32 / 2)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    pub fn clamp(&self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(self.x, self.right()), y.clamp(self.y, self.bottom()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_edges_are_inclusive() {
        let r = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(r.right(), -1);
        assert_eq!(r.bottom(), 1079);
        assert!(r.contains(-1, 1079));
        assert!(!r.contains(0, 0));
        assert_eq!(r.clamp(10, -5), (-1, 0));
    }
}
