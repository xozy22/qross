//! Bildschirmschoner abgleichen: erkennen, ob er läuft (Server), und ihn auf Zuruf starten,
//! beenden oder stattdessen sperren (Client).
//!
//! - Windows: erkennt per `SPI_GETSCREENSAVERRUNNING`, startet mit `SC_SCREENSAVE` den
//!   eingerichteten Bildschirmschoner, beendet ihn mit kleinen Mausbewegungen (als Eingabe von Qross
//!   markiert, damit die eigene Erfassung sie durchlässt), sperrt mit `LockWorkStation`.
//! - macOS: erkennt über die verteilten Benachrichtigungen `com.apple.screensaver.didstart` und
//!   `didstop`, startet und beendet `ScreenSaverEngine`, sperrt über die Sperrfunktion des Systems
//!   (wie Ctrl+Cmd+Q).
//! - Linux: über den Bildschirmschoner-Dienst des Desktops auf dem D-Bus (GNOME, Cinnamon, MATE,
//!   `org.freedesktop.ScreenSaver` bei KDE, Xfce und anderen), in einer X11-Sitzung ohne solchen
//!   Dienst über den Bildschirmschoner des X-Servers. Sperren über `loginctl lock-session`.
//!
//! Startet ein Client seinen Bildschirmschoner, gilt dort dasselbe wie nach Ablauf der Wartezeit:
//! Verlangt das System danach ein Passwort, ist er gesperrt und muss an seiner eigenen Tastatur
//! entsperrt werden. Ist gar kein Bildschirmschoner eingerichtet oder hält ein Programm den
//! Bildschirm wach (Video, Präsentation), startet Qross keinen.

use std::fmt;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub fn running() -> bool {
        false
    }

    pub fn start() -> super::Outcome {
        super::Outcome::Unsupported("on this system")
    }

    pub fn stop() -> super::Outcome {
        super::Outcome::Unsupported("on this system")
    }

    pub fn lock() -> super::Outcome {
        super::Outcome::Unsupported("on this system")
    }
}

/// Was beim Starten, Beenden oder Sperren passiert ist (fürs Protokoll)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started,
    Stopped,
    Locked,
    /// Lief schon bzw. lief gar nicht
    Unchanged,
    /// Hier ist kein Bildschirmschoner eingerichtet; Qross startet dann auch keinen.
    NotConfigured,
    /// Ein Programm hält den Bildschirm wach (Video, Präsentation); das System selbst würde jetzt
    /// auch keinen starten.
    KeptAwake,
    /// Hier nicht möglich, mit Grund (z. B. „in einer Remotedesktop-Sitzung“)
    Unsupported(&'static str),
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Started => f.write_str("started"),
            Outcome::Stopped => f.write_str("stopped"),
            Outcome::Locked => f.write_str("computer locked"),
            Outcome::Unchanged => f.write_str("unchanged"),
            Outcome::NotConfigured => f.write_str("none configured here"),
            Outcome::KeptAwake => f.write_str("a program is keeping the screen awake"),
            Outcome::Unsupported(why) => write!(f, "not possible {why}"),
            Outcome::Failed(e) => write!(f, "failed: {e}"),
        }
    }
}

/// Läuft auf diesem Rechner gerade der Bildschirmschoner? Wird regelmäßig abgefragt. Unter macOS
/// stammt die Antwort aus den Benachrichtigungen des Systems seit der ersten Abfrage.
pub fn running() -> bool {
    imp::running()
}

/// Bildschirmschoner starten, wie es das System nach Ablauf der Wartezeit täte. Mit `lock` wird der
/// Rechner stattdessen gesperrt; geht das nicht, startet wenigstens der Bildschirmschoner.
pub fn start(lock: bool) -> Outcome {
    if lock {
        match imp::lock() {
            Outcome::Locked => return Outcome::Locked,
            other => tracing::warn!("cannot lock the computer ({other}); starting only the screen saver"),
        }
    }
    imp::start()
}

/// Bildschirmschoner beenden, als hätte jemand die Maus bewegt. War der Rechner gesperrt, bleibt er
/// es und zeigt die Anmeldung.
pub fn stop() -> Outcome {
    imp::stop()
}

