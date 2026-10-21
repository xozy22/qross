mod config;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use qross_core::Layout;
use qross_net::client::{self, ClientConfig, ClientPlatform};
use qross_net::discovery;
use qross_net::server::{Server, ServerConfig, ServerPlatform};
use qross_net::{Fingerprint, Identity, TrustStore, KNOWN_SERVERS_FILE, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{LogInjector, MemoryClipboard};
use qross_proto::{GameState, Rect};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::ServerFile;

/// So lange wird im LAN auf Antworten von Servern gewartet
const DISCOVERY_WAIT: Duration = Duration::from_millis(1200);

// Die Doc-Kommentare von `Cli` und `Command` sind die Hilfetexte der Kommandozeile und deshalb englisch.
#[derive(Parser)]
#[command(name = "qross", version, about = "Share mouse and keyboard across several computers")]
struct Cli {
    /// Directory for the identity and trust lists (default: %APPDATA%\Qross)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Write the log to this file instead of the console (e.g. when started as a scheduled task)
    #[arg(long, global = true)]
    log_file: Option<PathBuf>,
    /// Verbose log (can be overridden with RUST_LOG)
    #[arg(long, short, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server – on the computer that mouse and keyboard are connected to
    Server {
        #[arg(long, short, default_value = "qross.toml")]
        config: PathBuf,
    },
    /// Connect this computer to a server as a client
    Client {
        /// Server address: host name, host:port or IP. Without it, the LAN is searched.
        server: Option<String>,
        /// Name of this computer in the server's layout (default: host name)
        #[arg(long)]
        name: Option<String>,
        /// Pin the server's fingerprint instead of saving it on first contact
        #[arg(long)]
        server_fingerprint: Option<Fingerprint>,
        /// Send copied files to the server up to this total size in MB (0 = never)
        #[arg(long, value_name = "MB", default_value_t = config::default_max_file_mb())]
        clipboard_max_mb: u64,
        /// Send content that a password manager marks as confidential to the server anyway
        #[arg(long)]
        share_concealed: bool,
        /// Only log input instead of performing it (for testing on a single computer)
        #[arg(long)]
        dry_run: bool,
        /// Game mode off: never tell the server that a full-screen program is running here
        #[arg(long)]
        no_game_mode: bool,
    },
    /// Search the LAN for Qross servers
    Discover,
    /// Show the fingerprint of this computer
    Fingerprint,
    /// Trust a client on this server
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
                .with_context(|| format!("failed to open log file {}", path.display()))?;
            logging.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
        }
        None => logging.init(),
    }

    let data_dir = match cli.data_dir {
        Some(dir) => dir,
        None => dirs::config_dir().context("no configuration directory found")?.join("Qross"),
    };
    // Dort merkt sich Qross unter Wayland die Freigaben der Desktop-Portale
    qross_platform::set_data_dir(&data_dir);

    match cli.command {
        Command::Server { config } => run_server(&data_dir, &config).await,
        Command::Client { server, name, server_fingerprint, clipboard_max_mb, share_concealed, dry_run, no_game_mode } => {
            let server = match server {
                Some(server) => server,
                None => find_server().await?,
            };
            let config = ClientConfig {
                name: name.unwrap_or_else(hostname),
                server,
                server_fingerprint,
                clipboard_file_limit: config::mb_to_bytes(clipboard_max_mb),
                share_concealed,
            };
            run_client(&data_dir, config, dry_run, !no_game_mode).await
        }
        Command::Discover => {
            let servers = discovery::discover(DISCOVERY_WAIT).await?;
            if servers.is_empty() {
                println!("No Qross server found.");
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
            println!("'{name}' is now trusted ({}).", path.display());
            Ok(())
        }
    }
}

