mod config;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use glide_core::Layout;
use glide_net::client::{self, ClientConfig, ClientPlatform};
use glide_net::discovery;
use glide_net::server::{Server, ServerConfig, ServerPlatform};
use glide_net::{Fingerprint, Identity, TrustStore, KNOWN_SERVERS_FILE, TRUSTED_CLIENTS_FILE};
use glide_platform::dryrun::{LogInjector, MemoryClipboard};
use glide_proto::Rect;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::ServerFile;

/// So lange wird im LAN auf Antworten von Servern gewartet
const DISCOVERY_WAIT: Duration = Duration::from_millis(1200);

#[derive(Parser)]
#[command(name = "glide", version, about = "Maus und Tastatur über mehrere Rechner teilen")]
struct Cli {
    /// Verzeichnis für Identität und Vertrauenslisten (Standard: %APPDATA%\Glide)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Log in diese Datei schreiben statt auf die Konsole (z. B. beim Start als geplante Aufgabe)
    #[arg(long, global = true)]
    log_file: Option<PathBuf>,
    /// Ausführliches Log (überschreibbar per RUST_LOG)
    #[arg(long, short, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Server starten – auf dem Rechner, an dem Maus und Tastatur hängen
    Server {
        #[arg(long, short, default_value = "glide.toml")]
        config: PathBuf,
    },
    /// Diesen Rechner als Client mit einem Server verbinden
    Client {
        /// Server-Adresse: Rechnername, host:port oder IP. Ohne Angabe wird im LAN gesucht.
        server: Option<String>,
        /// Name dieses Rechners im Layout des Servers (Standard: Rechnername)
        #[arg(long)]
        name: Option<String>,
        /// Fingerprint des Servers fest vorgeben, statt ihn beim ersten Kontakt zu speichern
        #[arg(long)]
        server_fingerprint: Option<Fingerprint>,
        /// Eingaben nur protokollieren statt ausführen (Test auf einem einzigen Rechner)
        #[arg(long)]
        dry_run: bool,
    },
    /// Glide-Server im LAN suchen
    Discover,
    /// Fingerprint dieses Rechners anzeigen
    Fingerprint,
    /// Einen Client auf diesem Server freigeben
    Trust { name: String, fingerprint: Fingerprint },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let default_filter = if cli.verbose { "debug,quinn=warn,quinn_proto=warn,rustls=warn" } else { "info,quinn=warn" };
    let logging = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .with_target(false);
    match &cli.log_file {
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("Log-Datei {} öffnen", path.display()))?;
            logging.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
        }
        None => logging.init(),
    }

    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => dirs::config_dir().context("kein Konfigurationsverzeichnis gefunden")?.join("Glide"),
    };

    match cli.command {
        Command::Server { config } => run_server(&data_dir, &config).await,
        Command::Client { server, name, server_fingerprint, dry_run } => {
            let server = match server {
                Some(server) => server,
                None => find_server().await?,
            };
            let config = ClientConfig { name: name.unwrap_or_else(hostname), server, server_fingerprint };
            run_client(&data_dir, config, dry_run).await
        }
        Command::Discover => {
            let servers = discovery::discover(DISCOVERY_WAIT).await?;
            if servers.is_empty() {
                println!("Kein Glide-Server gefunden.");
            }
            for s in servers {
                println!("{:<20} {:<22} {:?}  {}", s.name, s.addr, s.platform, s.fingerprint);
            }
            Ok(())
        }
        Command::Fingerprint => {
            println!("{}", Identity::load_or_create(&data_dir)?.fingerprint());
            Ok(())
        }
        Command::Trust { name, fingerprint } => {
            let path = data_dir.join(TRUSTED_CLIENTS_FILE);
            TrustStore::load(&path)?.trust(&name, &fingerprint)?;
            println!("„{name}“ ist freigegeben ({}).", path.display());
            Ok(())
        }
    }
}

async fn run_server(data_dir: &Path, config_path: &Path) -> Result<()> {
    let file = ServerFile::load(config_path)?;
    let name = file.name.unwrap_or_else(hostname);
    let layout = Layout::from_links(&file.links)?;
    if !layout.contains(&name) {
        warn!("„{name}“ kommt in den links der Konfiguration nicht vor – der Cursor kann nirgendwohin wechseln");
    }
    let identity = Identity::load_or_create(data_dir)?;
    info!("Fingerprint dieses Servers: {}", identity.fingerprint());
    let trust = TrustStore::load(&data_dir.join(TRUSTED_CLIENTS_FILE))?;

    glide_platform::init_process();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = glide_platform::start_capture(events)?;
    info!("Notausgang: Strg+Alt+Shift+Esc holt Maus und Tastatur sofort zurück");

    let server = Server::bind(ServerConfig { name, listen: file.listen, layout }, &identity, trust)?
        .with_switch_options(file.switching.options())
        .with_discovery(file.discovery);
    let platform = ServerPlatform {
        capture,
        events: events_rx,
        clipboard: glide_platform::clipboard(),
        screen: Box::new(glide_platform::screen_bounds),
    };
    tokio::select! {
        result = server.run(platform) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("beendet");
            Ok(())
        }
    }
}

async fn run_client(data_dir: &Path, config: ClientConfig, dry_run: bool) -> Result<()> {
    let identity = Identity::load_or_create(data_dir)?;
    let fingerprint = identity.fingerprint();
    info!("Dieser Rechner: „{}“, Fingerprint {fingerprint}", config.name);
    info!("Freigabe auf dem Server (einmalig): glide trust {} {fingerprint}", config.name);
    let trust = TrustStore::load(&data_dir.join(KNOWN_SERVERS_FILE))?;

    glide_platform::init_process();
    let platform = if dry_run {
        info!("Dry-Run: Eingaben werden nur protokolliert");
        ClientPlatform {
            injector: Box::new(LogInjector),
            clipboard: Box::new(MemoryClipboard::default()),
            screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
        }
    } else {
        ClientPlatform {
            injector: glide_platform::injector()?,
            clipboard: glide_platform::clipboard(),
            screen: Box::new(glide_platform::screen_bounds),
        }
    };
    tokio::select! {
        result = client::run(config, identity, trust, platform) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("beendet");
            Ok(())
        }
    }
}

/// Genau einen Server im LAN finden, sonst mit Hinweis abbrechen.
async fn find_server() -> Result<String> {
    info!("suche Glide-Server im LAN …");
    let servers = discovery::discover(DISCOVERY_WAIT).await?;
    match servers.as_slice() {
        [server] => {
            info!("gefunden: „{}“ unter {}", server.name, server.addr);
            Ok(server.addr.to_string())
        }
        [] => bail!(
            "kein Glide-Server im LAN gefunden – Adresse angeben (glide client <server>) oder              prüfen, ob die Firewall des Servers UDP 24900–24901 erlaubt"
        ),
        many => {
            let list: Vec<String> = many.iter().map(|s| format!("„{}“ ({})", s.name, s.addr)).collect();
            bail!("mehrere Server gefunden: {} – bitte einen angeben", list.join(", "))
        }
    }
}

/// Rechnername in Kleinbuchstaben (Windows liefert ihn in Großbuchstaben).
fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().to_lowercase()
}
