use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use crate::config::{self, SavedPrinter};
use crate::net::framing::{read_msg, write_msg, MsgReader};
use crate::net::protocol::{ClientMsg, ErrCode, HostMsg, PrinterMeta, CHUNK_SIZE, PROTO_VERSION};
use crate::net::tls;
use crate::pairing;
use crate::service::{JobDirection, JobRecord, RelayView, Shared};
use crate::win::{dpapi, elevation, powershell, spooler};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const IO_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct InfoSnapshot {
    pub host_id: Uuid,
    pub display_name: String,
    pub pin_required: bool,
    pub printers: Vec<PrinterMeta>,
    pub tls_fp: [u8; 32],
}

fn parse_fingerprint(hex_fp: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_fp).context("decoding stored fingerprint")?;
    if bytes.len() != 32 {
        bail!("stored fingerprint has the wrong length");
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn parse_addr(addr: &str) -> Result<SocketAddr> {
    if let Ok(socket) = addr.parse() {
        return Ok(socket);
    }
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()
        .with_context(|| format!("resolving {addr}"))?
        .next()
        .with_context(|| format!("{addr} did not resolve"))
}

async fn send_hello(stream: &mut tls::Conn) -> Result<HostMsg> {
    write_msg(
        stream,
        &ClientMsg::Hello {
            proto: PROTO_VERSION,
            app_version: APP_VERSION.to_string(),
        },
    )
    .await?;
    let reply = tokio::time::timeout(IO_TIMEOUT, read_msg::<_, HostMsg>(stream))
        .await
        .context("host handshake timed out")??;
    match &reply {
        HostMsg::HelloAck { proto, .. } if *proto != PROTO_VERSION => {
            bail!("host speaks protocol {proto}, this client needs {PROTO_VERSION}")
        }
        HostMsg::HelloAck { .. } => Ok(reply),
        HostMsg::Error { message, .. } => bail!("host refused the connection: {message}"),
        other => bail!("unexpected handshake reply: {other:?}"),
    }
}

pub async fn fetch_info(addr: SocketAddr, expected: Option<[u8; 32]>) -> Result<InfoSnapshot> {
    let config = tls::client_config(expected);
    let mut stream = tls::connect(addr, config).await?;
    let tls_fp = tls::peer_fingerprint(&stream).context("reading host certificate")?;
    let ack = send_hello(&mut stream).await?;
    let (host_id, display_name, pin_required) = match ack {
        HostMsg::HelloAck {
            host_id,
            display_name,
            pin_required,
            ..
        } => (host_id, display_name, pin_required),
        _ => unreachable!("send_hello validates"),
    };
    write_msg(&mut stream, &ClientMsg::Info).await?;
    let reply = tokio::time::timeout(IO_TIMEOUT, read_msg::<_, HostMsg>(&mut stream))
        .await
        .context("host info timed out")??;
    match reply {
        HostMsg::InfoReply { printers, .. } => Ok(InfoSnapshot {
            host_id,
            display_name,
            pin_required,
            printers,
            tls_fp,
        }),
        HostMsg::Error { message, .. } => bail!("host error: {message}"),
        other => bail!("unexpected info reply: {other:?}"),
    }
}

pub async fn pair_with_host(
    addr: SocketAddr,
    pin: &str,
    client_name: &str,
) -> Result<pairing::PairClientResult> {
    let config = tls::client_config(None);
    let mut stream = tls::connect(addr, config).await?;
    send_hello(&mut stream).await?;
    pairing::pair_client(&mut stream, pin, client_name).await
}

pub fn client_display_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "peerPrint".to_string());
    match std::env::var("COMPUTERNAME") {
        Ok(computer) => format!("{user} ({computer})"),
        Err(_) => user,
    }
}

