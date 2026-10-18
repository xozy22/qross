//! Neuigkeiten: `CHANGELOG.md` wird beim Bauen eingebettet und hier in Versionen zerlegt.
//!
//! Erwartetes Format (Keep a Changelog, siehe Kopf der Datei):
//! - `## [0.2.1] - 2026-10-18` beginnt eine Version, die neueste steht oben
//! - ein Absatz direkt darunter ist die Einleitung
//! - `### Neu`, `### Verbessert`, … sind Rubriken (unbekannte werden durchgereicht)
//! - `- Text` ist ein Eintrag, mit zwei Leerzeichen eingerückte Folgezeilen gehören dazu
//!
//! Text vor der ersten Version, `## [Unreleased]` und Linkreferenzen (`[0.2.1]: https://…`)
//! werden übersprungen. Der Parser bricht nie ab: Unlesbares fällt einfach weg.

use std::sync::OnceLock;

use serde::Serialize;

/// Der Changelog des Workspaces, zur Bauzeit eingebettet.
pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// ISO-Datum wie `2026-10-18`
    pub date: Option<String>,
    /// Absatz direkt unter der Versionsüberschrift; mehrere Absätze durch Leerzeile getrennt
    pub intro: Option<String>,
    pub sections: Vec<Section>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Rubrik wie in der Datei, z. B. „Neu“; leer, wenn Einträge ohne Rubrik direkt unter der Version stehen
    pub kind: String,
    pub items: Vec<String>,
}

/// Die eingebetteten Versionen, neueste zuerst (einmal zerlegt).
pub fn releases() -> &'static [Release] {
    static PARSED: OnceLock<Vec<Release>> = OnceLock::new();
    PARSED.get_or_init(|| parse(CHANGELOG))
}

pub fn parse(text: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    // Vor der ersten Version und unter `[Unreleased]` wird nichts übernommen.
    let mut skipping = true;
    let mut item: Option<String> = None;
    let mut after_blank = false;

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(heading) = line.strip_prefix("## ") {
            finish_item(&mut releases, &mut item);
            match parse_heading(heading) {
                Some(release) => {
                    releases.push(release);
                    skipping = false;
                }
                None => skipping = true,
            }
            after_blank = false;
            continue;
        }
        if skipping {
            continue;
        }
        let Some(release) = releases.last_mut() else { continue };

        if line.trim().is_empty() {
            finish_item(&mut releases, &mut item);
            after_blank = true;
            continue;
        }
        let blank_before = std::mem::replace(&mut after_blank, false);

        if let Some(kind) = line.strip_prefix("### ") {
            finish_item(&mut releases, &mut item);
            if let Some(release) = releases.last_mut() {
                release.sections.push(Section { kind: kind.trim().to_owned(), items: Vec::new() });
            }
        } else if is_link_reference(line) {
            finish_item(&mut releases, &mut item);
        } else if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            finish_item(&mut releases, &mut item);
            item = Some(text.trim().to_owned());
        } else if let (true, Some(current)) = (raw.starts_with("  "), item.as_mut()) {
            current.push(' ');
            current.push_str(line.trim());
        } else if release.sections.is_empty() {
            // Fließtext vor der ersten Rubrik: Einleitung
            let intro = release.intro.get_or_insert_with(String::new);
            if !intro.is_empty() {
                intro.push_str(if blank_before { "\n\n" } else { " " });
            }
            intro.push_str(line.trim());
        } else {
            // Fließtext in einer Rubrik geht nicht verloren, sondern wird ein eigener Eintrag.
            finish_item(&mut releases, &mut item);
            item = Some(line.trim().to_owned());
        }
    }
    finish_item(&mut releases, &mut item);
    releases
}

fn finish_item(releases: &mut [Release], item: &mut Option<String>) {
    let (Some(text), Some(release)) = (item.take(), releases.last_mut()) else { return };
    if release.sections.is_empty() {
        release.sections.push(Section { kind: String::new(), items: Vec::new() });
    }
    if let Some(section) = release.sections.last_mut() {
        section.items.push(text);
    }
}

