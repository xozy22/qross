//! Reine Logik für Drag & Drop unter macOS und Linux: Hat seit dem Drücken der Maustaste ein neues
//! Ziehen begonnen? Welche Dateien nennt eine `text/uri-list`? Welchen Ordner zeigt ein
//! Finder-Fenster? Ohne Systemaufrufe, damit sie auf jeder Plattform getestet werden kann.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ Laufendes Ziehen

/// Erkennt ein Ziehen, das nach dem Drücken der Maustaste begonnen hat. Grundlage ist ein Zähler,
/// der sich mit jedem neuen Ziehen ändert: unter macOS der `changeCount` der Drag-Zwischenablage,
/// unter X11 die Zahl der Besitzwechsel von `XdndSelection`. Die Daten des letzten Ziehens liegen
/// dort noch, bis das nächste beginnt; ohne den Stand beim Drücken sähen sie aus wie ein laufendes.
#[derive(Debug, Default)]
pub struct DragTracker {
    /// Zählerstand beim Drücken; `None`: keine Taste gedrückt
    pressed: Option<i64>,
    /// Zählerstand, dessen Daten schon gelesen wurden
    taken: Option<i64>,
}

impl DragTracker {
    /// Die Maustaste wurde gedrückt, der Zähler steht auf `count`.
    pub fn press(&mut self, count: i64) {
        self.pressed = Some(count);
    }

    pub fn release(&mut self) {
        self.pressed = None;
    }

    pub fn is_pressed(&self) -> bool {
        self.pressed.is_some()
    }

    /// Steht der Zähler auf `count`: Lohnt es, die gezogenen Dateien zu lesen? Nur bei gedrückter
    /// Taste und einem Ziehen, das seitdem begonnen hat. Danach gilt dieses Ziehen als erledigt:
    /// Ohne Dateien (etwa gezogener Text) wird es nicht bei jeder Bewegung an der Kante erneut
    /// gelesen, und einmal vergebene Dateien gehen nicht ein zweites Mal hinüber, wenn der Cursor
    /// mit gedrückter Taste zurückkommt.
    pub fn take(&mut self, count: i64) -> bool {
        let fresh = self.pressed.is_some_and(|at| at != count) && self.taken != Some(count);
        if fresh {
            self.taken = Some(count);
        }
        fresh
    }
}

// ------------------------------------------------------------------ text/uri-list

/// Dateien einer `text/uri-list` (RFC 2483), wie sie X11-Programme beim Ziehen anbieten: eine URI
/// je Zeile, `#` leitet einen Kommentar ein. Es zählen nur `file:`-URIs dieses Rechners (ohne Host
/// oder mit `localhost`); die Prozentkodierung wird aufgelöst. Andere Einträge fallen weg.
pub fn parse_uri_list(data: &[u8]) -> Vec<PathBuf> {
    // Manche Programme hängen ein Nullbyte an.
    let without_nul = |line: &[u8]| -> usize { line.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1) };
    data.split(|&b| b == b'\n')
        .map(|line| line[..without_nul(line)].trim_ascii())
        .filter(|line| !line.is_empty() && !line.starts_with(b"#"))
        .filter_map(file_uri_path)
        .collect()
}

fn file_uri_path(uri: &[u8]) -> Option<PathBuf> {
    let scheme = uri.get(..5)?;
    if !scheme.eq_ignore_ascii_case(b"file:") {
        return None;
    }
    let rest = &uri[5..];
    let path = match rest.strip_prefix(b"//") {
        Some(authority_and_path) => {
            let slash = authority_and_path.iter().position(|&b| b == b'/')?;
            let host = &authority_and_path[..slash];
            if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
                return None;
            }
            &authority_and_path[slash..]
        }
        // `file:/pfad` ohne Authority
        None if rest.starts_with(b"/") => rest,
        None => return None,
    };
    let bytes = percent_decode(path)?;
    if bytes.contains(&0) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

fn percent_decode(input: &[u8]) -> Option<Vec<u8>> {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let high = hex(*input.get(i + 1)?)?;
            let low = hex(*input.get(i + 2)?)?;
            out.push(high << 4 | low);
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ Finder-Fenster

/// Was ein Finder-Fenster über die Bedienungshilfen (Accessibility) preisgibt. Welchen Ordner es
/// zeigt, sagt der Finder nicht direkt: `AXDocument` bleibt unter macOS 26 leer. Der Titel ist der
/// Name des Ordners, die Pfadleiste und die sichtbaren Einträge tragen Datei-URLs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FinderView {
    /// `AXDocument`, sofern der Finder es setzt
    pub document: Option<PathBuf>,
    /// Fenstertitel: Name des gezeigten Ordners, so wie der Finder ihn anzeigt (lokalisiert)
    pub title: String,
    /// Pfadleiste von außen nach innen: angezeigter Name und Ordner (`None`, wenn kein Dateipfad,
    /// etwa „Netzwerk“). Leer, wenn sie ausgeblendet ist – so ist macOS eingestellt.
    pub crumbs: Vec<(String, Option<PathBuf>)>,
    /// Pfade einiger sichtbarer Einträge, in beliebiger Reihenfolge
    pub items: Vec<PathBuf>,
}

