#![windows_subsystem = "windows"]

mod backend;
mod client;
mod config;
mod discovery;
mod host;
mod identity;
mod ipc;
mod logging;
mod net;
mod pairing;
mod service;
mod ui;
mod win;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use eframe::egui;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

use crate::service::Shared;
use crate::win::{elevation, firewall, spooler};

#[derive(Parser, Debug)]
#[command(name = "peerPrint", version, about = "Share USB printers over Wi-Fi")]
struct Cli {
    /// Start quietly in the background (used by the login entry).
    #[arg(long, hide = true)]
    autostart: bool,
    /// Run without opening a window.
    #[arg(long)]
    background: bool,
    /// Elevated helper: create a client print queue from a spec file.
    #[arg(long, value_name = "SPEC", hide = true)]
    install_client: Option<String>,
    /// Elevated helper: remove a client print queue from a spec file.
    #[arg(long, value_name = "SPEC", hide = true)]
    uninstall_client: Option<String>,
    /// Elevated helper: remove every peerPrint queue and port.
    #[arg(long, value_name = "SPEC", hide = true)]
    uninstall_all_worker: Option<String>,
    /// Remove all peerPrint printers, startup entries and settings (used by the uninstaller).
    #[arg(long, hide = true)]
    uninstall_all: bool,
    /// Ask a running peerPrint to quit.
    #[arg(long, hide = true)]
    quit: bool,
    /// Elevation helper: add firewall rules.
    #[arg(long, hide = true)]
    firewall_setup: bool,
    /// Print diagnostics as JSON and exit.
    #[arg(long)]
    diagnose: bool,
    /// Send a file to a local Windows queue as RAW data.
    #[arg(long, value_name = "PRINTER")]
    print_raw: Option<String>,
    /// File to send with --print-raw.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Listen on a local TCP port and save the bytes a printer port sends.
    #[arg(long, value_name = "PORT")]
    capture_port: Option<u16>,
    /// Where --capture-port saves the captured job.
    #[arg(long, value_name = "FILE")]
    capture_out: Option<PathBuf>,
    /// Use machine-wide state in ProgramData (all-users installation).
    #[arg(long, hide = true)]
    machine_wide: bool,
    /// Run as the peerPrint Windows service (started by the service manager).
    #[arg(long, hide = true)]
    service: bool,
    /// Run the headless engine in this process (for testing without the SCM).
    #[arg(long, hide = true)]
    engine: bool,
    /// Store the install role: host, client or both.
    #[arg(long, value_name = "ROLE", hide = true)]
    set_role: Option<String>,
    /// Register and start the peerPrint Windows service (requires admin).
    #[arg(long, hide = true)]
    install_service: bool,
    /// Stop and remove the peerPrint Windows service (requires admin).
    #[arg(long, hide = true)]
    uninstall_service: bool,
    /// Verbose logging.
    #[arg(long)]
    verbose: bool,
}