pub fn sanitize_queue_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\\' | ',' | ';' | '|' | '"'))
        .collect();
    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.chars().count() > 110 {
        cleaned = cleaned.chars().take(110).collect();
    }
    if cleaned.is_empty() {
        cleaned = "peerPrint printer".to_string();
    }
    let upper = cleaned.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "LPT1", "LPT2", "LPT3"];
    if reserved.contains(&upper.as_str()) {
        cleaned.push_str(" printer");
    }
    cleaned
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallSpec {
    pub port_name: String,
    pub relay_port: u16,
    pub queue_name: String,
    pub driver_name: String,
    pub location: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallResult {
    pub ok: bool,
    pub message: String,
    pub need_driver: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UninstallSpec {
    pub port_name: String,
    pub queue_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UninstallAllSpec {
    pub entries: Vec<UninstallSpec>,
}

pub fn validate_install_spec(spec: &InstallSpec) -> Result<()> {
    let port_ok = spec.port_name.starts_with("PeerPrint_")
        && spec.port_name.len() <= 64
        && spec.port_name[11..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !port_ok {
        bail!("invalid port name");
    }
    if !(1024..=65530).contains(&spec.relay_port) {
        bail!("relay port out of range");
    }
    if spec.queue_name.trim().is_empty() || spec.queue_name.chars().count() > 120 {
        bail!("invalid queue name length");
    }
    if spec
        .queue_name
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ',' | ';' | '|'))
        || spec.queue_name.ends_with(' ')
        || spec.queue_name.ends_with('.')
    {
        bail!("queue name contains characters Windows rejects");
    }
    if spec.driver_name.trim().is_empty()
        || spec.driver_name.chars().count() > 128
        || spec.driver_name.chars().any(|c| c.is_control())
    {
        bail!("invalid driver name");
    }
    if spec.location.chars().count() > 256 || spec.location.chars().any(|c| c.is_control()) {
        bail!("invalid location");
    }
    Ok(())
}

fn install_script(spec: &InstallSpec) -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$portName = {port}
$queue = {queue}
$driver = {driver}
$location = {location}
if (Get-Printer -Name $queue -ErrorAction SilentlyContinue) {{
  Write-Output ('QUEUE_EXISTS:' + $queue)
  exit 23
}}
if (-not (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{
  Add-PrinterPort -Name $portName -PrinterHostAddress '127.0.0.1' -PortNumber {relay}
}}
$deadline = (Get-Date).AddSeconds(15)
while (-not (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{
  if ((Get-Date) -gt $deadline) {{ throw 'PORT_TIMEOUT' }}
  Start-Sleep -Milliseconds 100
}}
if (-not (Get-PrinterDriver -Name $driver -ErrorAction SilentlyContinue)) {{
  try {{ Add-PrinterDriver -Name $driver -ErrorAction Stop }} catch {{ }}
}}
if (-not (Get-PrinterDriver -Name $driver -ErrorAction SilentlyContinue)) {{
  Write-Output ('DRIVER_MISSING:' + $driver)
  exit 21
}}
Add-Printer -Name $queue -DriverName $driver -PortName $portName -Comment 'peerPrint' -Location $location
$installed = Get-Printer -Name $queue -ErrorAction Stop
if ($installed.PortName -ne $portName) {{
  Write-Output ('QUEUE_PORT_MISMATCH:' + $installed.PortName)
  exit 24
}}
Write-Output ('QUEUE_OK:' + $queue)
"#,
        port = powershell::ps_quote(&spec.port_name),
        queue = powershell::ps_quote(&spec.queue_name),
        driver = powershell::ps_quote(&spec.driver_name),
        location = powershell::ps_quote(&spec.location),
        relay = spec.relay_port,
    )
}

fn uninstall_script(spec: &UninstallSpec) -> String {
    format!(
        r#"$ErrorActionPreference = 'Continue'
$portName = {port}
$queue = {queue}
if (Get-Printer -Name $queue -ErrorAction SilentlyContinue) {{
  try {{ Get-PrintJob -PrinterName $queue -ErrorAction SilentlyContinue | Remove-PrintJob -ErrorAction SilentlyContinue }} catch {{ }}
  for ($i = 0; $i -lt 20; $i++) {{
    Remove-Printer -Name $queue -ErrorAction SilentlyContinue
    if (-not (Get-Printer -Name $queue -ErrorAction SilentlyContinue)) {{ break }}
    Start-Sleep -Milliseconds 300
  }}
}}
if (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue) {{
  for ($i = 0; $i -lt 20; $i++) {{
    Remove-PrinterPort -Name $portName -ErrorAction SilentlyContinue
    if (-not (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{ break }}
    Start-Sleep -Milliseconds 300
  }}
}}
if (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue) {{
  $vbs = Get-ChildItem "$env:WINDIR\System32\Printing_Admin_Scripts\*\prnport.vbs" -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($vbs) {{ & cscript.exe //nologo $vbs.FullName -d -r $portName | Out-Null }}
}}
if ((Get-Printer -Name $queue -ErrorAction SilentlyContinue) -or (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{
  Write-Output 'REMOVE_INCOMPLETE'
  exit 22
}}
Write-Output 'REMOVED'
"#,
        port = powershell::ps_quote(&spec.port_name),
        queue = powershell::ps_quote(&spec.queue_name),
    )
}

fn ipc_dir() -> PathBuf {
    config::data_dir().join("ipc")
}

fn result_path(spec: &Path) -> PathBuf {
    let stem = spec
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "spec".to_string());
    spec.with_file_name(format!("{stem}.result.json"))
}

fn validate_worker_path(path: &Path, prefix: &str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("opening {}", path.display()))?;
    let ipc = ipc_dir().canonicalize().context("resolving IPC directory")?;
    if !canonical.starts_with(&ipc) {
        bail!("spec file is outside the peerPrint IPC directory");
    }
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.starts_with(prefix) || canonical.extension().and_then(|e| e.to_str()) != Some("json") {
        bail!("unexpected spec file name");
    }
    Ok(canonical)
}

fn write_result(spec: &Path, result: &InstallResult) {
    let _ = config::write_json(&result_path(spec), result);
}

pub fn run_install_worker(path: &str) -> i32 {
    let _ = config::ensure_dirs();
    let spec_path = match validate_worker_path(Path::new(path), "install-") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("peerPrint install worker: {err:#}");
            return 2;
        }
    };
    let spec: InstallSpec = match config::read_json(&spec_path) {
        Some(spec) => spec,
        None => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: "could not read install spec".to_string(),
                    need_driver: false,
                },
            );
            return 2;
        }
    };
    if let Err(err) = validate_install_spec(&spec) {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: format!("install spec rejected: {err}"),
                need_driver: false,
            },
        );
        return 3;
    }

    let output = match powershell::run(&install_script(&spec), Duration::from_secs(120)) {
        Ok(output) => output,
        Err(err) => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: format!("{err:#}"),
                    need_driver: false,
                },
            );
            return 4;
        }
    };
    let text = output.combined();
    if text.contains("DRIVER_MISSING:") {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: format!("Printer driver '{}' is not installed", spec.driver_name),
                need_driver: true,
            },
        );
        return 21;
    }
    if text.contains("QUEUE_EXISTS:") {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: format!(
                    "A printer named '{}' already exists on this PC; remove it or retry",
                    spec.queue_name
                ),
                need_driver: false,
            },
        );
        return 23;
    }
    if text.contains("QUEUE_PORT_MISMATCH:") {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: format!(
                    "'{}' was created on the wrong port ({}); remove it and retry",
                    spec.queue_name,
                    text.lines()
                        .find_map(|line| line.strip_prefix("QUEUE_PORT_MISMATCH:"))
                        .unwrap_or("unknown")
                        .trim()
                ),
                need_driver: false,
            },
        );
        return 24;
    }
    if !output.success() || !text.contains("QUEUE_OK:") {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: if text.is_empty() {
                    "printer installation failed".to_string()
                } else {
                    text
                },
                need_driver: false,
            },
        );
        return 5;
    }

    let bidi = spooler::clear_bidi(&spec.queue_name);
    let message = match &bidi {
        Ok(()) => format!("Installed '{}'", spec.queue_name),
        Err(err) => format!("Installed '{}' (bidirectional flag not cleared: {err})", spec.queue_name),
    };
    write_result(
        &spec_path,
        &InstallResult {
            ok: true,
            message,
            need_driver: false,
        },
    );
    0
}

pub fn run_uninstall_worker(path: &str) -> i32 {
    let _ = config::ensure_dirs();
    let spec_path = match validate_worker_path(Path::new(path), "uninstall-") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("peerPrint uninstall worker: {err:#}");
            return 2;
        }
    };
    let spec: UninstallSpec = match config::read_json(&spec_path) {
        Some(spec) => spec,
        None => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: "could not read uninstall spec".to_string(),
                    need_driver: false,
                },
            );
            return 2;
        }
    };
    if !spec.port_name.starts_with("PeerPrint_") {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: "refusing to remove a port peerPrint did not create".to_string(),
                need_driver: false,
            },
        );
        return 3;
    }

    let output = match powershell::run(&uninstall_script(&spec), Duration::from_secs(120)) {
        Ok(output) => output,
        Err(err) => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: format!("{err:#}"),
                    need_driver: false,
                },
            );
            return 4;
        }
    };
    let text = output.combined();
    let ok = output.success() && text.contains("REMOVED");
    write_result(
        &spec_path,
        &InstallResult {
            ok,
            message: if text.is_empty() {
                "printer removal failed".to_string()
            } else {
                text
            },
            need_driver: false,
        },
    );
    if ok {
        0
    } else {
        5
    }
}

