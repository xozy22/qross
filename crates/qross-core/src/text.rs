//! Texte, die mehrere Teile von Qross gleich zeigen: die Oberfläche und die Drag-Vorschau am Cursor

/// Dezimale Einheiten wie bei der Größengrenze (1 GB = 1000 MB), deutsch mit Komma:
/// „512 B“, „3,1 MB“, „1 GB“
pub fn size_text(bytes: u64) -> String {
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
    let number = rounded.strip_suffix(".0").unwrap_or(&rounded).replace('.', ",");
    format!("{number} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_german() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(3_141_592), "3,1 MB");
        assert_eq!(size_text(999_960), "1 MB");
        assert_eq!(size_text(1_000_000_000), "1 GB");
        assert_eq!(size_text(2_500_000_000), "2,5 GB");
    }
}
