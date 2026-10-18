//! Zeichenmodus: Statt der physischen Taste kommt beim Client das Zeichen an, das sie auf dem
//! Tastaturlayout des Servers ergibt. Mit einem deutschen Server und einem US-Mac stimmen so Y und Z,
//! Umlaute, @, €, Klammern usw.
//!
//! Die Erfassung des Servers liest dafür sein Layout und meldet je Tastendruck [`KeyChars`].
//! [`Typist`] macht daraus, was der Client bekommt: Text, Kürzel wie Strg+Z nach dem Zeichen (damit
//! es dort „Rückgängig“ bleibt) und alles andere als Taste wie bisher.
//!
//! - Tote Tasten (^ ´ `) verbindet [`Typist`] selbst mit dem nächsten Zeichen ([`compose`]). Die
//!   Erfassung darf den Zustand des Systems dafür nicht verändern; unter Windows gingen tote Tasten
//!   sonst in Programmen kaputt.
//! - Zusatztasten hält [`Typist`] zurück, bis sie gebraucht werden: für ein Kürzel, eine andere Taste
//!   oder einen Klick. Für Text gehen sie nie zum Client: AltGr bzw. Option wären dort womöglich Alt
//!   und machten aus „@“ ein Tastenkürzel. Allein gedrückt (z. B. die Windows-Taste fürs Startmenü)
//!   kommen sie beim Loslassen an.

use qross_proto::{Input, KeyCode};

/// Was eine Taste auf dem Tastaturlayout des Servers gerade ergibt
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Produces {
    /// Diese Zeichen, meist genau eines (nie Steuerzeichen)
    Text(String),
    /// Tote Taste: Das Akzentzeichen (z. B. „^“) verbindet sich mit dem nächsten Zeichen.
    Dead(char),
    /// Nichts, z. B. eine im Layout unbelegte Taste
    Nothing,
}

/// Was die Erfassung des Servers je Tastendruck über sein Layout weiß
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChars {
    /// Mit den gedrückten Umschalt-, AltGr- bzw. Option-Tasten und der Feststelltaste
    pub produces: Produces,
    /// Das Zeichen der Taste ohne Zusatztasten, für Kürzel wie Strg+Z
    pub base: Option<char>,
    /// Ein Kürzel statt Text: Strg, Windows bzw. Cmd ist gedrückt, oder Alt ohne AltGr (am Mac
    /// tippt Option dagegen Zeichen). Was das ist, weiß nur die Erfassung der jeweiligen Plattform.
    pub shortcut: bool,
}

impl KeyChars {
    /// Die Taste ergibt `text`, ohne Zusatztasten `base`
    pub fn text(text: impl Into<String>, base: char) -> Self {
        Self { produces: Produces::Text(text.into()), base: Some(base), shortcut: false }
    }
}

impl Produces {
    /// Was ein Layout für eine Taste liefert: Text nur, wenn er nicht leer ist und keine
    /// Steuerzeichen enthält (Strg+A ergibt unter Windows z. B. U+0001).
    pub fn from_text(text: String) -> Self {
        if text.is_empty() || text.chars().any(char::is_control) {
            Self::Nothing
        } else {
            Self::Text(text)
        }
    }

    /// Das erste Zeichen bzw. der Akzent der toten Taste
    pub fn first(&self) -> Option<char> {
        match self {
            Self::Text(text) => text.chars().next(),
            Self::Dead(accent) => Some(*accent),
            Self::Nothing => None,
        }
    }
}

const CAPS_LOCK: KeyCode = KeyCode(0x39);

/// Tasten, die beim Tippen Zeichen ergeben: Buchstaben, Ziffern der oberen Reihe, Satzzeichen,
/// Leertaste, die ISO-Taste „<>“ und die japanischen Zeichentasten. Alles andere (Pfeile, F-Tasten,
/// Eingabe, Rücktaste, Tab, Esc, Entf, Pos1/Ende, Medientasten, der Ziffernblock) bleibt eine Taste.
pub fn is_typing_key(key: KeyCode) -> bool {
    matches!(key.0, 0x04..=0x27 | 0x2C..=0x38 | 0x64 | 0x87 | 0x89)
}