fn valid_port_name(name: &str) -> bool {
    name.starts_with("PeerPrint_")
        && name.len() <= 64
        && name[11..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn uninstall_all_script(entries: &[UninstallSpec]) -> String {
    let mut targets = String::new();
    for entry in entries {
        targets.push_str(&format!(
            "  [pscustomobject]@{{ Queue = {queue}; Port = {port} }}\n",
            queue = powershell::ps_quote(&entry.queue_name),
            port = powershell::ps_quote(&entry.port_name),
        ));
    }
    format!(
        r#"$ErrorActionPreference = 'Continue'
$targets = @(
{targets})
$failed = 0
foreach ($t in $targets) {{
  $queue = $t.Queue
  $portName = $t.Port
  if ($queue -and (Get-Printer -Name $queue -ErrorAction SilentlyContinue)) {{
    try {{ Get-PrintJob -PrinterName $queue -ErrorAction SilentlyContinue | Remove-PrintJob -ErrorAction SilentlyContinue }} catch {{ }}
    for ($i = 0; $i -lt 20; $i++) {{
      Remove-Printer -Name $queue -ErrorAction SilentlyContinue
      if (-not (Get-Printer -Name $queue -ErrorAction SilentlyContinue)) {{ break }}
      Start-Sleep -Milliseconds 300
    }}
  }}
  if (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue) {{
    for ($i = 0; $i -lt 20; $i++) {{
      Remove-PrinterPort -Name $portName -ErrorAction SilentlyContinue
      if (-not (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{ break }}
      Start-Sleep -Milliseconds 300
    }}
  }}
  if (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue) {{
    $vbs = Get-ChildItem "$env:WINDIR\System32\Printing_Admin_Scripts\*\prnport.vbs" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($vbs) {{ & cscript.exe //nologo $vbs.FullName -d -r $portName | Out-Null }}
  }}
  if (($queue -and (Get-Printer -Name $queue -ErrorAction SilentlyContinue)) -or (Get-PrinterPort -Name $portName -ErrorAction SilentlyContinue)) {{
    Write-Output ('REMOVE_INCOMPLETE:' + $portName)
    $failed++
  }}
}}
if ($failed -gt 0) {{ exit 22 }}
Write-Output 'REMOVED_ALL'
"#
    )
}

pub fn run_uninstall_all_worker(path: &str) -> i32 {
    let _ = config::ensure_dirs();
    let spec_path = match validate_worker_path(Path::new(path), "uninstall-all-") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("peerPrint uninstall-all worker: {err:#}");
            return 2;
        }
    };
    let spec: UninstallAllSpec = match config::read_json(&spec_path) {
        Some(spec) => spec,
        None => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: "could not read uninstall-all spec".to_string(),
                    need_driver: false,
                },
            );
            return 2;
        }
    };
    if let Some(bad) = spec.entries.iter().find(|entry| {
        !valid_port_name(&entry.port_name) || entry.queue_name.chars().any(char::is_control)
    }) {
        write_result(
            &spec_path,
            &InstallResult {
                ok: false,
                message: format!("refusing to remove unexpected port '{}'", bad.port_name),
                need_driver: false,
            },
        );
        return 3;
    }
    if spec.entries.is_empty() {
        write_result(
            &spec_path,
            &InstallResult {
                ok: true,
                message: "nothing to remove".to_string(),
                need_driver: false,
            },
        );
        return 0;
    }

    let output = match powershell::run(&uninstall_all_script(&spec.entries), Duration::from_secs(300))
    {
        Ok(output) => output,
        Err(err) => {
            write_result(
                &spec_path,
                &InstallResult {
                    ok: false,
                    message: format!("{err:#}"),
                    need_driver: false,
                },
            );
            return 4;
        }
    };
    let text = output.combined();
    let ok = output.success() && text.contains("REMOVED_ALL");
    write_result(
        &spec_path,
        &InstallResult {
            ok,
            message: if text.is_empty() {
                "printer removal failed".to_string()
            } else {
                text
            },
            need_driver: false,
        },
    );
    if ok {
        0
    } else {
        5
    }
}

fn collect_uninstall_entries() -> Vec<UninstallSpec> {
    let mut entries: Vec<UninstallSpec> = config::AppConfig::load()
        .client
        .saved
        .into_iter()
        .filter(|saved| valid_port_name(&saved.port_name))
        .map(|saved| UninstallSpec {
            port_name: saved.port_name,
            queue_name: saved.queue_name,
        })
        .collect();

    let script = r#"$ErrorActionPreference = 'SilentlyContinue'
$items = @()
Get-Printer | Where-Object { $_.PortName -like 'PeerPrint_*' } | ForEach-Object { $items += [pscustomobject]@{ port_name = $_.PortName; queue_name = $_.Name } }
Get-PrinterPort | Where-Object { $_.Name -like 'PeerPrint_*' } | ForEach-Object { $port = $_.Name; if (-not ($items | Where-Object { $_.port_name -eq $port })) { $items += [pscustomobject]@{ port_name = $port; queue_name = '' } } }
ConvertTo-Json -Compress -InputObject @($items)"#;
    if let Ok(output) = powershell::run(script, Duration::from_secs(30)) {
        if output.success() {
            let found: Vec<UninstallSpec> =
                serde_json::from_str::<Vec<UninstallSpec>>(output.stdout.trim())
                    .or_else(|_| {
                        serde_json::from_str::<UninstallSpec>(output.stdout.trim())
                            .map(|single| vec![single])
                    })
                    .unwrap_or_default();
            for entry in found {
                if valid_port_name(&entry.port_name)
                    && entries
                        .iter()
                        .all(|existing| existing.port_name != entry.port_name)
                {
                    entries.push(entry);
                }
            }
        }
    }
    entries
}

pub fn uninstall_all() -> Result<()> {
    config::ensure_dirs()?;
    let _ = crate::ipc::send_control("quit");
    std::thread::sleep(Duration::from_millis(800));
    let entries = collect_uninstall_entries();
    if entries.is_empty() {
        println!("No peerPrint printer queues to remove.");
    } else {
        let spec = UninstallAllSpec {
            entries: entries.clone(),
        };
        let spec_path = write_spec(&spec, "uninstall-all")?;
        let result = run_elevated_worker(&spec_path, "--uninstall-all-worker")?;
        if !result.ok {
            bail!(
                "removing peerPrint printers failed: {}",
                result.message.trim()
            );
        }
        println!("Removed {} peerPrint printer queue(s).", entries.len());
    }
    if let Err(err) = crate::win::autostart::set_enabled(false) {
        eprintln!("removing the startup entry failed: {err:#}");
    }
    crate::ipc::purge_data();
    println!("peerPrint settings and saved printers were deleted.");
    Ok(())
}

