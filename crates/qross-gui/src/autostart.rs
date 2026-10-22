//! Mit dem System starten: Qross trägt sich für die Anmeldung des Nutzers ein und startet dann mit
//! [`ARG`] still im Tray (ohne Tray minimiert).
//!
//! - Windows: Wert „Qross“ unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Windows zeigt ihn
//!   unter Einstellungen → Apps → Autostart und im Task-Manager. Schaltet ihn jemand dort ab, steht das
//!   unter `…\Explorer\StartupApproved\Run`, und er gilt als aus.
//! - macOS: LaunchAgent `~/Library/LaunchAgents/Qross.plist`. Über die Bundle-ID zeigen die
//!   Systemeinstellungen ihn unter Allgemein → Anmeldeobjekte als „Qross“.
//! - Linux: XDG-Autostart `~/.config/autostart/Qross.desktop`. Das beachten GNOME, KDE, Xfce, Cinnamon und
//!   Co. unter X11 wie Wayland.
//!
//! Namen und Orte sind dieselben wie bei `tauri-plugin-autostart` bis Qross 0.7.0; dessen Einträge
//! übernimmt Qross und bringt sie beim nächsten Start auf den neuen Stand.
//!
//! Der Eintrag zeigt auf die Programmdatei, aus der Qross gerade läuft, unter Linux auf die AppImage
//! selbst. Wurde Qross verschoben, bringt der nächste Start ihn auf den neuen Pfad. Von einem Ort, den es
//! nach dem Neustart nicht mehr gibt (macOS: DMG oder Quarantäne-Kopie), trägt Qross sich nicht ein.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use tracing::{info, warn};

use crate::i18n::Lang;

/// Kennzeichen im Aufruf: Qross wurde vom Autostart gestartet
pub const ARG: &str = "--autostart";
/// Name des Eintrags (wie beim Tauri-Plugin bis Qross 0.7.0)
const NAME: &str = "Qross";
/// Bundle-ID: macOS ordnet den LaunchAgent damit der App zu (Name und Symbol in den Anmeldeobjekten)
const BUNDLE_ID: &str = "app.qross.desktop";

/// Warum sich Qross von hier aus nicht in den Autostart eintragen kann
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsuitable {
    /// macOS: vorübergehende Kopie, solange die App unter Quarantäne steht
    Translocated,
    /// macOS: direkt aus dem DMG bzw. von einem eingehängten Laufwerk
    OnVolume,
    /// Linux: aus einer AppImage, deren Ort unbekannt ist
    AppImageUnknown,
    /// Der eigene Programmpfad ist unbekannt
    ProgramUnknown,
}

impl Unsuitable {
    /// Der Grund in Worten, für die Oberfläche in `lang`, fürs Log englisch
    pub fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Translocated, Lang::De) => {
                "Qross läuft aus einer vorübergehenden Kopie, weil macOS es noch unter Quarantäne hält. Zieh Qross in \
                 den Ordner „Programme“ und starte es dort, dann lässt sich der Autostart einschalten."
            }
            (Self::Translocated, Lang::En) => {
                "Qross is running from a temporary copy because macOS still has it in quarantine. Move Qross to the \
                 Applications folder and start it from there, then you can turn on autostart."
            }
            (Self::OnVolume, Lang::De) => {
                "Qross läuft direkt aus dem DMG bzw. von einem eingehängten Laufwerk. Zieh Qross in den Ordner \
                 „Programme“ und starte es dort, dann lässt sich der Autostart einschalten."
            }
            (Self::OnVolume, Lang::En) => {
                "Qross is running directly from the DMG or a mounted volume. Move Qross to the Applications folder \
                 and start it from there, then you can turn on autostart."
            }
            (Self::AppImageUnknown, Lang::De) => {
                "Qross läuft aus einer AppImage, deren Ort unbekannt ist. Starte die AppImage direkt, dann lässt sich \
                 der Autostart einschalten."
            }
            (Self::AppImageUnknown, Lang::En) => {
                "Qross is running from an AppImage whose location is unknown. Start the AppImage directly, then you \
                 can turn on autostart."
            }
            (Self::ProgramUnknown, Lang::De) => "Qross kennt den eigenen Programmpfad nicht.",
            (Self::ProgramUnknown, Lang::En) => "Qross doesn't know the path of its own program file.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Wo man den Eintrag im System wiederfindet (für die Oberfläche, in `lang`)
    pub const fn place(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Windows, Lang::De) => "Windows-Einstellungen → Apps → Autostart",
            (Self::Windows, Lang::En) => "Windows Settings → Apps → Startup",
            (Self::MacOs, Lang::De) => "Systemeinstellungen → Allgemein → Anmeldeobjekte",
            (Self::MacOs, Lang::En) => "System Settings → General → Login Items",
            (Self::Linux, _) => "~/.config/autostart/Qross.desktop",
        }
    }
}

