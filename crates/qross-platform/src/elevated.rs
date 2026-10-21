//! Fenster mit höheren Rechten unter Windows, plattformneutraler Teil.
//!
//! Windows gibt eine Eingabe nur an die Low-Level-Hooks von Prozessen, deren Integritätsstufe
//! mindestens so hoch ist wie die des Fensters, an das sie geht (UIPI): Tasten gehen an das Fenster
//! vorne, die Maus an das unter dem Cursor. Läuft ein Programm mit Administratorrechten – für
//! Administratoren etwa der Task-Manager, der sich selbst so startet –, sieht Qross deshalb keine
//! Tasten, solange sein Fenster vorne liegt, und keine Maus, solange es unter dem Cursor liegt. Ein
//! Server erkennt dann keine Kante mehr, und was getippt wird, landet in diesem Fenster. Abfrage über
//! [`fn@crate::elevated_window`].

/// Ein Fenster mit höherer Integritätsstufe als Qross liegt vorne oder unter dem Cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElevatedWindow {
    /// Prozess, dem es gehört
    pub pid: u32,
    /// Name der ausführbaren Datei, z. B. „Taskmgr.exe“ (`None`: verrät das System nicht)
    pub exe: Option<String>,
    /// Titel des Fensters, z. B. „Administrator: Eingabeaufforderung“ (`None`: hat keinen)
    pub title: Option<String>,
    /// Läuft mit Administratorrechten (Integritätsstufe „hoch“ oder darüber, ohne UI-Zugriff); sonst
    /// steht es aus anderem Grund über Qross, etwa ein Hilfsprogramm mit UI-Zugriff wie die Bildschirm-
    /// oder die Touch-Tastatur.
    pub admin: bool,
}

impl ElevatedWindow {
    /// Gehört es dem Task-Manager?
    pub fn is_task_manager(&self) -> bool {
        self.exe.as_deref().is_some_and(|exe| exe.eq_ignore_ascii_case("Taskmgr.exe"))
    }

    /// Programmname ohne „.exe“, z. B. „regedit“
    pub fn program(&self) -> Option<&str> {
        let exe = self.exe.as_deref()?;
        let stem = match exe.len().checked_sub(4) {
            Some(cut) if exe.is_char_boundary(cut) && exe[cut..].eq_ignore_ascii_case(".exe") => &exe[..cut],
            _ => exe,
        };
        (!stem.is_empty()).then_some(stem)
    }

    /// Fürs Log, z. B. „Taskmgr.exe (PID 4711)“. Ohne den Fenstertitel: Der kann Privates enthalten,
    /// etwa den Namen eines Dokuments.
    pub fn describe(&self) -> String {
        match &self.exe {
            Some(exe) => format!("{exe} (PID {})", self.pid),
            None => format!("PID {}", self.pid),
        }
    }
}

/// Dateiname aus einem Pfad, z. B. „Taskmgr.exe“ aus „C:\Windows\System32\Taskmgr.exe“; `None` bei
/// leerem Pfad oder einem, der auf einen Trenner endet.
pub fn file_name(path: &str) -> Option<&str> {
    path.rsplit(&['\\', '/'][..]).next().filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(exe: Option<&str>) -> ElevatedWindow {
        ElevatedWindow { pid: 4711, exe: exe.map(str::to_owned), title: None, admin: true }
    }

    #[test]
    fn file_names_come_from_windows_paths() {
        assert_eq!(file_name(r"C:\Windows\System32\Taskmgr.exe"), Some("Taskmgr.exe"));
        assert_eq!(file_name(r"\\?\C:\Program Files\App\app.exe"), Some("app.exe"));
        assert_eq!(file_name("C:/Tools/regedit.exe"), Some("regedit.exe"));
        assert_eq!(file_name("cmd.exe"), Some("cmd.exe"));
        assert_eq!(file_name(""), None);
        assert_eq!(file_name(r"C:\Windows\"), None);
    }

    #[test]
    fn the_task_manager_is_recognized_by_its_file() {
        assert!(window(Some("Taskmgr.exe")).is_task_manager());
        assert!(window(Some("TASKMGR.EXE")).is_task_manager());
        assert!(!window(Some("taskmgr.exe.bak")).is_task_manager());
        assert!(!window(Some("regedit.exe")).is_task_manager());
        assert!(!window(None).is_task_manager());
    }

    #[test]
    fn programs_are_named_without_extension() {
        assert_eq!(window(Some("regedit.exe")).program(), Some("regedit"));
        assert_eq!(window(Some("Setup.EXE")).program(), Some("Setup"));
        assert_eq!(window(Some("tool")).program(), Some("tool"));
        assert_eq!(window(Some("äöü.exe")).program(), Some("äöü"));
        assert_eq!(window(Some(".exe")).program(), None);
        assert_eq!(window(Some("ü")).program(), Some("ü"), "kürzer als die Endung");
        assert_eq!(window(None).program(), None);
    }

    #[test]
    fn descriptions_leave_out_the_title() {
        let mut taskmgr = window(Some("Taskmgr.exe"));
        taskmgr.title = Some("Task-Manager".into());
        assert_eq!(taskmgr.describe(), "Taskmgr.exe (PID 4711)");
        assert_eq!(window(None).describe(), "PID 4711");
    }
}