fn write_spec<T: Serialize>(spec: &T, prefix: &str) -> Result<PathBuf> {
    let dir = ipc_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{prefix}-{}.json", Uuid::new_v4()));
    config::write_json(&path, spec)?;
    Ok(path)
}

fn run_elevated_worker(spec_path: &Path, flag: &str) -> Result<InstallResult> {
    // The machine-wide engine already runs as LocalSystem; asking for
    // elevation again would fail outside an interactive desktop.
    let code = if elevation::is_elevated() {
        let path = spec_path.to_string_lossy().into_owned();
        match flag {
            "--install-client" => run_install_worker(&path) as u32,
            "--uninstall-client" => run_uninstall_worker(&path) as u32,
            "--uninstall-all-worker" => run_uninstall_all_worker(&path) as u32,
            _ => 2,
        }
    } else {
        let argument = format!(
            "{flag} {}",
            elevation::quote_argument(&spec_path.to_string_lossy())
        );
        elevation::run_elevated(&argument)?
    };
    let result: Option<InstallResult> = config::read_json(&result_path(spec_path));
    let _ = std::fs::remove_file(spec_path);
    let _ = std::fs::remove_file(result_path(spec_path));
    match result {
        Some(result) => Ok(result),
        None => Ok(InstallResult {
            ok: code == 0,
            message: if code == 0 {
                "done".to_string()
            } else {
                format!("elevated helper exited with code {code}")
            },
            need_driver: false,
        }),
    }
}

pub fn queue_exists(queue: &str) -> bool {
    let script = format!(
        "if (Get-Printer -Name {} -ErrorAction SilentlyContinue) {{ 'yes' }} else {{ 'no' }}",
        powershell::ps_quote(queue)
    );
    powershell::run(&script, Duration::from_secs(30))
        .map(|output| output.stdout.trim() == "yes")
        .unwrap_or(false)
}

pub fn port_exists(port_name: &str) -> bool {
    let script = format!(
        "if (Get-PrinterPort -Name {} -ErrorAction SilentlyContinue) {{ 'yes' }} else {{ 'no' }}",
        powershell::ps_quote(port_name)
    );
    powershell::run(&script, Duration::from_secs(30))
        .map(|output| output.stdout.trim() == "yes")
        .unwrap_or(false)
}

fn choose_relay_port(shared: &Shared) -> u16 {
    let cfg = shared.config();
    let used: HashSet<u16> = cfg.client.saved.iter().map(|saved| saved.relay_port).collect();
    let base = if cfg.client.port_base < 1024 {
        config::DEFAULT_RELAY_PORT_BASE
    } else {
        cfg.client.port_base
    };
    for offset in 0..200u16 {
        let Some(port) = base.checked_add(offset) else {
            break;
        };
        if used.contains(&port) {
            continue;
        }
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    base
}

fn unique_queue_name(base: &str) -> String {
    let candidate = sanitize_queue_name(base);
    if !queue_exists(&candidate) {
        return candidate;
    }
    let suffix = sanitize_queue_name(&format!("{candidate} (peerPrint)"));
    if !queue_exists(&suffix) {
        return suffix;
    }
    for index in 2..100 {
        let numbered = sanitize_queue_name(&format!("{candidate} (peerPrint {index})"));
        if !queue_exists(&numbered) {
            return numbered;
        }
    }
    sanitize_queue_name(&format!(
        "{candidate} (peerPrint {})",
        &Uuid::new_v4().simple().to_string()[..6]
    ))
}

fn queue_port(queue: &str) -> Option<String> {
    let script = format!(
        "$ErrorActionPreference='Stop'; (Get-Printer -Name {}).PortName",
        powershell::ps_quote(queue)
    );
    let output = powershell::run(&script, Duration::from_secs(30)).ok()?;
    if !output.success() {
        return None;
    }
    let port = output.stdout.trim().to_string();
    if port.is_empty() {
        None
    } else {
        Some(port)
    }
}

pub async fn add_printer(
    shared: Arc<Shared>,
    host: &crate::service::DiscoveredHost,
    printer_id: Uuid,
    pin: &str,
) -> Result<SavedPrinter> {
    let printer = host
        .printers
        .iter()
        .find(|printer| printer.id == printer_id)
        .with_context(|| "printer is no longer advertised by the host")?
        .clone();
    if !printer.driver_env.eq_ignore_ascii_case("Windows x64") {
        bail!(
            "the host driver for '{}' is {}; only Windows x64 drivers can be installed",
            printer.name,
            if printer.driver_env.is_empty() {
                "unknown"
            } else {
                &printer.driver_env
            }
        );
    }
    {
        let cfg = shared.config();
        if cfg.client.saved.iter().any(|saved| {
            saved.host_id == host.host_id.to_string() && saved.printer_id == printer_id
        }) {
            bail!("this printer is already installed");
        }
    }
    let addr = parse_addr(&host.addr)?;
    let client_name = client_display_name();
    let pair = pair_with_host(addr, pin, &client_name).await?;
    if pair.host_id != host.host_id {
        bail!("the host proved a different identity than advertised");
    }

    let id = Uuid::new_v4();
    let relay_port = choose_relay_port(&shared);
    let short = id.simple().to_string();
    let saved = SavedPrinter {
        id,
        client_id: pair.client_id,
        host_id: pair.host_id.to_string(),
        host_addr: host.addr.clone(),
        host_display: pair.host_display.clone(),
        printer_id,
        printer_name: printer.name.clone(),
        printer_model: printer.model.clone(),
        driver_name: printer.driver_name.clone(),
        driver_env: printer.driver_env.clone(),
        relay_port,
        port_name: format!("PeerPrint_{}", &short[..12]),
        queue_name: unique_queue_name(&printer.name),
        tls_fp: hex::encode(pair.tls_fp),
        token_dpapi: dpapi::protect_b64(&pair.token)?,
        installed: false,
    };

    let spec = InstallSpec {
        port_name: saved.port_name.clone(),
        relay_port: saved.relay_port,
        queue_name: saved.queue_name.clone(),
        driver_name: saved.driver_name.clone(),
        location: saved.host_display.clone(),
    };
    let spec_path = write_spec(&spec, "install")?;
    let result = tokio::task::spawn_blocking(move || run_elevated_worker(&spec_path, "--install-client"))
        .await
        .context("install task panicked")??;
    if !result.ok {
        if result.need_driver {
            bail!(
                "{} — install that driver on this PC, then try again",
                result.message
            );
        }
        bail!("{}", result.message);
    }

    let verify_queue = saved.queue_name.clone();
    let verify_port = saved.port_name.clone();
    let verified = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let port_here = port_exists(&verify_port);
            let mapped = queue_port(&verify_queue);
            if port_here && mapped.as_deref() == Some(verify_port.as_str()) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                tracing::warn!(
                    "install verification failed: port_exists={port_here} queue_port={mapped:?} want={verify_port} queue={verify_queue}"
                );
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    })
    .await
    .unwrap_or(false);
    if !verified {
        let spec = UninstallSpec {
            port_name: saved.port_name.clone(),
            queue_name: saved.queue_name.clone(),
        };
        if let Ok(spec_path) = write_spec(&spec, "uninstall") {
            let _ = tokio::task::spawn_blocking(move || {
                run_elevated_worker(&spec_path, "--uninstall-client")
            })
            .await;
        }
        bail!("Windows did not create the printer queue on the peerPrint port; the partial install was removed");
    }

    let mut installed = saved.clone();
    installed.installed = true;
    let mut cfg = shared.config();
    cfg.client.saved.push(installed.clone());
    shared.set_config(cfg)?;

    shared.notice(
        crate::service::NoticeLevel::Info,
        format!(
            "Installed '{}' from {} on 127.0.0.1:{}",
            installed.queue_name, installed.host_display, installed.relay_port
        ),
    );
    Ok(installed)
}

