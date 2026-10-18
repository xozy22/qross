//! Tray-Icon: zeigt den Zustand als farbigen Punkt und im Tooltip.

use std::sync::{Arc, OnceLock};

use qross_gui::Role;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::runtime::{Phase, Runtime, Status};

const TRAY_ID: &str = "main";

static TOGGLE: OnceLock<MenuItem<Wry>> = OnceLock::new();

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Qross öffnen", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Starten", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &PredefinedMenuItem::separator(app)?, &quit])?;
    let _ = TOGGLE.set(toggle);

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(&Status::default()))
        .tooltip("Qross – gestoppt")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::show_main(app),
            "toggle" => {
                let app = app.clone();
                let rt = app.state::<Arc<Runtime>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    if rt.status().running() {
                        rt.stop().await;
                    } else if !matches!(rt.start().await, Ok(None)) {
                        // Fehler oder unbekannter Server: im Fenster zeigen
                        crate::show_main(&app);
                        let _ = app.emit("start-from-tray", ());
                    }
                });
            }
            "quit" => {
                let app = app.clone();
                let rt = app.state::<Arc<Runtime>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    rt.stop().await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn update(app: &AppHandle, status: &Status) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_icon(Some(icon(status)));
    let _ = tray.set_tooltip(Some(tooltip(status)));
    if let Some(toggle) = TOGGLE.get() {
        let _ = toggle.set_text(if status.running() { "Stoppen" } else { "Starten" });
    }
}

fn tooltip(status: &Status) -> String {
    let role = match status.role {
        Some(Role::Server) => "Server",
        Some(Role::Client) => "Client",
        None => return format!("Qross – {}", if status.phase == Phase::Error { "Fehler" } else { "gestoppt" }),
    };
    let detail = match (status.role, status.phase) {
        (Some(Role::Server), Phase::Listening) => match status.clients.len() {
            0 => "wartet auf Clients".to_owned(),
            1 => "1 Client verbunden".to_owned(),
            n => format!("{n} Clients verbunden"),
        },
        (_, Phase::Connected) => format!("verbunden mit {}", status.server_name.as_deref().unwrap_or("Server")),
        (_, Phase::Connecting) => "verbindet …".to_owned(),
        (_, Phase::Starting) => "startet …".to_owned(),
        _ => status.message.clone(),
    };
    let lock = if status.locked { " · Cursor gesperrt" } else { "" };
    format!("Qross {role} – {detail}{lock}")
}

/// Basis-Icon mit Statuspunkt unten links.
fn icon(status: &Status) -> Image<'static> {
    let base = Image::from_bytes(include_bytes!("../icons/32x32.png")).expect("eingebettetes Icon");
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    let color = match status.phase {
        Phase::Stopped => None,
        Phase::Error => Some([220, 38, 38]),
        Phase::Starting | Phase::Connecting => Some([234, 179, 8]),
        Phase::Listening if status.clients.is_empty() => Some([234, 179, 8]),
        Phase::Listening | Phase::Connected => Some([22, 163, 74]),
    };
    if let Some([r, g, b]) = color {
        // unten links, damit der Mauszeiger im Logo frei bleibt
        let (cx, cy, radius) = (8.0, h as f32 - 8.0, 7.5f32);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let i = ((y * w + x) * 4) as usize;
                if d <= radius - 1.5 {
                    rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
                } else if d <= radius {
                    // weißer Rand, damit der Punkt auf jedem Hintergrund sichtbar ist
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    Image::new_owned(rgba, w, h)
}
