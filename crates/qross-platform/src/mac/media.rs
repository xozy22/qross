//! Medientasten unter macOS. Lautstärke und Wiedergabe sind dort keine normalen Tastenereignisse,
//! sondern Systemereignisse (`NX_SYSDEFINED`, Untertyp 8) mit dem Tastencode in `data1`.
//! Nur so reagieren Systemlautstärke und Musik-Apps darauf.

use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_core_graphics::{CGEvent, CGEventTapLocation};
use objc2_foundation::NSPoint;
use qross_proto::KeyCode;

/// Untertyp „Zusatztasten“ von `NX_SYSDEFINED`
const AUX_CONTROL_BUTTONS: i16 = 8;

/// `NX_KEYTYPE_*` aus `IOKit/hidsystem/ev_keymap.h`. Eine Stopp-Taste kennt macOS nicht.
fn nx_key(key: KeyCode) -> Option<isize> {
    Some(match key {
        KeyCode::VOLUME_UP => 0,
        KeyCode::VOLUME_DOWN => 1,
        KeyCode::MUTE => 7,
        KeyCode::MEDIA_PLAY_PAUSE => 16,
        KeyCode::MEDIA_NEXT => 17,
        KeyCode::MEDIA_PREVIOUS => 18,
        _ => return None,
    })
}

/// Speist eine Medientaste ein. `false`, wenn `key` keine ist (dann als normale Taste behandeln).
pub fn post(key: KeyCode, down: bool) -> bool {
    if !key.is_media() {
        return false;
    }
    let Some(nx) = nx_key(key) else {
        tracing::debug!(?key, "media key without a macOS counterpart");
        return true;
    };
    let state: isize = if down { 0xA } else { 0xB };
    let flags = NSEventModifierFlags::from_bits_retain(if down { 0xA00 } else { 0xB00 });
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::SystemDefined,
        NSPoint::new(0.0, 0.0),
        flags,
        0.0,
        0,
        None,
        AUX_CONTROL_BUTTONS,
        (nx << 16) | (state << 8),
        -1,
    );
    match event.and_then(|e| e.CGEvent()) {
        Some(cg) => CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&cg)),
        None => tracing::warn!(?key, "failed to create the media key event"),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_keys_have_nx_codes() {
        for key in [KeyCode::VOLUME_UP, KeyCode::VOLUME_DOWN, KeyCode::MUTE, KeyCode::MEDIA_PLAY_PAUSE, KeyCode::MEDIA_NEXT, KeyCode::MEDIA_PREVIOUS] {
            assert!(nx_key(key).is_some(), "{key:?}");
        }
        assert_eq!(nx_key(KeyCode::MEDIA_STOP), None);
        assert_eq!(nx_key(KeyCode::A), None);
    }
}