pub async fn remove_printer(shared: Arc<Shared>, saved_id: Uuid) -> Result<()> {
    let saved = shared
        .saved_printers()
        .into_iter()
        .find(|saved| saved.id == saved_id)
        .context("saved printer not found")?;
    let spec = UninstallSpec {
        port_name: saved.port_name.clone(),
        queue_name: saved.queue_name.clone(),
    };
    let spec_path = write_spec(&spec, "uninstall")?;
    let result = tokio::task::spawn_blocking(move || run_elevated_worker(&spec_path, "--uninstall-client"))
        .await
        .context("uninstall task panicked")??;
    if !result.ok {
        bail!("{}", result.message);
    }

    let mut cfg = shared.config();
    cfg.client.saved.retain(|item| item.id != saved_id);
    shared.set_config(cfg)?;
    shared.remove_relay(saved_id);
    let _ = std::fs::remove_dir_all(outbox_dir(saved_id));
    shared.notice(
        crate::service::NoticeLevel::Info,
        format!("Removed '{}'", saved.queue_name),
    );
    Ok(())
}

pub async fn test_page(queue_name: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let script = format!(
            r#"$ErrorActionPreference='Stop'
$text = @(
  'peerPrint test page',
  'If this page prints, the peerPrint print path is working.',
  (Get-Date).ToString('u')
) -join [Environment]::NewLine
$text | Out-Printer -Name {queue}
"#,
            queue = powershell::ps_quote(&queue_name)
        );
        let output = powershell::run(&script, Duration::from_secs(60))?;
        if !output.success() {
            bail!("{}", output.combined());
        }
        Ok(())
    })
    .await
    .context("test page task panicked")?
}

pub fn set_queue_paused(queue_name: &str, paused: bool) -> Result<()> {
    let method = if paused { "Pause" } else { "Resume" };
    let script = format!(
        "$ErrorActionPreference='Stop'; $name = {name}; $p = Get-CimInstance -ClassName Win32_Printer | Where-Object {{ $_.Name -eq $name }} | Select-Object -First 1; if ($p) {{ Invoke-CimMethod -InputObject $p -MethodName {method} | Out-Null }}",
        name = powershell::ps_quote(queue_name),
        method = method,
    );
    let output = powershell::run(&script, Duration::from_secs(30))?;
    if !output.success() {
        bail!("{}", output.combined());
    }
    Ok(())
}

pub fn pause_all_queues(shared: &Shared) {
    for saved in shared.saved_printers().iter().filter(|saved| saved.installed) {
        if let Err(err) = set_queue_paused(&saved.queue_name, true) {
            tracing::debug!("pausing '{}' failed: {err:#}", saved.queue_name);
        }
    }
}

pub fn resume_all_queues(shared: &Shared) {
    for saved in shared.saved_printers().iter().filter(|saved| saved.installed) {
        if let Err(err) = set_queue_paused(&saved.queue_name, false) {
            tracing::debug!("resuming '{}' failed: {err:#}", saved.queue_name);
        }
    }
}

// ---------------------------------------------------------------------------
// Outbox
// ---------------------------------------------------------------------------