/// Was bei der Anmeldung starten soll
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Diese Datei: das Programm bzw. unter Linux die AppImage
    Program(PathBuf),
    /// Von hier aus nicht; der Grund steht in der Oberfläche
    Unsuitable(Unsuitable),
}

/// Ziel des Eintrags für das Programm `exe`; `appimage` ist unter Linux der Pfad der AppImage, aus der
/// Qross läuft (`exe` liegt dann in ihrem flüchtigen Einhängepunkt).
pub fn target(os: Os, exe: &Path, appimage: Option<&Path>) -> Target {
    let text = exe.to_string_lossy();
    match os {
        Os::MacOs if text.contains("/AppTranslocation/") => Target::Unsuitable(Unsuitable::Translocated),
        Os::MacOs if text.starts_with("/Volumes/") => Target::Unsuitable(Unsuitable::OnVolume),
        Os::Linux => match appimage {
            Some(image) => Target::Program(image.to_path_buf()),
            None if text.starts_with("/tmp/.mount_") => Target::Unsuitable(Unsuitable::AppImageUnknown),
            None => Target::Program(exe.to_path_buf()),
        },
        Os::Windows => Target::Program(without_verbatim(exe)),
        _ => Target::Program(exe.to_path_buf()),
    }
}

/// Windows: `canonicalize` liefert den Pfad als `\\?\C:\…`. Einen Autostart-Eintrag in dieser Schreibweise
/// zeigt Windows unter „Autostart“ als eingeschaltet an, führt ihn bei der Anmeldung aber nicht aus.
fn without_verbatim(exe: &Path) -> PathBuf {
    let text = exe.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        // Nur mit Laufwerksbuchstaben; andere Formen (Volume-GUID) haben keine kurze Schreibweise
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => exe.to_path_buf(),
    }
}

