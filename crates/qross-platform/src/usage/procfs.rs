//! Dateien unter `/proc` lesen (Linux), ohne Systemaufrufe und deshalb überall getestet

/// Aus `/proc/<pid>/stat`: Elternprozess, Rechenzeit und Start, beides in Takten des Systems
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    pub parent: u32,
    /// Benutzer- und Systemanteil zusammen
    pub cpu_ticks: u64,
    /// Seit dem Systemstart
    pub started: u64,
}

/// Der Name in Klammern darf selbst Leerzeichen und Klammern enthalten; die Felder danach zählen ab der
/// letzten schließenden Klammer (Feld 3 ist der Zustand).
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let field = |n: usize| fields.get(n.checked_sub(3)?)?.parse::<u64>().ok();
    // 4: Elternprozess, 14 und 15: Rechenzeit, 22: Start
    Some(Stat { parent: u32::try_from(field(4)?).ok()?, cpu_ticks: field(14)?.saturating_add(field(15)?), started: field(22)? })
}

/// PSS in Bytes aus `/proc/<pid>/smaps_rollup` (die Zeile „Pss:“ in kB)
pub(crate) fn pss(text: &str) -> Option<u64> {
    let kb: u64 = text.lines().find_map(|line| line.strip_prefix("Pss:"))?.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kb * 1024)
}

/// Residenter Teil in Seiten aus `/proc/<pid>/statm` (zweites Feld)
pub(crate) fn resident_pages(text: &str) -> Option<u64> {
    text.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_count_after_the_name() {
        // Ein Name mit Leerzeichen und Klammern, wie ihn ein Programm sich selbst geben kann
        let line = "4242 (Web Content (1)) S 4100 4242 4100 0 -1 4194560 9001 0 12 0 1500 250 0 0 20 0 31 0 \
                    987654 2147483648 30000 18446744073709551615 0 0 0 0 0 4096 0 0 0 0 17 3 0 0 0 0 0\n";
        assert_eq!(parse_stat(line), Some(Stat { parent: 4100, cpu_ticks: 1750, started: 987_654 }));
        assert_eq!(parse_stat("4242 (kaputt) S 1"), None, "zu wenige Felder");
        assert_eq!(parse_stat("ohne Klammern"), None);
    }

    #[test]
    fn memory_comes_from_pss_or_resident_pages() {
        let rollup = "55d4c6a3e000-7ffd8f5fe000 ---p 00000000 00:00 0                          [rollup]\n\
                      Rss:              123456 kB\n\
                      Pss:               98765 kB\n\
                      Pss_Anon:          80000 kB\n\
                      Pss_File:          18765 kB\n";
        assert_eq!(pss(rollup), Some(98_765 * 1024));
        assert_eq!(pss("Rss: 1 kB\n"), None);
        assert_eq!(resident_pages("251234 30875 7171 1 0 40012 0\n"), Some(30_875));
    }
}