pub fn outbox_dir(saved_id: Uuid) -> PathBuf {
    config::data_dir().join("outbox").join(saved_id.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub job_ref: Uuid,
    pub printer_id: Uuid,
    pub doc_name: String,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub state: String,
    pub error: Option<String>,
    pub created: i64,
    pub attempts: u32,
    pub next_try: i64,
}

fn entry_paths(saved_id: Uuid, job_ref: Uuid) -> (PathBuf, PathBuf) {
    let dir = outbox_dir(saved_id);
    (
        dir.join(format!("{job_ref}.bin")),
        dir.join(format!("{job_ref}.json")),
    )
}

async fn spool_connection(shared: Arc<Shared>, saved: SavedPrinter, mut socket: TcpStream) {
    let job_ref = Uuid::new_v4();
    let dir = outbox_dir(saved.id);
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        tracing::error!("outbox directory: {err:#}");
        return;
    }
    let (bin_path, json_path) = entry_paths(saved.id, job_ref);
    let mut file = match tokio::fs::File::create(&bin_path).await {
        Ok(file) => file,
        Err(err) => {
            tracing::error!("creating outbox file: {err:#}");
            return;
        }
    };

    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut failure = None;
    loop {
        match tokio::time::timeout(Duration::from_secs(300), tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                hasher.update(&buffer[..n]);
                total += n as u64;
                if let Err(err) = file.write_all(&buffer[..n]).await {
                    failure = Some(format!("writing outbox: {err}"));
                    break;
                }
            }
            Ok(Err(err)) => {
                failure = Some(format!("reading spool stream: {err}"));
                break;
            }
            Err(_) => {
                failure = Some("spool stream stalled".to_string());
                break;
            }
        }
    }
    let _ = file.flush().await;
    drop(file);
    let _ = socket.shutdown().await;

    if let Some(error) = failure {
        tracing::warn!("job {job_ref} from spooler discarded: {error}");
        let _ = tokio::fs::remove_file(&bin_path).await;
        shared.notice(crate::service::NoticeLevel::Warn, format!("Spool job failed: {error}"));
        return;
    }
    if total == 0 {
        let _ = tokio::fs::remove_file(&bin_path).await;
        return;
    }

    let limit = shared.config().client.outbox_limit_mb.saturating_mul(1024 * 1024);
    if outbox_size(saved.id) + total > limit {
        let _ = tokio::fs::remove_file(&bin_path).await;
        shared.notice(
            crate::service::NoticeLevel::Error,
            "peerPrint outbox is full; the job was not queued".to_string(),
        );
        return;
    }

    let digest: [u8; 32] = hasher.finalize().into();
    let doc_name = format!("peerPrint job {}", &job_ref.simple().to_string()[..8]);
    let entry = OutboxEntry {
        job_ref,
        printer_id: saved.printer_id,
        doc_name: doc_name.clone(),
        bytes: total,
        sha256: digest,
        state: "Queued".to_string(),
        error: None,
        created: config::now_unix(),
        attempts: 0,
        next_try: 0,
    };
    if let Err(err) = tokio::fs::write(&json_path, serde_json::to_vec_pretty(&entry).unwrap_or_default()).await {
        tracing::error!("writing outbox metadata: {err:#}");
        let _ = tokio::fs::remove_file(&bin_path).await;
        return;
    }

    let mut job = JobRecord::new(
        job_ref,
        JobDirection::Client,
        saved.printer_name.clone(),
        saved.host_display.clone(),
        doc_name,
    );
    job.bytes = total;
    shared.record_job(job, true);
    set_relay_job(&shared, &saved, job_ref, &format!("Queued {total} bytes"));
}

fn outbox_size(saved_id: Uuid) -> u64 {
    std::fs::read_dir(outbox_dir(saved_id))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| entry.metadata().ok())
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

fn load_entries(saved_id: Uuid) -> Vec<(PathBuf, OutboxEntry)> {
    let mut entries = Vec::new();
    let Ok(dir) = std::fs::read_dir(outbox_dir(saved_id)) else {
        return entries;
    };
    for path in dir.filter_map(|entry| entry.ok()).map(|entry| entry.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(entry) = config::read_json::<OutboxEntry>(&path) {
            entries.push((path, entry));
        }
    }
    entries.sort_by_key(|(_, entry)| entry.created);
    entries
}

fn save_entry(path: &Path, entry: &OutboxEntry) {
    let _ = config::write_json(path, entry);
}

fn backoff_secs(attempts: u32) -> i64 {
    let base = 1u64 << attempts.min(5);
    (base.min(30) as i64) + (config::now_unix() % 3)
}

#[cfg(test)]
mod tests {
    use super::backoff_secs;

    #[test]
    fn retry_backoff_grows_and_is_capped() {
        for (attempts, low, high) in [(0, 1, 3), (1, 2, 4), (3, 8, 10), (5, 30, 32), (40, 30, 32)] {
            let delay = backoff_secs(attempts);
            assert!(
                (low..=high).contains(&delay),
                "attempt {attempts} waited {delay}s, expected {low}..={high}s"
            );
        }
    }
}

pub fn cancel_client_job(shared: &Shared, job: &JobRecord) {
    if job.direction != JobDirection::Client || job.terminal() {
        return;
    }
    if job.state == "Printing" {
        shared.request_cancel(job.id);
        shared.notice(
            crate::service::NoticeLevel::Info,
            format!("Asking the host to cancel '{}'…", job.doc_name),
        );
        return;
    }
    shared.request_cancel(job.id);
    if let Some(saved) = shared
        .saved_printers()
        .into_iter()
        .find(|saved| saved.printer_name == job.printer_name)
    {
        let (bin_path, json_path) = entry_paths(saved.id, job.id);
        let _ = std::fs::remove_file(bin_path);
        let _ = std::fs::remove_file(json_path);
        set_relay_job(shared, &saved, job.id, "Cancelled");
    }
    shared.mark_cancelled(job.id, "cancelled before printing");
    shared.notice(
        crate::service::NoticeLevel::Info,
        format!("Cancelled '{}'", job.doc_name),
    );
}

enum UploadOutcome {
    Done { pages: u32 },
    Retry(String),
    Failed(String),
    Cancelled,
    Revoked,
}