/// Der Eintrag, wie Qross ihn schreibt: unter Windows die Befehlszeile, unter macOS die plist, unter
/// Linux die `.desktop`-Datei
pub fn entry(os: Os, program: &Path) -> String {
    let path = program.to_string_lossy();
    match os {
        // In Anführungszeichen: Pfade wie „C:\Program Files\…“ enthalten Leerzeichen
        Os::Windows => format!("\"{path}\" {ARG}"),
        Os::MacOs => format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{NAME}</string>
  <key>AssociatedBundleIdentifiers</key>
  <array>
    <string>{BUNDLE_ID}</string>
  </array>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>{ARG}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#,
            xml_escape(&path)
        ),
        Os::Linux => format!(
            "[Desktop Entry]\nType=Application\nName={NAME}\nComment=Mouse and keyboard for multiple computers\n\
             Exec={} {ARG}\nTerminal=false\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n",
            desktop_exec_arg(&path)
        ),
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Ein Argument für `Exec=` nach der Desktop-Entry-Spezifikation: `%` verdoppeln; enthält es Leerzeichen
/// oder Sonderzeichen, in Anführungszeichen, darin `"`, `` ` ``, `$` und `\` maskiert. Weil die
/// Spezifikation erst die Maskierung von Zeichenketten anwendet, steht vor jedem davon `\\`.
fn desktop_exec_arg(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    if !arg.chars().any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c)) {
        return arg;
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str(r"\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str(r"\\\\"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Hat der Nutzer den Eintrag in seiner Desktop-Umgebung abgeschaltet (GNOME Tweaks, KDE)?
#[cfg_attr(windows, allow(dead_code))]
fn desktop_entry_disabled(content: &str) -> bool {
    content.lines().map(str::trim).any(|line| line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false")
}

/// Der Autostart dieses Rechners für die laufende Qross-Installation
pub struct Autostart {
    os: Os,
    target: Target,
    name: &'static str,
}

impl Autostart {
    /// `appimage`: unter Linux der Pfad der AppImage, aus der Qross läuft (siehe `tauri::Env`)
    pub fn new(appimage: Option<&Path>) -> Self {
        let os = Os::current();
        let target = match std::env::current_exe() {
            // Unter macOS über Links hinweg; der Pfad im Bundle bleibt
            Ok(exe) => target(os, &exe.canonicalize().unwrap_or(exe), appimage),
            Err(e) => {
                warn!("autostart: unknown program path: {e}");
                Target::Unsuitable(Unsuitable::ProgramUnknown)
            }
        };
        Self { os, target, name: NAME }
    }

    /// Warum sich Qross von hier aus nicht eintragen kann (in `lang`)
    pub fn unsuitable(&self, lang: Lang) -> Option<&'static str> {
        match self.target {
            Target::Unsuitable(reason) => Some(reason.text(lang)),
            Target::Program(_) => None,
        }
    }

    pub fn place(&self, lang: Lang) -> &'static str {
        self.os.place(lang)
    }

    /// Eingetragen und nicht im System abgeschaltet
    pub fn is_enabled(&self) -> bool {
        sys::read(self.name).is_some_and(|entry| !sys::disabled(self.name, &entry))
    }

    pub fn set(&self, enabled: bool) -> Result<()> {
        if !enabled {
            sys::remove(self.name)?;
            info!("autostart turned off");
            return Ok(());
        }
        match &self.target {
            Target::Unsuitable(reason) => bail!("{}", reason.text(Lang::En)),
            Target::Program(program) => {
                sys::write(self.name, &entry(self.os, program))?;
                info!("autostart turned on: {}", program.display());
                Ok(())
            }
        }
    }

    /// Beim Start (nur Release-Builds): beim ersten Mal eintragen, danach einen vorhandenen Eintrag auf
    /// den aktuellen Pfad bringen. Liefert, ob die Ersteinrichtung erledigt ist; von einem ungeeigneten
    /// Ort aus bleibt sie offen, bis Qross von einem festen Ort startet.
    pub fn sync(&self, configured: bool) -> bool {
        let program = match &self.target {
            Target::Unsuitable(reason) => {
                info!("autostart left unchanged: {}", reason.text(Lang::En));
                return configured;
            }
            Target::Program(program) => program,
        };
        if !configured {
            match self.set(true) {
                Ok(()) => info!("autostart set up: Qross starts at login"),
                Err(e) => warn!("failed to set up autostart: {e:#}"),
            }
            return true;
        }
        let wanted = entry(self.os, program);
        if self.is_enabled() && sys::read(self.name).as_deref() != Some(wanted.as_str()) {
            match sys::write(self.name, &wanted) {
                Ok(()) => info!("autostart now points to {}", program.display()),
                Err(e) => warn!("failed to update autostart: {e:#}"),
            }
        }
        true
    }
}

#[cfg(windows)]
mod sys {
    use anyhow::Result;
    use windows_registry::{Type, CURRENT_USER, LOCAL_MACHINE};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    /// „Eingeschaltet“ im Task-Manager
    const APPROVED_ON: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    pub fn read(name: &str) -> Option<String> {
        CURRENT_USER.open(RUN).and_then(|key| key.get_string(name)).ok()
    }

    /// Im Task-Manager bzw. unter Einstellungen → Apps → Autostart abgeschaltet: Das erste Byte ist dann
    /// ungerade (3 bzw. 7), danach folgt der Zeitpunkt.
    pub fn disabled(name: &str, _entry: &str) -> bool {
        CURRENT_USER
            .open(APPROVED)
            .and_then(|key| key.get_value(name))
            .is_ok_and(|value| value.first().is_some_and(|b| b & 1 == 1))
    }

    pub fn write(name: &str, entry: &str) -> Result<()> {
        CURRENT_USER.create(RUN)?.set_string(name, entry)?;
        // War er dort abgeschaltet, gilt jetzt wieder die Wahl in Qross
        if let Ok(key) = CURRENT_USER.options().read().write().open(APPROVED) {
            if key.get_value(name).is_ok() {
                key.set_bytes(name, Type::Bytes, &APPROVED_ON)?;
            }
        }
        Ok(())
    }

    pub fn remove(name: &str) -> Result<()> {
        if read(name).is_some() {
            CURRENT_USER.options().read().write().open(RUN)?.remove_value(name)?;
        }
        if let Ok(key) = CURRENT_USER.options().read().write().open(APPROVED) {
            if key.get_value(name).is_ok() {
                key.remove_value(name)?;
            }
        }
        // Ältere Versionen trugen sich mit Administratorrechten für alle Nutzer ein; ohne Rechte bleibt das
        if let Ok(key) = LOCAL_MACHINE.options().read().write().open(RUN) {
            if key.get_string(name).is_ok() {
                let _ = key.remove_value(name);
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod sys {
    use std::path::PathBuf;

    use anyhow::{Context, Result};

    fn file(name: &str) -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            Some(dirs::home_dir()?.join("Library/LaunchAgents").join(format!("{name}.plist")))
        } else {
            Some(dirs::config_dir()?.join("autostart").join(format!("{name}.desktop")))
        }
    }

    pub fn read(name: &str) -> Option<String> {
        std::fs::read_to_string(file(name)?).ok()
    }

    /// Unter Linux in der Desktop-Umgebung abgeschaltet; unter macOS ist der Schalter in den
    /// Anmeldeobjekten von außen nicht lesbar.
    pub fn disabled(_name: &str, entry: &str) -> bool {
        cfg!(target_os = "linux") && super::desktop_entry_disabled(entry)
    }

    pub fn write(name: &str, entry: &str) -> Result<()> {
        let path = file(name).context("unknown home directory")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        }
        std::fs::write(&path, entry).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn remove(name: &str) -> Result<()> {
        let Some(path) = file(name) else { return Ok(()) };
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("failed to delete {}", path.display()))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsuitable_places_are_refused() {
        let translocated = Path::new("/private/var/folders/x/T/AppTranslocation/1234/d/Qross.app/Contents/MacOS/qross-gui");
        assert_eq!(target(Os::MacOs, translocated, None), Target::Unsuitable(Unsuitable::Translocated));
        let dmg = Path::new("/Volumes/Qross/Qross.app/Contents/MacOS/qross-gui");
        assert_eq!(target(Os::MacOs, dmg, None), Target::Unsuitable(Unsuitable::OnVolume));
        let installed = Path::new("/Applications/Qross.app/Contents/MacOS/qross-gui");
        assert_eq!(target(Os::MacOs, installed, None), Target::Program(installed.into()));

        let mounted = Path::new("/tmp/.mount_QrossAbc/usr/bin/qross-gui");
        let image = Path::new("/home/anna/Programme/Qross_0.7.1_amd64.AppImage");
        assert_eq!(target(Os::Linux, mounted, Some(image)), Target::Program(image.into()), "die AppImage selbst");
        assert_eq!(target(Os::Linux, mounted, None), Target::Unsuitable(Unsuitable::AppImageUnknown));
        assert_eq!(target(Os::Linux, Path::new("/usr/bin/qross-gui"), None), Target::Program("/usr/bin/qross-gui".into()));

        let windows = Path::new(r"C:\Program Files\Qross\qross-gui.exe");
        assert_eq!(target(Os::Windows, windows, None), Target::Program(windows.into()));
    }

    #[test]
    fn reasons_and_places_in_both_languages() {
        assert_eq!(
            Unsuitable::OnVolume.text(Lang::De),
            "Qross läuft direkt aus dem DMG bzw. von einem eingehängten Laufwerk. Zieh Qross in den Ordner „Programme“ \
             und starte es dort, dann lässt sich der Autostart einschalten."
        );
        assert_eq!(
            Unsuitable::Translocated.text(Lang::De),
            "Qross läuft aus einer vorübergehenden Kopie, weil macOS es noch unter Quarantäne hält. Zieh Qross in den \
             Ordner „Programme“ und starte es dort, dann lässt sich der Autostart einschalten."
        );
        assert_eq!(
            Unsuitable::AppImageUnknown.text(Lang::De),
            "Qross läuft aus einer AppImage, deren Ort unbekannt ist. Starte die AppImage direkt, dann lässt sich der \
             Autostart einschalten."
        );
        assert_eq!(Unsuitable::ProgramUnknown.text(Lang::De), "Qross kennt den eigenen Programmpfad nicht.");
        assert!(Unsuitable::OnVolume.text(Lang::En).starts_with("Qross is running directly from the DMG"));
        assert!(Unsuitable::Translocated.text(Lang::En).ends_with("then you can turn on autostart."));
        assert_eq!(Os::Windows.place(Lang::De), "Windows-Einstellungen → Apps → Autostart");
        assert_eq!(Os::Windows.place(Lang::En), "Windows Settings → Apps → Startup");
        assert_eq!(Os::MacOs.place(Lang::En), "System Settings → General → Login Items");
        assert_eq!(Os::Linux.place(Lang::En), Os::Linux.place(Lang::De));
    }

    #[test]
    fn windows_entry_quotes_the_path() {
        let entry = entry(Os::Windows, Path::new(r"C:\Users\Anna Muster\AppData\Local\Qross\qross-gui.exe"));
        assert_eq!(entry, r#""C:\Users\Anna Muster\AppData\Local\Qross\qross-gui.exe" --autostart"#);
    }

    #[test]
    fn windows_entry_has_no_verbatim_prefix() {
        // So liefert `canonicalize` den Pfad; Windows startet einen Eintrag mit `\\?\` bei der Anmeldung nicht.
        let exe = Path::new(r"\\?\C:\Users\Dennis\AppData\Local\Qross\qross-gui.exe");
        let Target::Program(program) = target(Os::Windows, exe, None) else { panic!("Ziel erwartet") };
        assert_eq!(entry(Os::Windows, &program), r#""C:\Users\Dennis\AppData\Local\Qross\qross-gui.exe" --autostart"#);
        let unc = Path::new(r"\\?\UNC\server\apps\Qross\qross-gui.exe");
        assert_eq!(target(Os::Windows, unc, None), Target::Program(PathBuf::from(r"\\server\apps\Qross\qross-gui.exe")));
        let plain = Path::new(r"D:\Qross\qross-gui.exe");
        assert_eq!(target(Os::Windows, plain, None), Target::Program(plain.to_path_buf()));
        let volume = Path::new(r"\\?\Volume{0f3c}\Qross\qross-gui.exe");
        assert_eq!(target(Os::Windows, volume, None), Target::Program(volume.to_path_buf()), "keine kurze Form");
    }

    #[test]
    fn launch_agent_names_the_app_and_escapes_the_path() {
        let plist = entry(Os::MacOs, Path::new("/Users/anna/Apps & Tools/Qross.app/Contents/MacOS/qross-gui"));
        assert!(plist.contains("<string>/Users/anna/Apps &amp; Tools/Qross.app/Contents/MacOS/qross-gui</string>"), "{plist}");
        assert!(plist.contains("<string>--autostart</string>"));
        assert!(plist.contains("<key>Label</key>\n  <string>Qross</string>"));
        assert!(plist.contains("<string>app.qross.desktop</string>"), "Anmeldeobjekte zeigen „Qross“");
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
    }

    #[test]
    fn desktop_entry_quotes_like_the_specification() {
        let plain = entry(Os::Linux, Path::new("/usr/bin/qross-gui"));
        assert!(plain.contains("\nExec=/usr/bin/qross-gui --autostart\n"), "{plain}");
        assert!(plain.starts_with("[Desktop Entry]\nType=Application\nName=Qross\n"));

        let spaced = entry(Os::Linux, Path::new("/home/anna/Meine Programme/Qross 100%.AppImage"));
        assert!(spaced.contains("\nExec=\"/home/anna/Meine Programme/Qross 100%%.AppImage\" --autostart\n"), "{spaced}");
        assert_eq!(desktop_exec_arg(r#"/a/"b"/$c/\d"#), r#""/a/\\"b\\"/\\$c/\\\\d""#);
    }

    #[test]
    fn desktop_entries_switched_off_by_the_desktop_count_as_off() {
        let entry = entry(Os::Linux, Path::new("/usr/bin/qross-gui"));
        assert!(!desktop_entry_disabled(&entry));
        assert!(desktop_entry_disabled(&entry.replace("X-GNOME-Autostart-enabled=true", "X-GNOME-Autostart-enabled=false")));
        assert!(desktop_entry_disabled(&format!("{entry}Hidden=true\n")));
    }

    /// Schreibt einen echten Eintrag unter einem eigenen Namen und entfernt ihn wieder. Läuft nur in CI
    /// (`CI=true`) oder mit `QROSS_AUTOSTART_TEST=1`, weil er die Autostart-Liste des Systems ändert.
    #[test]
    fn real_entry_roundtrip() {
        let on = std::env::var_os("CI").is_some_and(|v| v == "true") || std::env::var_os("QROSS_AUTOSTART_TEST").is_some();
        if !on {
            eprintln!("echter Autostart nur in CI oder mit QROSS_AUTOSTART_TEST=1 – übersprungen");
            return;
        }
        let program = std::env::current_exe().unwrap();
        let autostart = Autostart { os: Os::current(), target: Target::Program(program.clone()), name: "QrossTest" };
        autostart.set(false).unwrap();
        assert!(!autostart.is_enabled());

        autostart.set(true).unwrap();
        assert!(autostart.is_enabled());
        assert_eq!(sys::read("QrossTest").as_deref(), Some(entry(Os::current(), &program).as_str()));

        // Windows: im Task-Manager abgeschaltet gilt als aus; Einschalten in Qross gibt ihn wieder frei
        #[cfg(windows)]
        {
            let approved = windows_registry::CURRENT_USER
                .create(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
                .unwrap();
            approved.set_bytes("QrossTest", windows_registry::Type::Bytes, &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
            assert!(!autostart.is_enabled(), "im Task-Manager abgeschaltet");
            autostart.set(true).unwrap();
            assert!(autostart.is_enabled(), "wieder freigegeben");
        }

        // Verschoben: Der nächste Start bringt den Eintrag auf den neuen Pfad
        let moved = Autostart { target: Target::Program(program.with_file_name("verschoben")), ..autostart };
        assert!(moved.sync(true));
        assert!(sys::read("QrossTest").unwrap().contains("verschoben"));

        moved.set(false).unwrap();
        assert!(!moved.is_enabled());
        assert_eq!(sys::read("QrossTest"), None);
        #[cfg(windows)]
        assert!(windows_registry::CURRENT_USER
            .open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run")
            .and_then(|key| key.get_value("QrossTest"))
            .is_err(), "auch die Freigabe ist weg");
        moved.set(false).unwrap();
    }
}