/// Akzente toter Tasten
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accent {
    Grave,
    Acute,
    Circumflex,
    Tilde,
    Diaeresis,
    Ring,
    Cedilla,
    Caron,
    Macron,
    Breve,
    Ogonek,
    DoubleAcute,
    DotAbove,
}

/// Akzent eines Zeichens, wie es die Layouts für tote Tasten melden: meist die alleinstehende Form
/// (^ ´ ` ~ ¨), manche das kombinierende Zeichen, US-International ' und " für ´ und ¨.
fn accent(c: char) -> Option<(Accent, char)> {
    use Accent::*;
    // (Akzent, alleinstehende Form)
    Some(match c {
        '`' | '\u{300}' | 'ˋ' => (Grave, '`'),
        '´' | '\u{301}' | 'ˊ' => (Acute, '´'),
        '\'' => (Acute, '\''),
        '^' | '\u{302}' | 'ˆ' => (Circumflex, '^'),
        '~' | '\u{303}' => (Tilde, '~'),
        '˜' => (Tilde, '˜'),
        '¨' | '\u{308}' => (Diaeresis, '¨'),
        '"' => (Diaeresis, '"'),
        '°' | '\u{30A}' => (Ring, '°'),
        '˚' => (Ring, '˚'),
        '¸' | '\u{327}' => (Cedilla, '¸'),
        'ˇ' | '\u{30C}' => (Caron, 'ˇ'),
        '¯' | '\u{304}' | 'ˉ' => (Macron, '¯'),
        '˘' | '\u{306}' => (Breve, '˘'),
        '˛' | '\u{328}' => (Ogonek, '˛'),
        '˝' | '\u{30B}' => (DoubleAcute, '˝'),
        '˙' | '\u{307}' => (DotAbove, '˙'),
        _ => return None,
    })
}

/// Grundbuchstabe → Buchstabe mit Akzent, je Akzent als zwei gleich lange Zeichenfolgen
#[rustfmt::skip]
const COMPOSED: &[(Accent, &str, &str)] = &[
    (Accent::Grave, "aeiouAEIOUnNwWyY", "àèìòùÀÈÌÒÙǹǸẁẀỳỲ"),
    (Accent::Acute, "aeiouyAEIOUYcCnNsSzZlLrRgGwW", "áéíóúýÁÉÍÓÚÝćĆńŃśŚźŹĺĹŕŔǵǴẃẂ"),
    (Accent::Circumflex, "aeiouAEIOUcCgGhHjJsSwWyYzZ", "âêîôûÂÊÎÔÛĉĈĝĜĥĤĵĴŝŜŵŴŷŶẑẐ"),
    (Accent::Tilde, "aonAONiIuUeEyY", "ãõñÃÕÑĩĨũŨẽẼỹỸ"),
    (Accent::Diaeresis, "aeiouyAEIOUYwWxXhHt", "äëïöüÿÄËÏÖÜŸẅẄẍẌḧḦẗ"),
    (Accent::Ring, "aAuUwy", "åÅůŮẘẙ"),
    (Accent::Cedilla, "cCsStTgGkKlLnNrReE", "çÇşŞţŢģĢķĶļĻņŅŗŖȩȨ"),
    (Accent::Caron, "cCdDeEnNrRsStTzZlLaAiIoOuUgGkKj", "čČďĎěĚňŇřŘšŠťŤžŽľĽǎǍǐǏǒǑǔǓǧǦǩǨǰ"),
    (Accent::Macron, "aAeEiIoOuUyYgG", "āĀēĒīĪōŌūŪȳȲḡḠ"),
    (Accent::Breve, "aAeEgGiIoOuU", "ăĂĕĔğĞĭĬŏŎŭŬ"),
    (Accent::Ogonek, "aAeEiIuUoO", "ąĄęĘįĮųŲǫǪ"),
    (Accent::DoubleAcute, "oOuU", "őŐűŰ"),
    (Accent::DotAbove, "cCeEgGIzZ", "ċĊėĖġĠİżŻ"),
];