/// Der Ordner, den ein Finder-Fenster zeigt, oder `None`, wenn das nicht eindeutig ist (Zuletzt
/// benutzt, Suche, AirDrop, Programme). Lieber nichts als ein falscher Ordner: Ohne Antwort landen
/// die Dateien in „Downloads/Qross“.
///
/// - Die Pfadleiste folgt der Auswahl: Ist ein Eintrag markiert, ist er ihr letztes Glied. Es zählt
///   deshalb das innerste Glied, das so heißt wie das Fenster.
/// - Ohne Pfadleiste: der Ordner der sichtbaren Einträge, wenn genau einer von ihnen so heißt wie
///   das Fenster. In der Spaltenansicht trifft das die betrachtete Spalte, in einer Suche keinen.
///
/// `names` liefert die Namen, unter denen ein Ordner erscheinen kann (angezeigter Name wie
/// „Schreibtisch“ und letzter Pfadteil), `is_dir`, ob es ihn als Ordner gibt.
pub fn finder_folder(
    view: &FinderView,
    names: impl Fn(&Path) -> Vec<String>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(document) = view.document.as_ref().filter(|d| is_dir(d)) {
        return Some(document.clone());
    }
    let title = comparable(&view.title);
    if title.is_empty() {
        return None;
    }
    let from_crumbs = view.crumbs.iter().rev().find_map(|(name, path)| {
        path.as_ref().filter(|path| comparable(name) == title && is_dir(path))
    });
    if let Some(path) = from_crumbs {
        return Some(path.clone());
    }
    let parents: BTreeSet<&Path> = view.items.iter().filter_map(|item| item.parent()).collect();
    let mut matches = parents.into_iter().filter(|parent| names(parent).iter().any(|n| comparable(n) == title));
    match (matches.next(), matches.next()) {
        (Some(only), None) if is_dir(only) => Some(only.to_path_buf()),
        _ => None,
    }
}

