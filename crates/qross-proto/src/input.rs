use serde::{Deserialize, Serialize};

/// Physische Taste als USB-HID-Usage (Keyboard/Keypad Page 0x07).
///
/// Bewusst plattformneutral und layoutunabhängig: Übertragen wird die Position der Taste,
/// das Tastaturlayout des Clients bestimmt das Zeichen (wie bei einer echten USB-Tastatur).
///
/// Medientasten wie Wiedergabe/Pause liegen auf der Consumer Page (0x0C) und werden als
/// `CONSUMER | Usage` kodiert. Eine ältere Gegenstelle kennt sie nicht und ignoriert sie.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCode(pub u16);

impl KeyCode {
    /// Kennzeichen für Usages der HID-Consumer-Page
    pub const CONSUMER: u16 = 0xC000;

    pub const A: Self = Self(0x04);
    pub const ENTER: Self = Self(0x28);
    pub const ESCAPE: Self = Self(0x29);
    pub const CAPS_LOCK: Self = Self(0x39);
    pub const SCROLL_LOCK: Self = Self(0x47);
    /// Num-Taste; auf Mac-Tastaturen „Clear“
    pub const NUM_LOCK: Self = Self(0x53);
    pub const MUTE: Self = Self(0x7F);
    pub const VOLUME_UP: Self = Self(0x80);
    pub const VOLUME_DOWN: Self = Self(0x81);
    pub const LEFT_CTRL: Self = Self(0xE0);
    pub const LEFT_SHIFT: Self = Self(0xE1);
    pub const LEFT_ALT: Self = Self(0xE2);
    pub const RIGHT_ALT: Self = Self(0xE6);
    pub const MEDIA_NEXT: Self = Self(Self::CONSUMER | 0xB5);
    pub const MEDIA_PREVIOUS: Self = Self(Self::CONSUMER | 0xB6);
    pub const MEDIA_STOP: Self = Self(Self::CONSUMER | 0xB7);
    pub const MEDIA_PLAY_PAUSE: Self = Self(Self::CONSUMER | 0xCD);

    /// Lautstärke- und Wiedergabetasten
    pub fn is_media(self) -> bool {
        matches!(self, Self::MUTE | Self::VOLUME_UP | Self::VOLUME_DOWN) || self.0 & 0xF000 == Self::CONSUMER
    }

    /// Strg, Umschalt, Alt bzw. Option, Windows bzw. Cmd (links und rechts)
    pub fn is_modifier(self) -> bool {
        (0xE0..=0xE7).contains(&self.0)
    }
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
