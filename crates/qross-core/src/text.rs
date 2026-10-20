//! Texte, die mehrere Teile von Qross gleich zeigen: die Oberfläche und die Drag-Vorschau am Cursor

use crate::lang::Lang;

/// Dezimale Einheiten wie bei der Größengrenze (1 GB = 1000 MB), englisch mit Punkt fürs Log:
/// „512 B“, „3.1 MB“, „1 GB“
pub fn size_text(bytes: u64) -> String {
    size_text_in(bytes, Lang::En)
}

/// Wie [`size_text`] für die Oberfläche: deutsch mit Komma („3,1 MB“), englisch mit Punkt („3.1 MB“)
pub fn size_text_in(bytes: u64, lang: Lang) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let rounded = format!("{value:.1}");
    let number = rounded.strip_suffix(".0").unwrap_or(&rounded);
    let number = match lang {
        Lang::De => number.replace('.', ","),
        Lang::En => number.to_owned(),
    };
    format!("{number} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_german() {
        let size_text = |bytes| size_text_in(bytes, Lang::De);
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(3_141_592), "3,1 MB");
        assert_eq!(size_text(999_960), "1 MB");
        assert_eq!(size_text(1_000_000_000), "1 GB");
        assert_eq!(size_text(2_500_000_000), "2,5 GB");
    }

    #[test]
    fn sizes_are_english_without_a_language() {
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(3_141_592), "3.1 MB");
        assert_eq!(size_text(999_960), "1 MB");
        assert_eq!(size_text(2_500_000_000), "2.5 GB");
        assert_eq!(size_text_in(268_435_456, Lang::En), "268.4 MB");
    }
}