/// The binary is GUI-subsystem so the tray UI never flashes a console window.
/// When it is instead started from a terminal, attach to that terminal so
/// `--diagnose`, `--quit`, `--install-service` and clap output stay visible.
fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> Result<()> {
    attach_parent_console();
    let cli = Cli::parse();

    if cli.machine_wide || cli.service || cli.engine || cli.install_service || cli.uninstall_service {
        config::set_machine_wide(true);
        win::dpapi::set_machine_scope(true);
    }

    if let Some(spec) = cli.install_client.as_deref() {
        std::process::exit(client::run_install_worker(spec));
    }
    if let Some(spec) = cli.uninstall_client.as_deref() {
        std::process::exit(client::run_uninstall_worker(spec));
    }
    if let Some(spec) = cli.uninstall_all_worker.as_deref() {
        std::process::exit(client::run_uninstall_all_worker(spec));
    }

    let _log_guard = logging::init(cli.verbose);

    if cli.quit {
        return match ipc::send_control("quit") {
            Ok(_) => {
                println!("Asked peerPrint to quit.");
                Ok(())
            }
            Err(_) => {
                println!("peerPrint is not running.");
                Ok(())
            }
        };
    }

    if cli.uninstall_all {
        return client::uninstall_all();
    }

    if cli.install_service {
        win::service::install()?;
        return Ok(());
    }
    if cli.uninstall_service {
        win::service::uninstall()?;
        return Ok(());
    }

    if let Some(role) = cli.set_role.as_deref() {
        let role = config::Role::parse(role)
            .with_context(|| format!("unknown role '{role}' (use host, client or both)"))?;
        config::ensure_dirs()?;
        let mut cfg = config::AppConfig::load();
        cfg.role = role;
        cfg.save()?;
        println!("peerPrint is set up as {role:?}.");
        return Ok(());
    }

    if cli.firewall_setup {
        return firewall_setup();
    }

    if cli.service {
        return win::service::run();
    }

    if let Some(printer) = cli.print_raw.as_deref() {
        let path = cli
            .input
            .as_ref()
            .context("--print-raw needs --input <file>")?;
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let job = spooler::print_raw(printer, &data, "peerPrint RAW test")?;
        println!(
            "sent {} bytes to '{}' as Windows job {job} (sha256 {})",
            data.len(),
            printer,
            hex::encode(Sha256::digest(&data))
        );
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("peerprint")
        .build()
        .context("starting async runtime")?;

    if let Some(port) = cli.capture_port {
        return runtime.block_on(capture_port(port, cli.capture_out.clone()));
    }

    if cli.diagnose {
        if config::is_machine_wide() {
            let snapshot = backend::fetch_snapshot(runtime.handle())?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            return Ok(());
        }
        let shared = {
            let _guard = runtime.enter();
            Shared::new(runtime.handle().clone()).context("loading peerPrint state")?
        };
        print_diagnostics(&shared);
        return Ok(());
    }

    if cli.engine {
        let shared = {
            let _guard = runtime.enter();
            Shared::new(runtime.handle().clone()).context("loading peerPrint state")?
        };
        runtime.block_on(service::start_services(shared.clone(), false, true));
        client::resume_all_queues(&shared);
        println!("peerPrint engine is running. Press Ctrl+C to stop.");
        runtime.block_on(async {
            let mut shutdown = shared.shutdown_tx.subscribe();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = shutdown.changed() => {}
            }
        });
        service::shutdown(&shared);
        client::pause_all_queues(&shared);
        runtime.shutdown_timeout(Duration::from_secs(2));
        return Ok(());
    }

    let result;
    if cli.machine_wide {
        let _instance = match win::single_instance::acquire(&ipc::instance_name()) {
            Some(guard) => guard,
            None => {
                let show = !cli.autostart;
                match ipc::notify_running_instance(show) {
                    Ok("background") => println!("peerPrint is already running in the background."),
                    Ok(_) => println!("peerPrint is already running."),
                    Err(err) => {
                        eprintln!("peerPrint is already running; could not reach it: {err}")
                    }
                }
                return Ok(());
            }
        };
        ipc::start_ui_control_server(runtime.handle());
        let backend = backend::Backend::remote(runtime.handle().clone());
        result = run_gui(backend, cli.autostart);
    } else {
        let _instance = match win::single_instance::acquire(&ipc::instance_name()) {
            Some(guard) => guard,
            None => {
                let show = !cli.autostart && !cli.background;
                match ipc::notify_running_instance(show) {
                    Ok("background") => println!("peerPrint is already running in the background."),
                    Ok(_) => println!("peerPrint is already running."),
                    Err(err) => {
                        eprintln!("peerPrint is already running; could not reach it: {err}")
                    }
                }
                return Ok(());
            }
        };

        let shared = {
            let _guard = runtime.enter();
            Shared::new(runtime.handle().clone()).context("loading peerPrint state")?
        };

        let cfg = shared.config();
        if cfg.autostart {
            if !win::autostart::is_current() {
                if let Err(err) = win::autostart::set_enabled(true) {
                    tracing::warn!("adding the startup entry failed: {err:#}");
                }
            }
        } else if win::autostart::is_enabled() {
            if let Err(err) = win::autostart::set_enabled(false) {
                tracing::warn!("removing the startup entry failed: {err:#}");
            }
        }

        let interactive = !cli.background;
        runtime.block_on(service::start_services(shared.clone(), interactive, false));

        if cli.background {
            client::resume_all_queues(&shared);
            println!("peerPrint is running in the background. Press Ctrl+C to stop.");
            runtime.block_on(async {
                let mut shutdown = shared.shutdown_tx.subscribe();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = shutdown.changed() => {}
                }
            });
            service::shutdown(&shared);
            client::pause_all_queues(&shared);
            runtime.shutdown_timeout(Duration::from_secs(2));
            return Ok(());
        }

        client::resume_all_queues(&shared);
        let backend = backend::Backend::local(shared.clone());
        result = run_gui(backend, cli.autostart);
        service::shutdown(&shared);
        client::pause_all_queues(&shared);
    }

    runtime.shutdown_timeout(Duration::from_secs(2));
    result.map_err(|err| anyhow::anyhow!("{err}"))
}

