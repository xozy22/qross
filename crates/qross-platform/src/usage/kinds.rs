//! Welche Prozesse zur Web-Oberfläche von Qross gehören und welcher Art sie sind, nach Name und Kommandozeile;
//! ohne Systemaufrufe und deshalb überall getestet

use qross_proto::UsageKind;

/// Windows: Ist das Programm (Dateiname aus der Prozessliste) WebView2?
#[cfg(any(windows, test))]
pub(crate) fn is_webview2(program: &str) -> bool {
    program.eq_ignore_ascii_case("msedgewebview2.exe")
}

/// Windows: Art eines Prozesses von WebView2 nach seiner Kommandozeile. Ohne `--type` ist es der Browser-Prozess,
/// der die anderen startet; Hilfsdienste (`--type=utility`) nennen ihren Dienst in `--utility-sub-type`.
#[cfg(any(windows, test))]
pub(crate) fn webview2_kind(command_line: &str) -> UsageKind {
    let (mut kind, mut service) = (None, String::new());
    for argument in arguments(command_line) {
        if let Some(value) = argument.strip_prefix("--type=") {
            kind = Some(value.to_owned());
        } else if let Some(value) = argument.strip_prefix("--utility-sub-type=") {
            service = value.to_owned();
        }
    }
    match kind.as_deref() {
        None => UsageKind::Browser,
        Some("renderer") => UsageKind::Page,
        Some("gpu-process") => UsageKind::Graphics,
        Some("utility") if service.starts_with("network.") => UsageKind::Network,
        // Speicher und weitere Dienste, der Absturzmelder (`crashpad-handler`)
        Some(_) => UsageKind::Other,
    }
}

/// Die Argumente einer Kommandozeile unter Windows: getrennt durch Leerraum, außer in Anführungszeichen (etwa im
/// Pfad des Programms); die Anführungszeichen selbst fallen weg
#[cfg(any(windows, test))]
fn arguments(command_line: &str) -> impl Iterator<Item = String> + '_ {
    let mut chars = command_line.chars().peekable();
    std::iter::from_fn(move || {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        chars.peek()?;
        let (mut argument, mut quoted) = (String::new(), false);
        while let Some(c) = chars.next_if(|&c| quoted || !c.is_whitespace()) {
            if c == '"' {
                quoted = !quoted;
            } else {
                argument.push(c);
            }
        }
        Some(argument)
    })
}

/// macOS: Art eines Dienstes von WebKit nach dem Namen seines Programms (etwa `com.apple.WebKit.WebContent`);
/// `None`, wenn es keiner ist
#[cfg(any(target_os = "macos", test))]
pub(crate) fn webkit_service_kind(program: &str) -> Option<UsageKind> {
    let service = program.strip_prefix("com.apple.WebKit.")?;
    Some(if service.starts_with("WebContent") {
        UsageKind::Page
    } else if service.starts_with("GPU") {
        UsageKind::Graphics
    } else if service.starts_with("Networking") {
        UsageKind::Network
    } else {
        UsageKind::Other
    })
}