/// Verbindet den Akzent einer toten Taste mit dem folgenden Buchstaben, z. B. „^“ und „e“ zu „ê“.
/// `None`, wenn es die Verbindung nicht gibt (dann erscheinen beide Zeichen einzeln).
pub fn compose(dead: char, base: char) -> Option<char> {
    let (accent, _) = accent(dead)?;
    let (_, bases, composed) = COMPOSED.iter().find(|(a, ..)| *a == accent)?;
    let i = bases.chars().position(|c| c == base)?;
    composed.chars().nth(i)
}

/// Was nach einer toten Taste herauskommt: der Buchstabe mit Akzent, nach der Leertaste der Akzent
/// allein, sonst beides einzeln (wie unter Windows und macOS).
fn after_dead(dead: char, text: &str) -> String {
    let spacing = accent(dead).map_or(dead, |(_, spacing)| spacing);
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(' '), None) => spacing.to_string(),
        (Some(c), None) => compose(dead, c).map_or_else(|| format!("{spacing}{c}"), String::from),
        _ => format!("{spacing}{text}"),
    }
}

/// Wie eine gedrückte Taste des Servers beim Client ankam
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Zusatztaste (bzw. auf eine umbelegte), noch nicht gesendet. `used`: Seitdem kam Text.
    Waiting { to: KeyCode, used: bool },
    /// Als Taste `to` gesendet
    Sent(KeyCode),
    /// Als `Input::Text` oder `Input::Shortcut` gesendet; das Loslassen geht als `Key`
    Typed,
    /// Tote Taste, die auf das nächste Zeichen wartet
    Dead,
    /// Verschluckt: die Feststelltaste, die nur das Layout des Servers umschaltet
    Swallowed,
}

/// Zeichenmodus für den Client mit dem Cursor: macht aus den Tastendrücken des Servers, was der
/// Client bekommt. Jeder Besuch beginnt mit einem neuen `Typist`; beim `Leave` lässt der Client
/// ohnehin alles los.
#[derive(Debug, Default)]
pub struct Typist {
    /// Akzent einer toten Taste, der auf das nächste Zeichen wartet
    dead: Option<char>,
    /// Gedrückte Tasten des Servers in der Reihenfolge des Drückens
    held: Vec<(KeyCode, Held)>,
}

impl Typist {
    pub fn new() -> Self {
        Self::default()
    }

    /// Taste `key` gedrückt (auch die Wiederholung, wenn sie gehalten wird). `remap`: die Taste, die
    /// dieser Client laut Umbelegung stattdessen bekommt; eine umbelegte Taste bleibt immer eine
    /// Taste. `chars`: was sie auf dem Server ergibt (`None`: keine Zeichentaste oder unbekannt).
    pub fn press(&mut self, key: KeyCode, remap: Option<KeyCode>, chars: Option<&KeyChars>) -> Vec<Input> {
        let to = remap.unwrap_or(key);
        if let Some(i) = self.held.iter().position(|(k, _)| *k == key) {
            match self.held[i].1 {
                // Wiederholung einer Zusatztaste bzw. der Feststelltaste
                Held::Waiting { .. } | Held::Swallowed => return Vec::new(),
                Held::Sent(to) => return vec![Input::Key { key: to, down: true }],
                // Text, Kürzel und tote Tasten wiederholen sich wie neu gedrückt
                Held::Typed | Held::Dead => {
                    self.held.remove(i);
                }
            }
        }
        if to.is_modifier() {
            self.held.push((key, Held::Waiting { to, used: false }));
            return Vec::new();
        }
        if remap.is_none() && key == CAPS_LOCK {
            self.held.push((key, Held::Swallowed));
            return Vec::new();
        }
        let chars = chars.filter(|_| remap.is_none());
        if let Some(chars) = chars {
            if !chars.shortcut && !self.remapped_shortcut_modifier() {
                match &chars.produces {
                    Produces::Text(text) => {
                        let text = match self.dead.take() {
                            Some(dead) => after_dead(dead, text),
                            None => text.clone(),
                        };
                        return self.typed(key, Input::Text { key, text });
                    }
                    Produces::Dead(accent) => match self.dead.take() {
                        // Zwei tote Tasten nacheinander: beide Akzente einzeln
                        Some(first) => {
                            let text = after_dead(first, &after_dead(*accent, " "));
                            return self.typed(key, Input::Text { key, text });
                        }
                        None => {
                            self.dead = Some(*accent);
                            self.mark_used();
                            self.held.push((key, Held::Dead));
                            return Vec::new();
                        }
                    },
                    Produces::Nothing => {}
                }
            } else if let Some(ch) = chars.base {
                self.dead = None;
                let mut out = self.flush();
                self.held.push((key, Held::Typed));
                out.push(Input::Shortcut { key, ch });
                return out;
            }
        }
        // Alles andere geht als Taste; eine wartende tote Taste verfällt.
        self.dead = None;
        let mut out = self.flush();
        self.held.push((key, Held::Sent(to)));
        out.push(Input::Key { key: to, down: true });
        out
    }