/// `[0.2.1] - 2026-10-18`, `0.2.1 - 2026-10-18` oder `[0.2.1]`; `None` für `[Unreleased]` und Unlesbares.
fn parse_heading(heading: &str) -> Option<Release> {
    let heading = heading.trim();
    let (version, rest) = match heading.strip_prefix('[') {
        Some(inner) => {
            let end = inner.find(']')?;
            (&inner[..end], &inner[end + 1..])
        }
        None => heading.split_once(' ').unwrap_or((heading, "")),
    };
    let version = version.trim().trim_start_matches('v');
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let date = rest.trim().trim_start_matches(['-', '–', '—']).trim();
    Some(Release {
        version: version.to_owned(),
        date: (!date.is_empty()).then(|| date.to_owned()),
        intro: None,
        sections: Vec::new(),
    })
}

fn is_link_reference(line: &str) -> bool {
    line.starts_with('[') && line.contains("]: ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_iso_date(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    }

    #[test]
    fn embedded_changelog_matches_this_version() {
        let releases = releases();
        assert!(!releases.is_empty(), "CHANGELOG.md enthält keine Version");
        assert_eq!(releases[0].version, env!("CARGO_PKG_VERSION"), "oberster Eintrag muss die aktuelle Version sein");
        for release in releases {
            let date = release.date.as_deref().unwrap_or_else(|| panic!("{} hat kein Datum", release.version));
            assert!(is_iso_date(date), "{}: Datum {date} ist nicht JJJJ-MM-TT", release.version);
            let items: usize = release.sections.iter().map(|s| s.items.len()).sum();
            assert!(items > 0, "{} hat keine Einträge", release.version);
            for item in release.sections.iter().flat_map(|s| &s.items) {
                assert!(!item.contains("]: http"), "Linkreferenz als Eintrag: {item}");
                assert!(!item.contains("  "), "Folgezeile nicht sauber verbunden: {item}");
            }
        }
    }

    #[test]
    fn versions_are_newest_first_and_unique() {
        let parse_version = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
        let versions: Vec<_> = releases().iter().map(|r| parse_version(&r.version)).collect();
        assert!(versions.windows(2).all(|w| w[0] > w[1]), "Versionen nicht absteigend sortiert: {versions:?}");
    }

    #[test]
    fn full_example_is_parsed() {
        let text = "# Changelog\n\nKopftext, der ignoriert wird.\n\n## [Unreleased]\n\n### Neu\n\n- noch nicht fertig\n\n\
                    ## [1.2.0] - 2026-11-02\n\nEinleitung über\nzwei Zeilen.\n\nZweiter Absatz.\n\n\
                    ### Neu\n\n- Erster Eintrag mit **fett**\n  und Folgezeile\n  über drei Zeilen.\n- Zweiter mit `code`\n\n\
                    ### Sicherheit\n\n- Unbekannte Rubrik bleibt erhalten\n\n\
                    ## 1.1.0 - 2026-10-01\n\n- Eintrag ohne Rubrik\n\n\
                    [1.2.0]: https://example.com/compare\n[1.1.0]: https://example.com/tag\n";
        let releases = parse(text);
        assert_eq!(releases.len(), 2, "[Unreleased] wird übersprungen");

        let r = &releases[0];
        assert_eq!(r.version, "1.2.0");
        assert_eq!(r.date.as_deref(), Some("2026-11-02"));
        assert_eq!(r.intro.as_deref(), Some("Einleitung über zwei Zeilen.\n\nZweiter Absatz."));
        assert_eq!(r.sections[0].kind, "Neu");
        assert_eq!(
            r.sections[0].items,
            ["Erster Eintrag mit **fett** und Folgezeile über drei Zeilen.", "Zweiter mit `code`"]
        );
        assert_eq!(r.sections[1], Section { kind: "Sicherheit".into(), items: vec!["Unbekannte Rubrik bleibt erhalten".into()] });

        let r = &releases[1];
        assert_eq!((r.version.as_str(), r.date.as_deref()), ("1.1.0", Some("2026-10-01")));
        assert_eq!(r.sections, [Section { kind: String::new(), items: vec!["Eintrag ohne Rubrik".into()] }]);
    }

    #[test]
    fn broken_input_never_panics() {
        for text in ["", "## ", "## [", "## [1.0", "### Neu\n- x", "## [x] - y\n  - z", "- a\n  b", "## [1.0.0]\n[1.0.0]: x"] {
            let _ = parse(text);
        }
        assert!(parse("").is_empty());
        assert!(parse("Nur Text\n- und eine Liste").is_empty());
        assert_eq!(parse("## [1.0.0]").len(), 1);
    }
}