/// Die Pfadleiste schreibt Namen mit geschütztem Leerzeichen („iCloud Drive“), der Titel mit
/// gewöhnlichem.
fn comparable(name: &str) -> String {
    name.replace('\u{a0}', " ").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_counts_only_if_it_began_after_the_press() {
        let mut t = DragTracker::default();
        assert!(!t.take(7), "ohne gedrückte Taste nie");
        t.press(7);
        assert!(!t.take(7), "die Daten liegen noch vom letzten Ziehen");
        assert!(t.take(8), "seit dem Drücken hat ein neues Ziehen begonnen");
        assert!(!t.take(8), "dasselbe Ziehen nur einmal");
        t.release();
        assert!(!t.is_pressed());
        t.press(8);
        assert!(!t.take(8), "neues Drücken ohne neues Ziehen");
        assert!(t.take(9));
    }

    #[test]
    fn coming_back_with_the_button_held_does_not_offer_the_files_again() {
        let mut t = DragTracker::default();
        t.press(1);
        assert!(t.take(2), "Dateien gehen zum Client");
        // Cursor kommt mit gedrückter Taste zurück und erreicht die Kante erneut
        assert!(!t.take(2));
    }

    #[test]
    fn uri_lists_become_paths() {
        let list = b"# kommentar\r\nfile:///home/anna/Bericht%20Gr%C3%BC%C3%9Fe.pdf\r\nfile://localhost/tmp/a\r\n\
                     file:/tmp/ohne-authority\r\nhttps://example.org/x\r\nfile://andererrechner/tmp/x\r\n\
                     file:///tmp/ordner/\r\n\0";
        assert_eq!(
            parse_uri_list(list),
            [
                PathBuf::from("/home/anna/Bericht Grüße.pdf"),
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/ohne-authority"),
                PathBuf::from("/tmp/ordner/"),
            ]
        );
    }

    #[test]
    fn broken_uri_lists_give_nothing() {
        assert!(parse_uri_list(b"").is_empty());
        assert!(parse_uri_list(b"file:///tmp/kaputt%2").is_empty(), "abgebrochene Prozentkodierung");
        assert!(parse_uri_list(b"file:///tmp/%00null").is_empty(), "Nullbyte im Pfad");
        assert!(parse_uri_list(b"file:relativ").is_empty());
        assert_eq!(parse_uri_list(b"FILE:///tmp/x\n\n"), [PathBuf::from("/tmp/x")], "Schema ohne Groß/klein");
    }

    /// Pfadleiste für `/Users/anna/<teile…>`
    fn crumbs(parts: &[&str]) -> Vec<(String, Option<PathBuf>)> {
        let mut path = PathBuf::from("/Users/anna");
        let mut crumbs = vec![
            ("Macintosh HD".to_owned(), Some(PathBuf::from("/"))),
            ("Benutzer".to_owned(), Some(PathBuf::from("/Users"))),
            ("anna".to_owned(), Some(path.clone())),
        ];
        for part in parts {
            path.push(part);
            crumbs.push((part.to_string(), Some(path.clone())));
        }
        crumbs
    }

    fn view(title: &str) -> FinderView {
        FinderView { title: title.into(), ..FinderView::default() }
    }

    /// Ordner sind alle Pfade ohne Punkt im letzten Teil; angezeigt wird der letzte Pfadteil, für
    /// `/Users/anna/Desktop` wie im deutschen Finder „Schreibtisch“.
    fn pick(view: &FinderView) -> Option<PathBuf> {
        let names = |p: &Path| {
            let last = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let shown = if p == Path::new("/Users/anna/Desktop") { "Schreibtisch".to_owned() } else { last.clone() };
            vec![shown, last]
        };
        let is_dir = |p: &Path| p.file_name().is_none_or(|n| !n.to_string_lossy().contains('.'));
        finder_folder(view, names, is_dir)
    }

    #[test]
    fn the_document_wins_when_finder_sets_it() {
        let v = FinderView { document: Some("/Users/anna/Projekt".into()), ..view("egal") };
        assert_eq!(pick(&v), Some("/Users/anna/Projekt".into()));
        let file = FinderView { document: Some("/Users/anna/a.txt".into()), ..view("") };
        assert_eq!(pick(&file), None, "nur Ordner");
    }

    #[test]
    fn the_innermost_crumb_named_like_the_window() {
        let v = FinderView { crumbs: crumbs(&["Projekt", "Bilder"]), ..view("Bilder") };
        assert_eq!(pick(&v), Some("/Users/anna/Projekt/Bilder".into()));
        // Ein markierter Eintrag hängt an der Pfadleiste – eine Datei oder ein Ordner
        let file = FinderView { crumbs: crumbs(&["Projekt", "Bilder", "foto.jpg"]), ..view("Bilder") };
        assert_eq!(pick(&file), Some("/Users/anna/Projekt/Bilder".into()));
        let folder = FinderView { crumbs: crumbs(&["Projekt", "Bilder"]), ..view("Projekt") };
        assert_eq!(pick(&folder), Some("/Users/anna/Projekt".into()));
    }

    #[test]
    fn protected_spaces_in_the_path_bar_match_the_title() {
        let icloud = PathBuf::from("/Users/anna/Library/Mobile Documents/com~apple~CloudDocs");
        let v = FinderView { crumbs: vec![("iCloud\u{a0}Drive".into(), Some(icloud.clone()))], ..view("iCloud Drive") };
        assert_eq!(pick(&v), Some(icloud));
    }

    #[test]
    fn crumbs_without_a_file_path_or_folder_give_nothing() {
        assert_eq!(pick(&FinderView { crumbs: vec![("Netzwerk".into(), None)], ..view("Netzwerk") }), None);
        let gone = FinderView { crumbs: crumbs(&["weg.d"]), ..view("weg.d") };
        assert_eq!(pick(&gone), None, "kein Ordner (mehr)");
    }

    #[test]
    fn without_the_path_bar_the_folder_of_the_items() {
        let items = vec!["/Users/anna/Projekt/a.txt".into(), "/Users/anna/Projekt/b.md".into()];
        assert_eq!(pick(&FinderView { items, ..view("Projekt") }), Some("/Users/anna/Projekt".into()));
        let desktop = vec!["/Users/anna/Desktop/notiz.txt".into()];
        assert_eq!(pick(&FinderView { items: desktop, ..view("Schreibtisch") }), Some("/Users/anna/Desktop".into()));
    }

    #[test]
    fn in_column_view_the_column_named_like_the_window() {
        let items = vec![
            "/Users/anna/Projekt".into(),
            "/Users/anna/Dokumente".into(),
            "/Users/anna/Projekt/Bilder".into(),
            "/Users/anna/Projekt/plan.txt".into(),
            "/Users/anna/Projekt/Bilder/foto.jpg".into(),
        ];
        assert_eq!(pick(&FinderView { items, ..view("Bilder") }), Some("/Users/anna/Projekt/Bilder".into()));
    }

    #[test]
    fn searches_and_mixed_folders_give_nothing() {
        let search = vec!["/Users/anna/a/x.txt".into(), "/Users/anna/b/y.txt".into()];
        assert_eq!(pick(&FinderView { items: search, ..view("Suche: x") }), None);
        let twins = vec!["/Users/anna/a/Bilder/x.jpg".into(), "/Users/anna/b/Bilder/y.jpg".into()];
        assert_eq!(pick(&FinderView { items: twins, ..view("Bilder") }), None, "zwei Ordner gleichen Namens");
        assert_eq!(pick(&FinderView { items: vec!["/Users/anna/a/x.txt".into()], ..view("") }), None, "ohne Titel");
    }
}
