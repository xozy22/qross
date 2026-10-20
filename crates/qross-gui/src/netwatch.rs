//! Profil nach Netzwerk: fragt das Netzwerk alle paar Sekunden ab und aktiviert bei einem Wechsel
//! das passende Profil (Regeln in `qross_gui::profiles`). Lief Server oder Client, startet er mit
//! dem neuen Profil neu.

use std::sync::Arc;
use std::time::Duration;

use qross_gui::i18n::{self, Lang};
use qross_gui::profiles::{self, Choice, NetworkMark};
use qross_gui::Role;
use tauri::Emitter;
use tracing::{info, warn};

use crate::runtime::Runtime;
use crate::updates::AppNotice;

/// Abstand der Abfragen. Ein neues Netzwerk zählt nach zwei gleichen Ergebnissen, also nach 4–8 s.
const POLL: Duration = Duration::from_secs(4);

/// Aktuelle Netzwerke, bevorzugtes zuerst. `names`: auch die Namen (Linux startet dafür `nmcli`).
pub async fn networks(names: bool) -> Vec<NetworkMark> {
    let query = move || qross_platform::network::current(names).iter().map(NetworkMark::of).collect();
    tauri::async_runtime::spawn_blocking(query).await.unwrap_or_default()
}

/// Beim Start, bevor Server oder Client fortgesetzt werden: In einem anderen Netzwerk als bei der
/// letzten Wahl gleich das passende Profil aktivieren. Im selben bleibt ein von Hand gewähltes.
pub async fn check_at_start(rt: &Arc<Runtime>) {
    if !rt.profiles.lock().unwrap().automatic() {
        return;
    }
    let current = networks(true).await;
    let signature = profiles::signature(&current);
    if current.is_empty() || rt.app_state().network.as_deref() == Some(signature.as_str()) {
        return;
    }
    choose(rt, current, signature).await;
}

/// Sofort nach dem Netzwerk wählen, etwa wenn die Automatik eingeschaltet wird.
pub async fn check_now(rt: &Arc<Runtime>) {
    let current = networks(true).await;
    if current.is_empty() {
        return;
    }
    let signature = profiles::signature(&current);
    choose(rt, current, signature).await;
}

/// Fragt im Hintergrund ab, solange die Automatik etwas zu tun hat.
pub fn spawn(rt: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        // Beim Wechsel tauchen Gateway und dessen MAC nacheinander auf; erst das zweite gleiche
        // Ergebnis zählt, sonst wechselte das Profil womöglich zweimal.
        let mut candidate: Option<String> = None;
        loop {
            tokio::time::sleep(POLL).await;
            if !rt.profiles.lock().unwrap().automatic() {
                candidate = None;
                continue;
            }
            let current = networks(false).await;
            if current.is_empty() {
                // offline: Profil behalten, bis wieder ein Netzwerk da ist
                continue;
            }
            let signature = profiles::signature(&current);
            if rt.app_state().network.as_deref() == Some(signature.as_str()) {
                candidate = None;
            } else if candidate.as_deref() != Some(signature.as_str()) {
                candidate = Some(signature);
            } else {
                candidate = None;
                check_now(&rt).await;
            }
        }
    });
}

/// Merkt sich das Netzwerk, aktiviert das passende Profil und sagt Bescheid.
async fn choose(rt: &Arc<Runtime>, current: Vec<NetworkMark>, signature: String) {
    rt.update_app_state(|s| s.network = Some(signature));
    let profiles = rt.profiles();
    // Erkanntes Netzwerk; `None`: Kein Profil passt, es gilt das für unbekannte Netzwerke.
    let (id, network) = match profiles.choose(&current) {
        Choice::Keep => return,
        Choice::Network(id) => {
            let label = profiles.matched(&id, &current).map(|n| n.label.clone()).unwrap_or_default();
            (id, Some(label))
        }
        Choice::Fallback(id) => (id, None),
    };
    let name = profiles.get(&id).map(|p| p.name.clone()).unwrap_or_default();
    match &network {
        Some(label) => info!("network '{label}' detected, switching to profile '{name}'"),
        None => info!("no profile matches this network, switching to profile '{name}'"),
    }
    let was_running = rt.status().running();
    let result = rt.activate_profile(&id).await;
    if rt.profiles().active != id {
        if let Err(e) = result {
            warn!("failed to activate profile '{name}': {e:#}");
        }
        return;
    }
    let app = rt.app_handle();
    let settings = rt.settings.lock().unwrap().clone();
    let lang = i18n::lang();
    let reason = match (&network, lang) {
        (Some(label), Lang::De) => format!("Netzwerk „{label}“ erkannt."),
        (Some(label), Lang::En) => format!("Network “{label}” detected."),
        (None, Lang::De) => "Kein Profil passt zu diesem Netzwerk.".to_owned(),
        (None, Lang::En) => "No profile matches this network.".to_owned(),
    };
    let address = settings.client.address.trim();
    let what = match (&result, settings.role, lang) {
        (Ok(Some(_)), _, _) => {
            // Das Fenster fragt nach dem Fingerprint, sobald es offen ist.
            let _ = app.emit("start-from-tray", ());
            match lang {
                Lang::De => " Den Server bitte im Fenster bestätigen.".to_owned(),
                Lang::En => " Please confirm the server in the window.".to_owned(),
            }
        }
        (Err(e), _, Lang::De) => format!(" Start fehlgeschlagen: {e:#}"),
        (Err(e), _, Lang::En) => format!(" Couldn't start: {e:#}"),
        (Ok(None), ..) if !was_running => String::new(),
        (Ok(None), Role::Server, Lang::De) => " Der Server läuft jetzt mit der Anordnung dieses Profils.".to_owned(),
        (Ok(None), Role::Server, Lang::En) => " The server is now running with this profile's layout.".to_owned(),
        (Ok(None), Role::Client, Lang::De) => format!(" Der Client verbindet sich mit {address}."),
        (Ok(None), Role::Client, Lang::En) => format!(" The client is connecting to {address}."),
    };
    let text = format!("{reason}{what}");
    let title = match lang {
        Lang::De => format!("Profil „{name}“"),
        Lang::En => format!("Profile “{name}”"),
    };
    if crate::window_in_front(app) {
        let tone = if result.is_err() { "warn" } else { "info" };
        let _ = app.emit("app-notice", AppNotice { tone, text: format!("{title}: {text}"), action: None });
    } else {
        crate::notify(app, &title, &text);
    }
}
