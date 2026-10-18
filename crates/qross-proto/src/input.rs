use serde::{Deserialize, Serialize};

/// Physische Taste als USB-HID-Usage (Keyboard/Keypad Page 0x07).
///
/// Bewusst plattformneutral und layoutunabhängig: Übertragen wird die Position der Taste,
/// das Tastaturlayout des Clients bestimmt das Zeichen (wie bei einer echten USB-Tastatur).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCode(pub u16);

impl KeyCode {
    pub const A: Self = Self(0x04);
    pub const ENTER: Self = Self(0x28);
    pub const ESCAPE: Self = Self(0x29);
    pub const LEFT_CTRL: Self = Self(0xE0);
    pub const LEFT_SHIFT: Self = Self(0xE1);
    pub const LEFT_ALT: Self = Self(0xE2);
    pub const RIGHT_ALT: Self = Self(0xE6);
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// Seitentaste „Zurück“ (X1)
    Back,
    /// Seitentaste „Vor“ (X2)
    Forward,
}
