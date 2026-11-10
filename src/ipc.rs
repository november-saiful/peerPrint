use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config;
use crate::service::Shared;

/// Bumped by the control pipe and by the tray so the UI can raise its window.
static ACTIVATIONS: AtomicU64 = AtomicU64::new(0);
/// Set when another process asks this one to shut down.
static QUIT_REQUESTS: AtomicU64 = AtomicU64::new(0);

pub fn user_key() -> String {
    let key: String = std::env::var("USERNAME")
        .unwrap_or_else(|_| "local".to_string())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if key.is_empty() {
        "local".to_string()
    } else {
        key
    }
}

pub fn instance_name() -> String {
    format!("Local\\peerPrint-{}", user_key())
}

fn pipe_name() -> String {
    format!("peerprint-control-{}", user_key())
}

fn pipe_path() -> String {
    format!(r"\\.\pipe\{}", pipe_name())
}

pub fn request_show() {
    ACTIVATIONS.fetch_add(1, Ordering::SeqCst);
}

/// Returns true once per activation request.
pub fn take_activation() -> bool {
    ACTIVATIONS.swap(0, Ordering::SeqCst) > 0
}

pub fn request_quit() {
    QUIT_REQUESTS.fetch_add(1, Ordering::SeqCst);
}

/// Returns true once per shutdown request.
pub fn take_quit() -> bool {
    QUIT_REQUESTS.swap(0, Ordering::SeqCst) > 0
}

/// Sends a command to the running instance. Commands: "show", "ping", "quit".
pub fn send_control(command: &str) -> Result<&'static str> {
    let path = pipe_path();
    let mut last_error = None;
    for _ in 0..20 {
        match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(mut pipe) => {
                let _ = pipe.write_all(command.as_bytes());
                let _ = pipe.write_all(b"\n");
                let _ = pipe.flush();
                let mut buf = [0u8; 32];
                let read = pipe.read(&mut buf).unwrap_or(0);
                let reply = String::from_utf8_lossy(&buf[..read]);
                return Ok(match reply.trim() {
                    "shown" => "shown",
                    "background" => "background",
                    "quitting" => "quitting",
                    _ => "ok",
                });
            }
            Err(err) => {
                last_error = Some(err);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| std::io::Error::other("control pipe unavailable"))
        .into())
}

/// Asks the running instance to show its window (`show`) or just report its mode.
pub fn notify_running_instance(show: bool) -> Result<&'static str> {
    send_control(if show { "show" } else { "ping" })
}

pub fn start_control_server(shared: Arc<Shared>, interactive: bool) {
    let path = pipe_path();
    let runtime = shared.runtime.clone();
    runtime.spawn(async move {
        let mut shutdown = shared.shutdown_tx.subscribe();
        loop {
            if *shared.shutdown_tx.borrow() {
                break;
            }
            let server = match tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(false)
                .create(&path)
            {
                Ok(server) => server,
                Err(err) => {
                    tracing::warn!("control pipe unavailable: {err}");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(2)) => continue,
                        _ = shutdown.changed() => break,
                    }
                }
            };
            tokio::select! {
                result = server.connect() => {
                    if result.is_err() {
                        continue;
                    }
                }
                _ = shutdown.changed() => break,
            }
            let mut server = server;
            let mut buf = [0u8; 64];
            let read = tokio::time::timeout(Duration::from_secs(2), server.read(&mut buf))
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or(0);
            let command = String::from_utf8_lossy(&buf[..read]).trim().to_string();
            let reply: &[u8] = match command.as_str() {
                "show" => {
                    if interactive {
                        request_show();
                        b"shown\n"
                    } else {
                        b"background\n"
                    }
                }
                "quit" => {
                    request_quit();
                    if !interactive {
                        let _ = shared.shutdown_tx.send(true);
                    }
                    b"quitting\n"
                }
                _ => b"ok\n",
            };
            let _ = server.write_all(reply).await;
            let _ = server.flush().await;
            let _ = server.disconnect();
        }
    });
}

/// Control pipe for a UI process that has no in-process engine (all-users
/// install). Only window activation and quitting the tray process are handled.
pub fn start_ui_control_server(runtime: &tokio::runtime::Handle) {
    let path = pipe_path();
    let runtime = runtime.clone();
    runtime.spawn(async move {
        loop {
            let server = match tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(false)
                .create(&path)
            {
                Ok(server) => server,
                Err(err) => {
                    tracing::warn!("UI control pipe unavailable: {err}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            if server.connect().await.is_err() {
                continue;
            }
            let mut server = server;
            let mut buf = [0u8; 64];
            let read = tokio::time::timeout(Duration::from_secs(2), server.read(&mut buf))
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or(0);
            let command = String::from_utf8_lossy(&buf[..read]).trim().to_string();
            let reply: &[u8] = match command.as_str() {
                "show" => {
                    request_show();
                    b"shown\n"
                }
                "ping" => b"background\n",
                "quit" => {
                    request_quit();
                    b"quitting\n"
                }
                _ => b"ok\n",
            };
            let _ = server.write_all(reply).await;
            let _ = server.flush().await;
            let _ = server.disconnect();
        }
    });
}

pub fn status_path() -> PathBuf {
    config::data_dir().join("state").join("status.json")
}

pub fn write_status(shared: &Shared, mode: &str) {
    let cfg = shared.config();
    let core = shared.core();
    let report = json!({
        "pid": std::process::id(),
        "mode": mode,
        "updated_at": config::now_unix(),
        "host": {
            "enabled": cfg.host.enabled,
            "running": core.host_running,
            "display_name": cfg.host.display_name,
            "port": cfg.host.listen_port,
            "pin_required": cfg.host.require_pin,
            "shared": cfg.host.shared.iter().filter(|printer| printer.enabled).map(|printer| json!({
                "id": printer.id,
                "name": printer.display_name,
                "model": printer.model,
                "spooler": printer.spooler_name,
            })).collect::<Vec<_>>(),
            "error": core.host_error,
        },
        "printers": core.local_printers.iter().map(|printer| json!({
            "name": printer.name,
            "port": printer.port_name,
            "status": printer.status_text,
            "online": printer.online,
            "jobs": printer.jobs,
        })).collect::<Vec<_>>(),
        "relays": core.relays.iter().map(|relay| json!({
            "queue": relay.queue_name,
            "port": relay.port,
            "listening": relay.listening,
            "last_error": relay.last_error,
            "last_job": relay.last_job,
        })).collect::<Vec<_>>(),
        "saved": cfg.client.saved.iter().map(|printer| json!({
            "queue": printer.queue_name,
            "host": printer.host_display,
            "host_addr": printer.host_addr,
            "relay_port": printer.relay_port,
            "installed": printer.installed,
        })).collect::<Vec<_>>(),
        "jobs": core.jobs.iter().take(20).map(|job| json!({
            "id": job.id,
            "direction": job.direction,
            "printer": job.printer_name,
            "doc": job.doc_name,
            "bytes": job.bytes,
            "state": job.state,
            "pages": job.pages,
            "created": job.created,
            "finished": job.finished,
        })).collect::<Vec<_>>(),
    });
    let path = status_path();
    let tmp = path.with_extension("json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Best-effort removal of peerPrint's per-user files. Open log files may survive.
pub fn purge_data() {
    let dir = config::data_dir();
    for name in [
        "config.json",
        "paired_clients.json",
        "jobs.jsonl",
        "identity",
        "outbox",
        "state",
        "logs",
        "ipc",
    ] {
        let path = dir.join(name);
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}