/// Linux: Art eines Prozesses von WebKitGTK nach seinem Namen in `/proc/<pid>/stat`, der nach 15 Zeichen
/// abgeschnitten ist (etwa `WebKitWebProces`); `None`, wenn er nicht dazugehört
#[cfg(any(target_os = "linux", test))]
pub(crate) fn webkitgtk_kind(name: &str) -> Option<UsageKind> {
    let process = name.strip_prefix("WebKit")?;
    Some(if process.starts_with("WebProc") {
        UsageKind::Page
    } else if process.starts_with("GPU") {
        UsageKind::Graphics
    } else if process.starts_with("Network") {
        UsageKind::Network
    } else {
        UsageKind::Other
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gekürzt aus einer echten Sitzung (WebView2 141)
    const EXE: &str = r#""C:\Program Files (x86)\Microsoft\EdgeWebView\Application\141.0.3537.71\msedgewebview2.exe""#;

    #[test]
    fn webview2_processes_by_their_command_line() {
        assert!(is_webview2("msedgewebview2.exe") && is_webview2("MSEdgeWebView2.EXE"));
        assert!(!is_webview2("msedge.exe") && !is_webview2("qross-gui.exe") && !is_webview2("explorer.exe"));

        let line = |args: &str| format!("{EXE} {args}");
        let data = r#"--user-data-dir="C:\Users\Dennis\AppData\Local\com.qross.app\EBWebView""#;
        let cases = [
            (line(&format!("--embedded-browser-webview=1 --webview-exe-name=qross-gui.exe {data} --noerrdialogs")), UsageKind::Browser),
            (line(&format!("--type=renderer --noerrdialogs {data} --lang=de --renderer-client-id=5 /prefetch:1")), UsageKind::Page),
            (line(&format!("--type=gpu-process --noerrdialogs {data} --gpu-preferences=UAAAAAAAAADgAAAE --mojo-platform-channel-handle=1932")), UsageKind::Graphics),
            (line(&format!("--type=utility --utility-sub-type=network.mojom.NetworkService --lang=de --service-sandbox-type=none {data} /prefetch:3")), UsageKind::Network),
            (line(&format!("--type=utility --utility-sub-type=storage.mojom.StorageService --lang=de --service-sandbox-type=service {data} /prefetch:8")), UsageKind::Other),
            (line(&format!("--type=crashpad-handler {data} /prefetch:4 --monitor-self-annotation=ptype=crashpad-handler")), UsageKind::Other),
            (line("--type=utility --utility-sub-type=audio.mojom.AudioService"), UsageKind::Other),
            (EXE.to_owned(), UsageKind::Browser),
        ];
        for (command_line, kind) in cases {
            assert_eq!(webview2_kind(&command_line), kind, "{command_line}");
        }
        // Was in Anführungszeichen steht, ist ein einziges Argument, auch wenn es wie eine Option aussieht.
        let tricky = line(r#""--user-data-dir=C:\Daten --type=renderer\EBWebView" --noerrdialogs"#);
        assert_eq!(webview2_kind(&tricky), UsageKind::Browser);
        assert_eq!(arguments(r#"  "a b"  c"d e"f  "#).collect::<Vec<_>>(), ["a b", "cd ef"]);
        assert_eq!(arguments("").count(), 0);
    }

    #[test]
    fn webkit_services_by_their_program() {
        let cases = [
            ("com.apple.WebKit.WebContent", Some(UsageKind::Page)),
            ("com.apple.WebKit.WebContent.CaptivePortal", Some(UsageKind::Page)),
            ("com.apple.WebKit.GPU", Some(UsageKind::Graphics)),
            ("com.apple.WebKit.Networking", Some(UsageKind::Network)),
            ("com.apple.WebKit.WebAuthn", Some(UsageKind::Other)),
            // Was Qross selbst startet, etwa für einen Link oder die Netzwerkerkennung
            ("open", None),
            ("netstat", None),
            ("Safari", None),
            ("com.apple.Safari.SafeBrowsing.Service", None),
        ];
        for (program, kind) in cases {
            assert_eq!(webkit_service_kind(program), kind, "{program}");
        }
    }

    #[test]
    fn webkitgtk_processes_by_their_name() {
        // Wie in `/proc/<pid>/stat`: nach 15 Zeichen abgeschnitten
        let cases = [
            ("WebKitWebProces", Some(UsageKind::Page)),
            ("WebKitGPUProces", Some(UsageKind::Graphics)),
            ("WebKitNetworkPr", Some(UsageKind::Network)),
            ("WebKitPluginPro", Some(UsageKind::Other)),
            ("xdg-open", None),
            ("firefox", None),
            ("bwrap", None),
        ];
        for (name, kind) in cases {
            assert_eq!(webkitgtk_kind(name), kind, "{name}");
        }
    }
}