async fn run_server(data_dir: &Path, config_path: &Path) -> Result<()> {
    let file = ServerFile::load(config_path)?;
    let name = file.name.unwrap_or_else(hostname);
    let layout = Layout::from_links(&file.links)?;
    if !layout.contains(&name) {
        warn!("'{name}' does not appear in the links of the configuration – the cursor cannot switch anywhere");
    }
    let identity = Identity::load_or_create(data_dir)?;
    info!("fingerprint of this server: {}", identity.fingerprint());
    let trust = TrustStore::load(&data_dir.join(TRUSTED_CLIENTS_FILE))?;

    qross_platform::init_process();
    // Bis zum Ende des Laufs: Das System bremst den Server nicht aus (macOS: App Nap aus)
    let _responsive = qross_platform::keep_responsive();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = qross_platform::start_capture(events)?;
    info!("emergency exit: Ctrl+Alt+Shift+Esc brings mouse and keyboard back immediately");
    for (hotkey, target) in file.keys.config()?.options.switch {
        info!("shortcut: {} switches {}", hotkey.label(cfg!(target_os = "macos")), target.label());
    }
    if let Some(lock) = file.keys.config()?.options.lock {
        info!("lock key: {} keeps the cursor on the current screen", lock.label(cfg!(target_os = "macos")));
    }
    if file.game.enabled {
        info!("game mode: with full-screen programs, the cursor stays on the computer they run on");
    }
    let finder = qross_platform::finder_overlay();
    if file.finder.style().is_on() && finder.is_none() {
        info!("cursor finder: not available here (Wayland), clients show it anyway");
    }
    match (file.screensaver.sync, file.screensaver.lock_clients) {
        (true, false) => info!("screen saver: clients start and stop theirs along with this one"),
        (true, true) => info!("screen saver: clients lock themselves when it starts here"),
        (false, _) => {}
    }
    let sharing = file.sharing.rules();
    if sharing.default != qross_core::sharing::Sharing::ALL {
        info!("permissions for computers without their own entry: {}", qross_core::sharing::describe(sharing.default));
    }

    let server = Server::bind(ServerConfig { name, listen: file.listen, layout }, &identity, trust)?
        .with_switch_options(file.switching.options())
        .with_discovery(file.discovery)
        .with_clipboard_file_limit(file.clipboard.file_limit())
        .with_share_concealed(file.clipboard.share_concealed)
        .with_keys(file.keys.config()?)
        .with_game_mode(file.game.enabled)
        .with_screen_saver(file.screensaver.sync, file.screensaver.lock_clients)
        .with_sharing(sharing)
        .with_cursor_finder(file.finder.style());
    // Hält ein Programm die sichere Tastatureingabe, bleibt der Cursor auf dem Mac (steht im Log)
    #[cfg(target_os = "macos")]
    let server = server.with_secure_input(Box::new(qross_platform::secure_input));
    // Liegt ein Fenster mit Administratorrechten im Weg, etwa der Task-Manager, wechselt der Cursor
    // nicht zu Clients (steht im Log)
    #[cfg(windows)]
    let server = server.with_elevated_windows(qross_net::elevated::Detector::system());
    let platform = ServerPlatform {
        capture,
        events: events_rx,
        clipboard: qross_platform::clipboard(),
        screen: Box::new(qross_platform::screen_bounds),
        monitors: Box::new(qross_platform::monitors),
        game: Box::new(qross_platform::game_state),
        finder,
    };
    tokio::select! {
        result = server.run(platform) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("stopped");
            Ok(())
        }
    }
}

async fn run_client(data_dir: &Path, config: ClientConfig, dry_run: bool, game_mode: bool) -> Result<()> {
    let identity = Identity::load_or_create(data_dir)?;
    let fingerprint = identity.fingerprint();
    info!("this computer: '{}', fingerprint {fingerprint}", config.name);
    info!("to approve this computer on the server (once): qross trust {} {fingerprint}", config.name);
    let trust = TrustStore::load(&data_dir.join(KNOWN_SERVERS_FILE))?;

    qross_platform::init_process();
    // Bis zum Ende des Laufs: Das System bremst den Client nicht aus (macOS: App Nap aus)
    let _responsive = qross_platform::keep_responsive();
    let platform = if dry_run {
        info!("dry run: input is only logged");
        ClientPlatform {
            injector: Box::new(LogInjector),
            clipboard: Box::new(MemoryClipboard::default()),
            screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
            finder: None,
        }
    } else {
        ClientPlatform {
            injector: qross_platform::injector()?,
            clipboard: qross_platform::clipboard(),
            screen: Box::new(qross_platform::screen_bounds),
            monitors: Box::new(qross_platform::monitors),
            game: if game_mode { Box::new(qross_platform::game_state) } else { Box::new(GameState::default) },
            finder: qross_platform::finder_overlay(),
        }
    };
    tokio::select! {
        result = client::run(config, identity, trust, platform) => result,
        _ = tokio::signal::ctrl_c() => {
            info!("stopped");
            Ok(())
        }
    }
}

/// Genau einen Server im LAN finden, sonst mit Hinweis abbrechen.
async fn find_server() -> Result<String> {
    info!("searching the LAN for Qross servers …");
    let servers = discovery::discover(DISCOVERY_WAIT).await?;
    match servers.as_slice() {
        [server] => {
            info!("found '{}' at {}", server.name, server.addr);
            Ok(server.addr.to_string())
        }
        [] => bail!(
            "no Qross server found in the LAN – specify its address (qross client <server>) or check that the \
             server's firewall allows UDP 24900–24901"
        ),
        many => {
            let list: Vec<String> = many.iter().map(|s| format!("'{}' ({})", s.name, s.addr)).collect();
            bail!("several servers found: {} – please specify one", list.join(", "))
        }
    }
}

/// Rechnername in Kleinbuchstaben (Windows liefert ihn in Großbuchstaben).
fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().to_lowercase()
}
