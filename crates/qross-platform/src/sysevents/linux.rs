//! Linux: Ruhezustand über logind (Signal `PrepareForSleep` auf dem System-Bus) mit einer
//! Verzögerungssperre, damit vor dem Einschlafen noch Zeit zum Trennen bleibt; Netzwerk über ein
//! Netlink-Socket (Netzwerkkarten, Adressen, Routen – unabhängig von NetworkManager). Ohne logind
//! (Container, andere Init-Systeme) bleibt der Ruhezustand unbemerkt.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use ashpd::zbus;
use futures_util::StreamExt;
use tracing::{debug, info, warn};

use super::{dispatch, ready, SystemEvent, BEFORE_SLEEP};

pub fn start() {
    let netlink = std::thread::Builder::new().name("qross-netlink".into()).spawn(|| {
        if let Err(e) = watch_netlink() {
            warn!("not watching the network (netlink): {e}");
        }
    });
    let logind = std::thread::Builder::new().name("qross-logind".into()).spawn(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => return warn!("not watching for sleep: {e}"),
        };
        if let Err(e) = runtime.block_on(watch_logind()) {
            info!("not watching for sleep (logind not reachable: {e})");
        }
    });
    if let Err(e) = netlink.and(logind) {
        warn!("not watching system events: {e}");
    }
}

fn watch_netlink() -> io::Result<()> {
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_ROUTE) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
    addr.nl_groups = (libc::RTMGRP_LINK
        | libc::RTMGRP_IPV4_IFADDR
        | libc::RTMGRP_IPV6_IFADDR
        | libc::RTMGRP_IPV4_ROUTE
        | libc::RTMGRP_IPV6_ROUTE) as u32;
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            (&addr as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound < 0 {
        return Err(io::Error::last_os_error());
    }
    ready(false);
    // Der Inhalt ist egal: Jede Nachricht heißt „etwas hat sich geändert“.
    let mut buf = [0u8; 8192];
    loop {
        let n = unsafe { libc::recv(socket.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n >= 0 {
            dispatch(SystemEvent::NetworkChanged);
            continue;
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => {}
            // Zu viele Meldungen auf einmal: Es hat sich also etwas geändert.
            Some(libc::ENOBUFS) => dispatch(SystemEvent::NetworkChanged),
            _ => return Err(e),
        }
    }
}

async fn watch_logind() -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let manager = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let mut signals = manager.receive_signal("PrepareForSleep").await?;
    let mut lock = inhibit(&manager).await;
    ready(true);
    while let Some(message) = signals.next().await {
        let Ok(going) = message.body().deserialize::<bool>() else { continue };
        if going {
            dispatch(SystemEvent::Sleeping);
            tokio::time::sleep(BEFORE_SLEEP).await;
            // Sperre freigeben: Jetzt darf logind einschlafen.
            drop(lock.take());
        } else {
            dispatch(SystemEvent::Woke);
            lock = inhibit(&manager).await;
        }
    }
    Ok(())
}

/// Verzögerungssperre: logind wartet vor dem Einschlafen, bis sie freigegeben ist (höchstens
/// `InhibitDelayMaxSec`, üblich 5 s). Ohne sie käme `Sleeping` womöglich zu spät an.
async fn inhibit(manager: &zbus::Proxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
    let why = "Close connections cleanly before going to sleep";
    match manager.call("Inhibit", &("sleep", "Qross", why, "delay")).await {
        Ok(fd) => Some(fd),
        Err(e) => {
            debug!("no delay inhibitor from logind: {e}");
            None
        }
    }
}