fn run_gui(
    backend: std::sync::Arc<backend::Backend>,
    start_hidden: bool,
) -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/peerprint-256.png")).ok();
    eframe::run_native(
        "peerPrint",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1000.0, 720.0])
                .with_min_inner_size([820.0, 600.0])
                .with_icon(icon.unwrap_or_default())
                // The UI draws its own title bar; the native title and icon stay
                // so the taskbar and its previews still show the app identity.
                .with_title("peerPrint")
                .with_decorations(false)
                .with_visible(!start_hidden),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            ui::theme::install(&cc.egui_ctx);
            Ok(Box::new(ui::App::new(backend, start_hidden)))
        }),
    )
}

fn firewall_setup() -> Result<()> {
    if !elevation::is_elevated() {
        println!("Requesting administrator permission to add firewall rules...");
        let code = elevation::run_elevated("--firewall-setup")?;
        if code != 0 {
            bail!("firewall setup failed (exit code {code})");
        }
        println!("Firewall rules are in place.");
        return Ok(());
    }
    let config = config::AppConfig::load();
    firewall::ensure_rules(config.host.listen_port)?;
    let mut config = config;
    config.host.firewall_configured = true;
    config.save()?;
    println!("Added private-network firewall rules for TCP {} and mDNS.", config.host.listen_port);
    Ok(())
}

async fn capture_port(port: u16, out: Option<PathBuf>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("listening on 127.0.0.1:{port}"))?;
    println!("Capturing on 127.0.0.1:{port}. Print to that port, then press Ctrl+C.");
    loop {
        let (mut socket, peer) = listener.accept().await?;
        let mut data = Vec::new();
        match tokio::time::timeout(Duration::from_secs(300), socket.read_to_end(&mut data)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                eprintln!("capture read failed: {err}");
                continue;
            }
            Err(_) => {
                eprintln!("capture timed out");
                continue;
            }
        }
        let path = out.clone().unwrap_or_else(|| {
            config::data_dir().join(format!("capture-{}.bin", config::now_unix()))
        });
        std::fs::write(&path, &data)?;
        let digest = hex::encode(Sha256::digest(&data));
        println!(
            "captured {} bytes from {peer} -> {} sha256={digest}",
            data.len(),
            path.display()
        );
        let _ = std::io::stdout().flush();
    }
}

fn print_diagnostics(shared: &Shared) {
    let config = shared.config();
    let core = shared.core();
    let saved: Vec<_> = config
        .client
        .saved
        .iter()
        .map(|printer| {
            serde_json::json!({
                "queue": printer.queue_name,
                "host": printer.host_display,
                "host_addr": printer.host_addr,
                "relay_port": printer.relay_port,
                "installed": printer.installed,
                "driver": printer.driver_name,
            })
        })
        .collect();
    let printers: Vec<_> = core
        .local_printers
        .iter()
        .map(|printer| {
            serde_json::json!({
                "name": printer.name,
                "port": printer.port_name,
                "driver": printer.driver_name,
                "driver_env": printer.driver_env,
                "status": printer.status_text,
                "shareable": printer.shareable,
            })
        })
        .collect();
    let report = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "data_dir": config::data_dir(),
        "status_file": ipc::status_path(),
        "host_id": shared.identity.host_id,
        "tls_fingerprint": hex::encode(shared.identity.fingerprint),
        "host": {
            "enabled": config.host.enabled,
            "display_name": config.host.display_name,
            "listen_port": config.host.listen_port,
            "pin_required": config.host.require_pin,
            "pin_set": config.host.pin_dpapi.is_some(),
            "firewall_configured": config.host.firewall_configured,
            "running": core.host_running,
            "error": core.host_error,
        },
        "local_printers": printers,
        "saved_printers": saved,
        "relays": core.relays.iter().map(|relay| serde_json::json!({
            "queue": relay.queue_name,
            "port": relay.port,
            "listening": relay.listening,
            "last_error": relay.last_error,
            "last_job": relay.last_job,
        })).collect::<Vec<_>>(),
        "firewall_rules": firewall::has_rules(),
        "protected_print_mode": spooler::protected_print_mode(),
        "elevated": elevation::is_elevated(),
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
}