async fn upload_job(
    shared: &Arc<Shared>,
    saved: &SavedPrinter,
    entry: &OutboxEntry,
) -> UploadOutcome {
    let Some(token) = config::token_from_saved(saved) else {
        return UploadOutcome::Failed("saved token is unreadable".to_string());
    };
    let Ok(fingerprint) = parse_fingerprint(&saved.tls_fp) else {
        return UploadOutcome::Failed("saved fingerprint is corrupt".to_string());
    };
    if shared.cancel_requested(entry.job_ref)
        || shared.job_state(entry.job_ref).as_deref() == Some("Cancelled")
    {
        return UploadOutcome::Cancelled;
    }
    let Some(addr) = resolve_host(shared, saved) else {
        return UploadOutcome::Retry("host address unknown".to_string());
    };
    let (bin_path, _) = entry_paths(saved.id, entry.job_ref);
    let mut file = match tokio::fs::File::open(&bin_path).await {
        Ok(file) => file,
        Err(err) => return UploadOutcome::Failed(format!("outbox job missing: {err}")),
    };

    let config = tls::client_config(Some(fingerprint));
    let mut stream = match tls::connect(addr, config).await {
        Ok(stream) => stream,
        Err(err) => return UploadOutcome::Retry(format!("{err:#}")),
    };
    if let Err(err) = send_hello(&mut stream).await {
        return UploadOutcome::Retry(format!("{err:#}"));
    }
    if let Err(err) = write_msg(
        &mut stream,
        &ClientMsg::Auth {
            client_id: saved.client_id,
            token,
        },
    )
    .await
    {
        return UploadOutcome::Retry(format!("{err:#}"));
    }
    match read_reply(&mut stream).await {
        Ok(HostMsg::AuthOk { .. }) => {}
        Ok(HostMsg::Error { code, message, .. }) => {
            return match code {
                ErrCode::Revoked | ErrCode::Unauthorized => UploadOutcome::Revoked,
                _ => UploadOutcome::Retry(message),
            };
        }
        Ok(other) => return UploadOutcome::Retry(format!("unexpected auth reply: {other:?}")),
        Err(err) => return UploadOutcome::Retry(format!("{err:#}")),
    }

    let tx = random_tx();
    if let Err(err) = write_msg(
        &mut stream,
        &ClientMsg::JobStart {
            tx,
            printer_id: saved.printer_id,
            client_job_ref: entry.job_ref,
            doc_name: entry.doc_name.clone(),
            len: entry.bytes,
            sha256: entry.sha256,
        },
    )
    .await
    {
        return UploadOutcome::Retry(format!("{err:#}"));
    }
    match read_reply(&mut stream).await {
        Ok(HostMsg::JobAccepted { .. }) => {}
        Ok(HostMsg::JobFailed {
            code,
            retryable,
            message,
            ..
        }) => {
            return failure_outcome(code, retryable, message);
        }
        Ok(HostMsg::Error { message, .. }) => return UploadOutcome::Retry(message),
        Ok(other) => return UploadOutcome::Retry(format!("unexpected job reply: {other:?}")),
        Err(err) => return UploadOutcome::Retry(format!("{err:#}")),
    }

    if shared.cancel_requested(entry.job_ref) {
        send_cancel(&mut stream, tx).await;
        return UploadOutcome::Cancelled;
    }

    shared.update_job(entry.job_ref, |job| {
        job.state = "Uploading".to_string();
    });
    set_relay_job(shared, saved, entry.job_ref, "Uploading");

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut sent = 0u64;
    loop {
        if shared.cancel_requested(entry.job_ref) {
            send_cancel(&mut stream, tx).await;
            return UploadOutcome::Cancelled;
        }
        let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await;
        let n = match read {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => return UploadOutcome::Retry(format!("reading outbox: {err}")),
        };
        sent += n as u64;
        if let Err(err) = write_msg(
            &mut stream,
            &ClientMsg::JobData {
                tx,
                data: buffer[..n].to_vec(),
            },
        )
        .await
        {
            return UploadOutcome::Retry(format!("upload interrupted after {sent} bytes: {err:#}"));
        }
    }
    if sent != entry.bytes {
        return UploadOutcome::Failed(format!(
            "outbox file changed size ({sent} of {} bytes)",
            entry.bytes
        ));
    }
    if let Err(err) = write_msg(&mut stream, &ClientMsg::JobEnd { tx }).await {
        return UploadOutcome::Retry(format!("{err:#}"));
    }

    let (read_half, mut write_half) = tokio::io::split(&mut stream);
    let mut reader = MsgReader::new(read_half);
    let mut cancel_sent = false;
    loop {
        if !cancel_sent && shared.cancel_requested(entry.job_ref) {
            let cancel = ClientMsg::JobCancel {
                tx,
                reason: "cancelled on this PC".to_string(),
            };
            if let Err(err) = write_msg(&mut write_half, &cancel).await {
                return UploadOutcome::Retry(format!("sending cancellation: {err:#}"));
            }
            cancel_sent = true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            message = reader.read::<HostMsg>() => match message {
                Ok(HostMsg::JobProgress { pages_printed, .. }) => {
                    shared.update_job(entry.job_ref, |job| {
                        job.state = "Printing".to_string();
                        job.pages = pages_printed;
                    });
                    set_relay_job(shared, saved, entry.job_ref, "Printing");
                }
                Ok(HostMsg::JobDone { pages, .. }) => return UploadOutcome::Done { pages },
                Ok(HostMsg::JobFailed {
                    code,
                    retryable,
                    message,
                    ..
                }) => {
                    return failure_outcome(code, retryable, message);
                }
                Ok(HostMsg::Error { message, .. }) => return UploadOutcome::Retry(message),
                Ok(HostMsg::Pong { .. }) => {}
                Ok(other) => {
                    return UploadOutcome::Retry(format!("unexpected job status: {other:?}"));
                }
                Err(err) => return UploadOutcome::Retry(format!("{err:#}")),
            },
        }
    }
}

async fn read_reply(stream: &mut tls::Conn) -> Result<HostMsg> {
    tokio::time::timeout(IO_TIMEOUT, read_msg::<_, HostMsg>(stream))
        .await
        .context("host stopped responding")?
}

async fn send_cancel(stream: &mut tls::Conn, tx: u32) {
    let cancel = ClientMsg::JobCancel {
        tx,
        reason: "cancelled on this PC".to_string(),
    };
    if write_msg(stream, &cancel).await.is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(5), read_msg::<_, HostMsg>(stream)).await;
    }
}

fn failure_outcome(code: ErrCode, retryable: bool, message: String) -> UploadOutcome {
    match (code, retryable) {
        (ErrCode::Cancelled, _) => UploadOutcome::Cancelled,
        (_, true) => UploadOutcome::Retry(message),
        (_, false) => UploadOutcome::Failed(message),
    }
}

fn random_tx() -> u32 {
    let mut bytes = [0u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        return 1;
    }
    u32::from_le_bytes(bytes)
}

fn resolve_host(shared: &Shared, saved: &SavedPrinter) -> Option<SocketAddr> {
    if let Ok(addr) = saved.host_addr.parse() {
        return Some(addr);
    }
    shared
        .core()
        .discovered
        .iter()
        .find(|host| host.host_id.to_string() == saved.host_id)
        .and_then(|host| host.addr.parse().ok())
        .or_else(|| parse_addr(&saved.host_addr).ok())
}