    /// Taste `key` losgelassen. `to`: die Taste laut Umbelegung, für Tasten, die schon vor diesem
    /// Besuch gedrückt waren.
    pub fn release(&mut self, key: KeyCode, to: KeyCode) -> Vec<Input> {
        let Some(i) = self.held.iter().position(|(k, _)| *k == key) else {
            return vec![Input::Key { key: to, down: false }];
        };
        match self.held.remove(i).1 {
            // Allein gedrückt, z. B. die Windows-Taste fürs Startmenü
            Held::Waiting { to, used: false } => vec![Input::Key { key: to, down: true }, Input::Key { key: to, down: false }],
            Held::Waiting { used: true, .. } | Held::Dead | Held::Swallowed => Vec::new(),
            Held::Sent(to) => vec![Input::Key { key: to, down: false }],
            Held::Typed => vec![Input::Key { key, down: false }],
        }
    }

    /// Zurückgehaltene Zusatztasten senden – vor Klicks und Raddrehungen (Umschalt+Klick,
    /// Strg+Rad) und vor Tasten, die keine Zeichen sind.
    pub fn flush(&mut self) -> Vec<Input> {
        let mut out = Vec::new();
        for (_, held) in &mut self.held {
            if let Held::Waiting { to, .. } = *held {
                *held = Held::Sent(to);
                out.push(Input::Key { key: to, down: true });
            }
        }
        out
    }

    fn typed(&mut self, key: KeyCode, input: Input) -> Vec<Input> {
        self.mark_used();
        self.held.push((key, Held::Typed));
        vec![input]
    }

    /// Text kam: Die gehaltenen Zusatztasten gehörten dazu und gehen beim Loslassen nicht mehr raus.
    fn mark_used(&mut self) {
        for (_, held) in &mut self.held {
            if let Held::Waiting { used, .. } = held {
                *used = true;
            }
        }
    }

