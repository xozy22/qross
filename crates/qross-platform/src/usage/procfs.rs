//! Dateien unter `/proc` lesen und auswerten (Linux), ohne Systemaufrufe und deshalb überall getestet

use std::collections::HashMap;

use qross_proto::UsageKind;

use super::kinds;

/// Aus `/proc/<pid>/stat`: Name, Elternprozess, Rechenzeit und Start, beides in Takten des Systems
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stat {
    /// Name des Programms, nach 15 Zeichen abgeschnitten
    pub name: String,
    pub parent: u32,
    /// Benutzer- und Systemanteil zusammen
    pub cpu_ticks: u64,
    /// Seit dem Systemstart
    pub started: u64,
}

/// Der Name in Klammern darf selbst Leerzeichen und Klammern enthalten; die Felder danach zählen ab der
/// letzten schließenden Klammer (Feld 3 ist der Zustand).
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let (open, close) = (text.find('(')?, text.rfind(')')?);
    let name = text.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let field = |n: usize| fields.get(n.checked_sub(3)?)?.parse::<u64>().ok();
    // 4: Elternprozess, 14 und 15: Rechenzeit, 22: Start
    Some(Stat { name, parent: u32::try_from(field(4)?).ok()?, cpu_ticks: field(14)?.saturating_add(field(15)?), started: field(22)? })
}

/// Die Prozesse von Qross unter allen des Systems (`all`, nach Nummer): `own` selbst und darunter die von WebKitGTK,
/// auch hinter bubblewrap (`bwrap`, das selbst nicht zählt). Andere Nachfahren (etwa `xdg-open` und ein Browser
/// für einen Link) zählen nicht mit, auch nicht, was unter ihnen läuft. Ein Kind muss nach seinem Elternprozess
/// gestartet sein; sonst trägt es nur eine neu vergebene Nummer.
pub(crate) fn qross_processes(all: &HashMap<u32, Stat>, own: u32) -> Vec<(u32, UsageKind)> {
    let Some(root) = all.get(&own) else { return Vec::new() };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, stat) in all {
        if pid != stat.parent {
            children.entry(stat.parent).or_default().push(pid);
        }
    }
    let mut found = vec![(own, UsageKind::Core)];
    let mut queue = vec![(own, root.started)];
    while let Some((parent, started)) = queue.pop() {
        for &pid in children.get(&parent).into_iter().flatten() {
            let stat = &all[&pid];
            if stat.started < started {
                continue;
            }
            let kind = kinds::webkitgtk_kind(&stat.name);
            if kind.is_some() || stat.name == "bwrap" {
                queue.push((pid, stat.started));
            }
            found.extend(kind.map(|kind| (pid, kind)));
        }
    }
    found
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
        let stat = Stat { name: "Web Content (1)".into(), parent: 4100, cpu_ticks: 1750, started: 987_654 };
        assert_eq!(parse_stat(line), Some(stat));
        assert_eq!(parse_stat("4242 (kaputt) S 1"), None, "too few fields");
        assert_eq!(parse_stat("ohne Klammern"), None);
    }

    #[test]
    fn qross_is_itself_and_webkitgtk_below_it() {
        let stat = |name: &str, parent, started| Stat { name: name.into(), parent, cpu_ticks: 0, started };
        let all: HashMap<u32, Stat> = [
            (1, stat("systemd", 0, 0)),
            (100, stat("qross-gui", 1, 500)),
            // Ohne Sandbox direkt unter Qross
            (101, stat("WebKitNetworkPr", 100, 510)),
            (102, stat("WebKitWebProces", 100, 520)),
            // Mit Sandbox hinter bubblewrap
            (103, stat("bwrap", 100, 530)),
            (104, stat("bwrap", 103, 531)),
            (105, stat("WebKitGPUProces", 104, 532)),
            // Ein Link im Browser: weder xdg-open noch der Browser noch dessen WebKit zählen
            (110, stat("xdg-open", 100, 600)),
            (111, stat("epiphany", 110, 601)),
            (112, stat("WebKitWebProces", 111, 602)),
            // Ein anderes Programm, dessen Nummer einmal einem Kind von Qross gehörte
            (120, stat("WebKitWebProces", 100, 400)),
            // Fremd
            (200, stat("WebKitWebProces", 1, 700)),
        ]
        .into_iter()
        .collect();
        let mut found = qross_processes(&all, 100);
        found.sort_unstable();
        let expected =
            [(100, UsageKind::Core), (101, UsageKind::Network), (102, UsageKind::Page), (105, UsageKind::Graphics)];
        assert_eq!(found, expected);
        assert!(qross_processes(&all, 999).is_empty(), "unknown own process");
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