async fn process_outbox(shared: Arc<Shared>, saved: SavedPrinter) {
    let entries = tokio::task::spawn_blocking(move || load_entries(saved.id))
        .await
        .unwrap_or_default();
    let now = config::now_unix();
    for (path, mut entry) in entries {
        if entry.state != "Queued" || entry.next_try > now {
            continue;
        }
        let outcome = upload_job(&shared, &saved, &entry).await;
        let (bin_path, _) = entry_paths(saved.id, entry.job_ref);
        match outcome {
            UploadOutcome::Done { pages } => {
                shared.update_job(entry.job_ref, |job| {
                    job.state = "Printed".to_string();
                    job.pages = pages;
                    job.finished = Some(config::now_unix());
                    job.error = None;
                });
                let job = shared
                    .core()
                    .jobs
                    .into_iter()
                    .find(|job| job.id == entry.job_ref);
                if let Some(job) = job {
                    crate::service::append_job(&job);
                }
                let _ = std::fs::remove_file(&bin_path);
                let _ = std::fs::remove_file(&path);
                set_relay_job(&shared, &saved, entry.job_ref, "Printed");
            }
            UploadOutcome::Retry(message) => {
                entry.attempts = entry.attempts.saturating_add(1);
                entry.next_try = config::now_unix() + backoff_secs(entry.attempts);
                entry.error = Some(message.clone());
                save_entry(&path, &entry);
                shared.update_job(entry.job_ref, |job| {
                    job.state = "Waiting for host".to_string();
                    job.error = Some(message.clone());
                });
                set_relay_job(&shared, &saved, entry.job_ref, "Waiting for host");
                break;
            }
            UploadOutcome::Failed(message) => {
                shared.update_job(entry.job_ref, |job| {
                    job.state = "Failed".to_string();
                    job.error = Some(message.clone());
                    job.finished = Some(config::now_unix());
                });
                let job = shared
                    .core()
                    .jobs
                    .into_iter()
                    .find(|job| job.id == entry.job_ref);
                if let Some(job) = job {
                    crate::service::append_job(&job);
                }
                let _ = std::fs::remove_file(&bin_path);
                entry.state = "Failed".to_string();
                entry.error = Some(message.clone());
                save_entry(&path, &entry);
                shared.notice(
                    crate::service::NoticeLevel::Error,
                    format!("Print job failed: {message}"),
                );
                set_relay_job(&shared, &saved, entry.job_ref, "Failed");
            }
            UploadOutcome::Cancelled => {
                shared.clear_cancel(entry.job_ref);
                shared.mark_cancelled(entry.job_ref, "cancelled before printing");
                let _ = std::fs::remove_file(&bin_path);
                let _ = std::fs::remove_file(&path);
                set_relay_job(&shared, &saved, entry.job_ref, "Cancelled");
                shared.notice(
                    crate::service::NoticeLevel::Info,
                    format!("Cancelled '{}'", entry.doc_name),
                );
            }
            UploadOutcome::Revoked => {
                shared.update_job(entry.job_ref, |job| {
                    job.state = "Revoked".to_string();
                    job.error = Some("this PC was removed on the host".to_string());
                    job.finished = Some(config::now_unix());
                });
                let job = shared
                    .core()
                    .jobs
                    .into_iter()
                    .find(|job| job.id == entry.job_ref);
                if let Some(job) = job {
                    crate::service::append_job(&job);
                }
                let _ = std::fs::remove_file(&bin_path);
                entry.state = "Revoked".to_string();
                save_entry(&path, &entry);
                let _ = set_queue_paused(&saved.queue_name, true);
                shared.notice(
                    crate::service::NoticeLevel::Error,
                    format!(
                        "{}: this PC was removed on the host. Re-pair to continue printing.",
                        saved.queue_name
                    ),
                );
                set_relay_job(&shared, &saved, entry.job_ref, "Revoked");
            }
        }
    }
}

fn relay_view(shared: &Shared, saved: &SavedPrinter, listening: bool, error: Option<String>) -> RelayView {
    let previous = shared.relay_for(saved.id);
    RelayView {
        saved_id: saved.id,
        queue_name: saved.queue_name.clone(),
        port: saved.relay_port,
        listening,
        last_error: error,
        last_job: previous.and_then(|view| view.last_job),
    }
}

fn set_relay_job(shared: &Shared, saved: &SavedPrinter, job_ref: Uuid, state: &str) {
    let previous = shared.relay_for(saved.id);
    shared.set_relay(RelayView {
        saved_id: saved.id,
        queue_name: saved.queue_name.clone(),
        port: saved.relay_port,
        listening: previous.map(|view| view.listening).unwrap_or(true),
        last_error: None,
        last_job: Some(format!("{} {state}", &job_ref.simple().to_string()[..8])),
    });
}

async fn listen_task(shared: Arc<Shared>, saved: SavedPrinter) {
    let addr = SocketAddr::from(([127, 0, 0, 1], saved.relay_port));
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            let message = format!("port {} busy: {err}", saved.relay_port);
            shared.set_relay(relay_view(&shared, &saved, false, Some(message.clone())));
            shared.notice(
                crate::service::NoticeLevel::Error,
                format!("{}: {message}", saved.queue_name),
            );
            return;
        }
    };
    shared.set_relay(relay_view(&shared, &saved, true, None));
    tracing::info!("relay for '{}' listening on {addr}", saved.queue_name);

    let mut shutdown = shared.shutdown_tx.subscribe();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((socket, peer)) => {
                        tracing::debug!("spool connection from {peer} for '{}'", saved.queue_name);
                        spool_connection(shared.clone(), saved.clone(), socket).await;
                    }
                    Err(err) => {
                        tracing::warn!("relay accept failed: {err}");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            }
            _ = shutdown.changed() => break,
        }
    }
    shared.set_relay(relay_view(&shared, &saved, false, None));
}

async fn upload_task(shared: Arc<Shared>, saved: SavedPrinter) {
    let mut shutdown = shared.shutdown_tx.subscribe();
    loop {
        process_outbox(shared.clone(), saved.clone()).await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown.changed() => break,
        }
    }
}

struct RunningRelay {
    signature: String,
    listener: tokio::task::JoinHandle<()>,
    uploader: tokio::task::JoinHandle<()>,
}

fn relay_signature(saved: &SavedPrinter) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        saved.relay_port,
        saved.host_addr,
        saved.printer_id,
        saved.client_id,
        saved.token_dpapi.len()
    )
}

pub fn spawn_relay_supervisor(shared: Arc<Shared>) {
    let runtime = shared.runtime.clone();
    runtime.spawn(async move {
        let mut running: HashMap<Uuid, RunningRelay> = HashMap::new();
        let mut shutdown = shared.shutdown_tx.subscribe();
        loop {
            let saved_list = shared.saved_printers();
            let mut seen = HashSet::new();
            for saved in saved_list.iter().filter(|saved| saved.installed) {
                seen.insert(saved.id);
                let signature = relay_signature(saved);
                let restart = running
                    .get(&saved.id)
                    .map(|relay| relay.signature != signature)
                    .unwrap_or(true);
                if restart {
                    if let Some(old) = running.remove(&saved.id) {
                        old.listener.abort();
                        old.uploader.abort();
                    }
                    let listener = tokio::spawn(listen_task(shared.clone(), saved.clone()));
                    let uploader = tokio::spawn(upload_task(shared.clone(), saved.clone()));
                    running.insert(
                        saved.id,
                        RunningRelay {
                            signature,
                            listener,
                            uploader,
                        },
                    );
                }
            }
            running.retain(|id, relay| {
                if seen.contains(id) {
                    return true;
                }
                relay.listener.abort();
                relay.uploader.abort();
                shared.remove_relay(*id);
                false
            });

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = shutdown.changed() => break,
            }
        }
        for (_, relay) in running {
            relay.listener.abort();
            relay.uploader.abort();
        }
    });
}