    /// Ist eine Taste gedrückt, die für diesen Client auf Strg bzw. Windows/Cmd umbelegt ist (z. B.
    /// die Feststelltaste als Strg)? Dann ist es ein Kürzel, auch wenn der Server Text sieht.
    fn remapped_shortcut_modifier(&self) -> bool {
        self.held.iter().any(|&(key, held)| match held {
            Held::Waiting { to, .. } | Held::Sent(to) => to != key && matches!(to.0, 0xE0 | 0xE3 | 0xE4 | 0xE7),
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIFT: KeyCode = KeyCode(0xE1);
    const CTRL: KeyCode = KeyCode(0xE0);
    const ALTGR: KeyCode = KeyCode(0xE6);
    const META: KeyCode = KeyCode(0xE3);
    const KEY_Q: KeyCode = KeyCode(0x14);
    const KEY_E: KeyCode = KeyCode(0x08);
    const KEY_X: KeyCode = KeyCode(0x1B);
    /// Auf deutschen Tastaturen die Taste mit dem Z (US: Y)
    const KEY_Y: KeyCode = KeyCode(0x1C);
    const SPACE: KeyCode = KeyCode(0x2C);
    /// Deutsch: ^ (tote Taste) links neben der 1
    const BACKQUOTE: KeyCode = KeyCode(0x35);
    /// Deutsch: ´ (tote Taste) links neben der Rücktaste
    const EQUAL: KeyCode = KeyCode(0x2E);
    const ARROW_LEFT: KeyCode = KeyCode(0x50);

    fn key(key: KeyCode, down: bool) -> Input {
        Input::Key { key, down }
    }

    fn text(key: KeyCode, text: &str) -> Input {
        Input::Text { key, text: text.into() }
    }

    fn dead(accent: char) -> KeyChars {
        KeyChars { produces: Produces::Dead(accent), base: Some(accent), shortcut: false }
    }

    fn shortcut(base: char) -> KeyChars {
        KeyChars { produces: Produces::Nothing, base: Some(base), shortcut: true }
    }

    #[test]
    fn typing_keys_are_characters_and_the_rest_stays_keys() {
        for k in [0x04, 0x1D, 0x1E, 0x27, 0x2C, 0x2D, 0x31, 0x32, 0x35, 0x38, 0x64] {
            assert!(is_typing_key(KeyCode(k)), "{k:#x}");
        }
        // Eingabe, Esc, Rücktaste, Tab, Feststelltaste, F1, Entf, Pfeile, Ziffernblock, Strg, Medien
        for k in [0x28, 0x29, 0x2A, 0x2B, 0x39, 0x3A, 0x4C, 0x4A, 0x4D, 0x50, 0x59, 0x63, 0xE0, 0x7F] {
            assert!(!is_typing_key(KeyCode(k)), "{k:#x}");
        }
        assert!(!is_typing_key(KeyCode::MEDIA_PLAY_PAUSE));
    }

    #[test]
    fn control_characters_are_no_text() {
        assert_eq!(Produces::from_text("ä".into()), Produces::Text("ä".into()));
        assert_eq!(Produces::from_text("\u{1}".into()), Produces::Nothing, "Strg+A");
        assert_eq!(Produces::from_text("\r".into()), Produces::Nothing);
        assert_eq!(Produces::from_text(String::new()), Produces::Nothing);
        assert_eq!(Produces::Dead('^').first(), Some('^'));
        assert_eq!(Produces::Text("ab".into()).first(), Some('a'));
        assert_eq!(Produces::Nothing.first(), None);
    }

    #[test]
    fn dead_keys_combine_with_the_next_letter() {
        assert_eq!(compose('^', 'e'), Some('ê'));
        assert_eq!(compose('´', 'E'), Some('É'));
        assert_eq!(compose('`', 'a'), Some('à'));
        assert_eq!(compose('~', 'n'), Some('ñ'));
        assert_eq!(compose('¨', 'u'), Some('ü'));
        assert_eq!(compose('˜', 'o'), Some('õ'), "macOS: Option+N");
        assert_eq!(compose('\'', 'c'), Some('ć'), "US-International");
        assert_eq!(compose('\u{301}', 'a'), Some('á'), "kombinierendes Zeichen");
        assert_eq!(compose('ˇ', 'z'), Some('ž'));
        assert_eq!(compose('˛', 'a'), Some('ą'));
        assert_eq!(compose('˝', 'o'), Some('ő'));
        assert_eq!(compose('°', 'a'), Some('å'));
        assert_eq!(compose('^', 'x'), None);
        assert_eq!(compose('x', 'e'), None, "kein Akzent");
        for (_, bases, composed) in COMPOSED {
            assert_eq!(bases.chars().count(), composed.chars().count(), "{bases}");
        }
        assert_eq!(after_dead('^', " "), "^");
        assert_eq!(after_dead('\u{302}', "x"), "^x", "alleinstehende Form");
        assert_eq!(after_dead('´', "ab"), "´ab");
    }

    #[test]
    fn letters_arrive_as_text_without_their_modifiers() {
        let mut t = Typist::new();
        // Deutsches Layout: Die Taste, die in den USA Y heißt, ergibt Z
        assert_eq!(t.press(KEY_Y, None, Some(&KeyChars::text("z", 'z'))), vec![text(KEY_Y, "z")]);
        assert_eq!(t.release(KEY_Y, KEY_Y), vec![key(KEY_Y, false)]);
        // Umschalt geht für Text nicht zum Client und beim Loslassen auch nicht
        assert!(t.press(SHIFT, None, None).is_empty());
        assert_eq!(t.press(KEY_Y, None, Some(&KeyChars::text("Z", 'z'))), vec![text(KEY_Y, "Z")]);
        assert_eq!(t.press(KEY_Y, None, Some(&KeyChars::text("Z", 'z'))), vec![text(KEY_Y, "Z")], "Wiederholung");
        assert_eq!(t.release(KEY_Y, KEY_Y), vec![key(KEY_Y, false)]);
        assert!(t.release(SHIFT, SHIFT).is_empty());
    }

    #[test]
    fn altgr_characters_are_text_and_altgr_never_reaches_the_client() {
        let mut t = Typist::new();
        assert!(t.press(ALTGR, None, None).is_empty());
        assert_eq!(t.press(KEY_Q, None, Some(&KeyChars::text("@", 'q'))), vec![text(KEY_Q, "@")]);
        assert_eq!(t.press(KEY_E, None, Some(&KeyChars::text("€", 'e'))), vec![text(KEY_E, "€")]);
        assert_eq!(t.release(KEY_Q, KEY_Q), vec![key(KEY_Q, false)]);
        assert_eq!(t.release(KEY_E, KEY_E), vec![key(KEY_E, false)]);
        assert!(t.release(ALTGR, ALTGR).is_empty(), "sonst wäre es z. B. am Mac ein Option-Druck");
    }

    #[test]
    fn dead_keys_wait_for_the_next_character() {
        let mut t = Typist::new();
        assert!(t.press(BACKQUOTE, None, Some(&dead('^'))).is_empty());
        assert!(t.release(BACKQUOTE, BACKQUOTE).is_empty());
        assert_eq!(t.press(KEY_E, None, Some(&KeyChars::text("e", 'e'))), vec![text(KEY_E, "ê")]);
        t.release(KEY_E, KEY_E);

        // Mit Umschalt dazwischen: ´ Umschalt+E → É
        t.press(EQUAL, None, Some(&dead('´')));
        t.press(SHIFT, None, None);
        assert_eq!(t.press(KEY_E, None, Some(&KeyChars::text("E", 'e'))), vec![text(KEY_E, "É")]);
        t.release(KEY_E, KEY_E);
        assert!(t.release(SHIFT, SHIFT).is_empty());

        // Ohne passenden Buchstaben beide einzeln, mit der Leertaste der Akzent allein
        t.press(BACKQUOTE, None, Some(&dead('^')));
        assert_eq!(t.press(KEY_X, None, Some(&KeyChars::text("x", 'x'))), vec![text(KEY_X, "^x")]);
        t.press(BACKQUOTE, None, Some(&dead('^')));
        assert_eq!(t.press(SPACE, None, Some(&KeyChars::text(" ", ' '))), vec![text(SPACE, "^")]);
        // Zweimal die tote Taste: zwei Akzente
        t.press(EQUAL, None, Some(&dead('´')));
        t.release(EQUAL, EQUAL);
        assert_eq!(t.press(EQUAL, None, Some(&dead('´'))), vec![text(EQUAL, "´´")]);
        assert_eq!(t.release(EQUAL, EQUAL), vec![key(EQUAL, false)]);
        // Eine andere Taste lässt sie verfallen
        t.press(BACKQUOTE, None, Some(&dead('^')));
        assert_eq!(t.press(ARROW_LEFT, None, None), vec![key(ARROW_LEFT, true)]);
        assert_eq!(t.press(KEY_E, None, Some(&KeyChars::text("e", 'e'))), vec![text(KEY_E, "e")]);
    }

    #[test]
    fn shortcuts_go_by_the_character_with_their_modifiers() {
        let mut t = Typist::new();
        // Strg+Z auf deutscher Tastatur: gemeint ist „z“, nicht die Position
        assert!(t.press(CTRL, None, None).is_empty());
        assert_eq!(
            t.press(KEY_Y, None, Some(&shortcut('z'))),
            vec![key(CTRL, true), Input::Shortcut { key: KEY_Y, ch: 'z' }]
        );
        assert_eq!(t.press(KEY_Y, None, Some(&shortcut('z'))), vec![Input::Shortcut { key: KEY_Y, ch: 'z' }], "Wiederholung");
        assert_eq!(t.release(KEY_Y, KEY_Y), vec![key(KEY_Y, false)]);
        assert_eq!(t.release(CTRL, CTRL), vec![key(CTRL, false)]);

        // Ohne bekanntes Zeichen die Taste selbst
        t.press(CTRL, None, None);
        let unknown = KeyChars { produces: Produces::Nothing, base: None, shortcut: true };
        assert_eq!(t.press(KEY_Q, None, Some(&unknown)), vec![key(CTRL, true), key(KEY_Q, true)]);
        assert_eq!(t.release(KEY_Q, KEY_Q), vec![key(KEY_Q, false)]);
    }

    #[test]
    fn modifiers_alone_and_before_other_keys_arrive() {
        let mut t = Typist::new();
        // Windows-Taste allein: beim Loslassen drücken und loslassen (Startmenü)
        assert!(t.press(META, None, None).is_empty());
        assert_eq!(t.release(META, META), vec![key(META, true), key(META, false)]);
        // Umschalt+Pfeil markiert: Umschalt geht vor dem Pfeil raus
        t.press(SHIFT, None, None);
        assert_eq!(t.press(ARROW_LEFT, None, None), vec![key(SHIFT, true), key(ARROW_LEFT, true)]);
        assert_eq!(t.press(SHIFT, None, None), vec![key(SHIFT, true)], "Wiederholung der gesendeten Taste");
        assert_eq!(t.release(ARROW_LEFT, ARROW_LEFT), vec![key(ARROW_LEFT, false)]);
        assert_eq!(t.release(SHIFT, SHIFT), vec![key(SHIFT, false)]);
        // Umschalt+Klick: vor dem Klick senden
        t.press(SHIFT, None, None);
        assert_eq!(t.flush(), vec![key(SHIFT, true)]);
        assert!(t.flush().is_empty());
        assert_eq!(t.release(SHIFT, SHIFT), vec![key(SHIFT, false)]);
        // Vor dem Besuch gedrückt: Loslassen geht durch
        assert_eq!(t.release(CTRL, META), vec![key(META, false)]);
    }

    #[test]
    fn remapped_keys_stay_keys() {
        const CAPS: KeyCode = KeyCode(0x39);
        let mut t = Typist::new();
        // Umbelegt: immer als Taste, Zeichen hin oder her
        assert_eq!(t.press(KEY_Y, Some(KEY_X), Some(&KeyChars::text("z", 'z'))), vec![key(KEY_X, true)]);
        assert_eq!(t.release(KEY_Y, KEY_X), vec![key(KEY_X, false)]);
        // Feststelltaste als Strg: wird wie Strg zurückgehalten und macht aus Text ein Kürzel
        assert!(t.press(CAPS, Some(CTRL), None).is_empty());
        assert_eq!(
            t.press(KEY_Q, None, Some(&KeyChars::text("q", 'q'))),
            vec![key(CTRL, true), Input::Shortcut { key: KEY_Q, ch: 'q' }]
        );
        t.release(KEY_Q, KEY_Q);
        assert_eq!(t.release(CAPS, CTRL), vec![key(CTRL, false)]);
        // Die Feststelltaste selbst schaltet nur das Layout des Servers um
        assert!(t.press(CAPS, None, None).is_empty());
        assert!(t.press(CAPS, None, None).is_empty());
        assert!(t.release(CAPS, CAPS).is_empty());
        // Unbekannte Tasten gehen als Taste
        assert_eq!(t.press(KEY_Q, None, None), vec![key(KEY_Q, true)]);
        let nothing = KeyChars { produces: Produces::Nothing, base: None, shortcut: false };
        assert_eq!(t.press(KEY_E, None, Some(&nothing)), vec![key(KEY_E, true)]);
    }

    #[test]
    fn a_used_modifier_is_still_sent_before_a_later_key() {
        let mut t = Typist::new();
        t.press(SHIFT, None, None);
        assert_eq!(t.press(KEY_Q, None, Some(&KeyChars::text("Q", 'q'))), vec![text(KEY_Q, "Q")]);
        t.release(KEY_Q, KEY_Q);
        // Umschalt+Pfeil nach Text: Umschalt muss doch noch raus
        assert_eq!(t.press(ARROW_LEFT, None, None), vec![key(SHIFT, true), key(ARROW_LEFT, true)]);
        t.release(ARROW_LEFT, ARROW_LEFT);
        assert_eq!(t.release(SHIFT, SHIFT), vec![key(SHIFT, false)]);
    }
}