/// Für die Erfassungen der Systeme: Der Bildschirmschoner des Servers soll enden, die Eingaben dafür
/// fängt Qross aber gerade für einen Client ab. Beenden dauert unter Windows einen Moment; die
/// Erfassung soll derweil weiterlaufen.
pub fn stop_in_background() {
    let spawned = std::thread::Builder::new().name("qross-screensaver-stop".into()).spawn(|| match stop() {
        Outcome::Failed(e) => tracing::debug!("failed to stop the screen saver: {e}"),
        outcome => tracing::debug!("screen saver: {outcome}"),
    });
    if let Err(e) = spawned {
        tracing::warn!("failed to stop the screen saver: {e}");
    }
}

/// Für die Einspeisungen der Systeme: dem Bildschirmschoner des Servers folgen
pub fn follow(active: bool, lock: bool) -> Outcome {
    if active {
        start(lock)
    } else {
        stop()
    }
}

/// Windows: Ist ein Bildschirmschoner eingerichtet? Bildschirmschoner an (`SPI_GETSCREENSAVEACTIVE`)
/// und eine `.scr` gewählt (`SCRNSAVE.EXE`, per Richtlinie oder vom Nutzer). „(Kein)“ lässt den
/// Wert leer bzw. weg.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_configured(active: bool, exe: Option<&str>) -> bool {
    active && exe.is_some_and(|exe| !exe.trim().is_empty())
}

/// macOS: `idleTime` aus `com.apple.screensaver` (Sekunden bis zum Start); 0 heißt „Nie“. Fehlt der
/// Wert, gilt die Vorgabe des Systems, also eingerichtet.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_configured(idle_time: Option<i64>) -> bool {
    idle_time != Some(0)
}

/// GNOME: `org.gnome.desktop.session idle-delay`, wie `gsettings get` es ausgibt („uint32 300“);
/// 0 heißt „Bildschirm abschalten: Nie“. Unlesbar: eingerichtet.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gnome_configured(gsettings: &str) -> bool {
    gsettings.split_whitespace().last().and_then(|n| n.parse::<u32>().ok()) != Some(0)
}

/// KDE: Abschnitt `[Daemon]` aus `kscreenlockerrc`. Ohne `Autolock=false` sperrt KDE nach der
/// Wartezeit (Vorgabe); der „Bildschirmschoner“ von KDE ist die Sperre.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn kde_configured(kscreenlockerrc: &str) -> bool {
    let mut daemon = false;
    for line in kscreenlockerrc.lines().map(str::trim) {
        if line.starts_with('[') {
            daemon = line == "[Daemon]";
        } else if daemon {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "Autolock" {
                    return value.trim() != "false";
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_needs_a_chosen_screen_saver() {
        assert!(windows_configured(true, Some(r"C:\Windows\system32\Mystify.scr")));
        assert!(!windows_configured(true, None), "„(Kein)“");
        assert!(!windows_configured(true, Some(" ")));
        assert!(!windows_configured(false, Some(r"C:\Windows\system32\Mystify.scr")), "abgeschaltet");
    }

    #[test]
    fn mac_never_means_not_configured() {
        assert!(mac_configured(None));
        assert!(mac_configured(Some(300)));
        assert!(!mac_configured(Some(0)));
    }

    #[test]
    fn gnome_idle_delay_zero_means_never() {
        assert!(gnome_configured("uint32 300\n"));
        assert!(!gnome_configured("uint32 0\n"));
        assert!(gnome_configured(""), "unlesbar: wie eingerichtet");
    }

    #[test]
    fn kde_locks_unless_autolock_is_off() {
        assert!(kde_configured(""));
        assert!(kde_configured("[Daemon]\nTimeout=10\n"));
        assert!(!kde_configured("[Daemon]\nAutolock=false\nTimeout=10\n"));
        assert!(kde_configured("[Greeter]\nAutolock=false\n[Daemon]\nAutolock=true\n"));
    }

    #[test]
    fn outcomes_read_well_in_the_log() {
        assert_eq!(Outcome::NotConfigured.to_string(), "none configured here");
        assert_eq!(Outcome::Failed("x".into()).to_string(), "failed: x");
        assert_eq!(Outcome::Unsupported("in a Remote Desktop session").to_string(), "not possible in a Remote Desktop session");
    }
}
