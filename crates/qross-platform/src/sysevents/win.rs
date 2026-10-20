//! Windows: Ruhezustand über `PowerRegisterSuspendResumeNotification` mit Rückruf (geht ohne Fenster
//! und Nachrichtenschleife, auch in der Kommandozeile), Netzwerk über `NotifyIpInterfaceChange`
//! (Karten an/ab, Verbindung da/weg) und `NotifyUnicastIpAddressChange` (Adressen). Die Rückrufe
//! laufen auf Threads des Systems; die Anmeldungen gelten bis zum Prozessende.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use tracing::warn;
use windows_sys::Win32::Foundation::{HANDLE, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    MibInitialNotification, NotifyIpInterfaceChange, NotifyUnicastIpAddressChange, MIB_IPINTERFACE_ROW,
    MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW,
};
use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;
use windows_sys::Win32::System::Power::{PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS};
use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND};

use super::{dispatch, ready, SystemEvent, BEFORE_SLEEP};

pub fn start() {
    // Windows liest die Parameter womöglich erst später; sie leben deshalb bis zum Prozessende.
    let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: null_mut() }));
    let mut registration = null_mut();
    let result = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            (params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(),
            &mut registration,
        )
    };
    if result == NO_ERROR {
        ready(true);
    } else {
        warn!("not watching for sleep (PowerRegisterSuspendResumeNotification: error {result})");
    }

    // Die erste Meldung (`MibInitialNotification`) bestätigt nur die Anmeldung.
    let mut handle: HANDLE = null_mut();
    let result = unsafe { NotifyIpInterfaceChange(AF_UNSPEC, Some(on_interface), null(), true, &mut handle) };
    if result != NO_ERROR {
        warn!("not watching the network adapters (NotifyIpInterfaceChange: error {result})");
    }
    let mut handle: HANDLE = null_mut();
    let result = unsafe { NotifyUnicastIpAddressChange(AF_UNSPEC, Some(on_address), null(), true, &mut handle) };
    if result != NO_ERROR {
        warn!("not watching the network addresses (NotifyUnicastIpAddressChange: error {result})");
    }
}

unsafe extern "system" fn on_power(_context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
    match kind {
        PBT_APMSUSPEND => {
            dispatch(SystemEvent::Sleeping);
            // Windows wartet auf die Rückkehr (bis etwa 2 s); so lange können Verbindungen schließen.
            std::thread::sleep(BEFORE_SLEEP);
        }
        // Kommt bei jedem Aufwachen; `PBT_APMRESUMESUSPEND` folgt nur, wenn jemand davorsitzt.
        PBT_APMRESUMEAUTOMATIC => dispatch(SystemEvent::Woke),
        _ => {}
    }
    NO_ERROR
}

fn on_network(kind: MIB_NOTIFICATION_TYPE) {
    if kind == MibInitialNotification {
        ready(false);
    } else {
        dispatch(SystemEvent::NetworkChanged);
    }
}

unsafe extern "system" fn on_interface(_: *const c_void, _: *const MIB_IPINTERFACE_ROW, kind: MIB_NOTIFICATION_TYPE) {
    on_network(kind);
}

unsafe extern "system" fn on_address(_: *const c_void, _: *const MIB_UNICASTIPADDRESS_ROW, kind: MIB_NOTIFICATION_TYPE) {
    on_network(kind);
}
