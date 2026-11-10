//! Engine/UI split: the UI either owns the engine (single-user install) or
//! talks to a machine-wide engine over a named pipe (all-users install).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use uuid::Uuid;

use crate::config::{self, AppConfig, PairedClient, SavedPrinter};
use crate::service::{CoreState, DiscoveredHost, JobRecord, NoticeLevel, Shared};

pub const ENGINE_PIPE: &str = r"\\.\pipe\peerprint-engine";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const PAIR_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_REQUEST: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityView {
    pub host_id: String,
    pub tls_fp: String,
}

impl Default for IdentityView {
    fn default() -> Self {
        Self {
            host_id: String::new(),
            tls_fp: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub pid: u32,
    pub updated_at: i64,
    pub data_dir: String,
    pub machine_wide: bool,
    pub config: AppConfig,
    pub core: CoreState,
    pub paired_clients: Vec<PairedClient>,
    pub identity: IdentityView,
    pub firewall_rules: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: 0,
            updated_at: 0,
            data_dir: String::new(),
            machine_wide: false,
            config: AppConfig::default(),
            core: CoreState::default(),
            paired_clients: Vec::new(),
            identity: IdentityView::default(),
            firewall_rules: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Snapshot,
    Ping,
    SetConfig { config: AppConfig },
    RefreshPrinters,
    Notice { level: NoticeLevel, message: String },
    ClearNotices,
    CancelJob { id: Uuid },
    CancelClientJob { id: Uuid },
    RevokeClient { id: Uuid },
    Pair {
        host: DiscoveredHost,
        printer_id: Uuid,
        pin: String,
    },
    RemovePrinter { id: Uuid },
    TestPage { queue: String },
    Probe { addr: String },
    Firewall,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Ok {
        #[serde(default)]
        snapshot: Option<Box<Snapshot>>,
    },
    Saved {
        printer: Box<SavedPrinter>,
    },
    Error {
        message: String,
    },
}

fn ok() -> Response {
    Response::Ok { snapshot: None }
}

pub async fn send_request(request: &Request, timeout: Duration) -> Result<Response> {
    let exchange = async {
        let mut pipe = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(ENGINE_PIPE)
            .context("the peerPrint background service is not running")?;
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        pipe.write_all(&line).await?;
        pipe.flush().await?;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let read = pipe.read(&mut chunk).await?;
            if read == 0 {
                bail!("the peerPrint service closed the connection");
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.contains(&b'\n') || buffer.len() > MAX_REQUEST {
                break;
            }
        }
        Ok(serde_json::from_slice::<Response>(&buffer)?)
    };
    tokio::time::timeout(timeout, exchange)
        .await
        .context("the peerPrint service did not answer")?
}

fn take_ok(response: Response) -> Result<Option<Snapshot>> {
    match response {
        Response::Ok { snapshot } => Ok(snapshot.map(|snapshot| *snapshot)),
        Response::Error { message } => bail!(message),
        Response::Saved { .. } => bail!("unexpected reply from the peerPrint service"),
    }
}

struct RemoteState {
    snapshot: Snapshot,
    connected: bool,
    last_error: Option<String>,
}

/// UI-side handle to a machine-wide engine.
pub struct Remote {
    state: Mutex<RemoteState>,
    runtime: Handle,
    stopping: AtomicBool,
}

impl Remote {
    fn new(runtime: Handle) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RemoteState {
                snapshot: Snapshot::default(),
                connected: false,
                last_error: None,
            }),
            runtime,
            stopping: AtomicBool::new(false),
        })
    }

    pub fn start(runtime: Handle) -> Arc<Self> {
        let remote = Self::new(runtime);
        let poller = remote.clone();
        remote.runtime.spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(600));
            while !poller.stopping.load(Ordering::SeqCst) {
                ticker.tick().await;
                match send_request(&Request::Snapshot, Duration::from_secs(2)).await {
                    Ok(response) => match take_ok(response).and_then(|snapshot| {
                        snapshot.context("the service sent no state")
                    }) {
                        Ok(snapshot) => {
                            let mut state = poller.state.lock().expect("remote lock");
                            state.snapshot = snapshot;
                            state.connected = true;
                            state.last_error = None;
                        }
                        Err(err) => poller.mark_error(format!("{err:#}")),
                    },
                    Err(err) => poller.mark_error(format!("{err:#}")),
                }
            }
        });
        remote
    }

    fn mark_error(&self, message: String) {
        let mut state = self.state.lock().expect("remote lock");
        state.connected = false;
        state.last_error = Some(message);
    }

    fn state(&self) -> RemoteState {
        let state = self.state.lock().expect("remote lock");
        RemoteState {
            snapshot: state.snapshot.clone(),
            connected: state.connected,
            last_error: state.last_error.clone(),
        }
    }

    fn call(&self, request: Request, timeout: Duration) -> Result<Response> {
        self.runtime.block_on(send_request(&request, timeout))
    }

    fn set_config(&self, cfg: AppConfig) -> Result<()> {
        take_ok(
            self.call(
                Request::SetConfig {
                    config: cfg.clone(),
                },
                REQUEST_TIMEOUT,
            )?,
        )?;
        self.state.lock().expect("remote lock").snapshot.config = cfg;
        Ok(())
    }

    fn send(&self, request: Request) -> Result<()> {
        take_ok(self.call(request, REQUEST_TIMEOUT)?)?;
        Ok(())
    }
}

/// Everything the UI needs from the engine, whichever process hosts it.
pub enum Backend {
    Local(Arc<Shared>),
    Remote(Arc<Remote>),
}

impl Backend {
    pub fn local(shared: Arc<Shared>) -> Arc<Self> {
        Arc::new(Self::Local(shared))
    }

    pub fn remote(runtime: Handle) -> Arc<Self> {
        Arc::new(Self::Remote(Remote::start(runtime)))
    }

    pub fn runtime(&self) -> Handle {
        match self {
            Backend::Local(shared) => shared.runtime.clone(),
            Backend::Remote(remote) => remote.runtime.clone(),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Backend::Remote(_))
    }

    pub fn config(&self) -> AppConfig {
        match self {
            Backend::Local(shared) => shared.config(),
            Backend::Remote(remote) => remote.state.lock().expect("remote lock").snapshot.config.clone(),
        }
    }

    pub fn core(&self) -> CoreState {
        match self {
            Backend::Local(shared) => shared.core(),
            Backend::Remote(remote) => remote.state.lock().expect("remote lock").snapshot.core.clone(),
        }
    }

    pub fn paired_clients(&self) -> Vec<PairedClient> {
        match self {
            Backend::Local(shared) => shared.paired_clients(),
            Backend::Remote(remote) => remote
                .state
                .lock()
                .expect("remote lock")
                .snapshot
                .paired_clients
                .clone(),
        }
    }

    pub fn identity(&self) -> IdentityView {
        match self {
            Backend::Local(shared) => IdentityView {
                host_id: shared.identity.host_id.to_string(),
                tls_fp: hex::encode(shared.identity.fingerprint),
            },
            Backend::Remote(remote) => remote
                .state
                .lock()
                .expect("remote lock")
                .snapshot
                .identity
                .clone(),
        }
    }

    pub fn data_dir(&self) -> String {
        match self {
            Backend::Local(_) => config::data_dir().display().to_string(),
            Backend::Remote(remote) => remote
                .state
                .lock()
                .expect("remote lock")
                .snapshot
                .data_dir
                .clone(),
        }
    }

    pub fn firewall_rules(&self) -> bool {
        match self {
            Backend::Local(_) => crate::win::firewall::has_rules(),
            Backend::Remote(remote) => remote
                .state
                .lock()
                .expect("remote lock")
                .snapshot
                .firewall_rules,
        }
    }

    pub fn connected(&self) -> bool {
        match self {
            Backend::Local(_) => true,
            Backend::Remote(remote) => remote.state().connected,
        }
    }

    pub fn last_error(&self) -> Option<String> {
        match self {
            Backend::Local(_) => None,
            Backend::Remote(remote) => remote.state().last_error,
        }
    }

    pub fn set_config(&self, cfg: AppConfig) -> Result<()> {
        match self {
            Backend::Local(shared) => shared.set_config(cfg),
            Backend::Remote(remote) => remote.set_config(cfg),
        }
    }

    pub fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        let message = message.into();
        match self {
            Backend::Local(shared) => shared.notice(level, message),
            Backend::Remote(remote) => {
                if let Err(err) = remote.send(Request::Notice {
                    level,
                    message: message.clone(),
                }) {
                    tracing::warn!("could not deliver notice to the service: {err:#}");
                }
            }
        }
    }

    pub fn clear_notices(&self) {
        match self {
            Backend::Local(shared) => shared.clear_notices(),
            Backend::Remote(remote) => {
                if let Err(err) = remote.send(Request::ClearNotices) {
                    tracing::warn!("could not clear notices in the service: {err:#}");
                }
                remote
                    .state
                    .lock()
                    .expect("remote lock")
                    .snapshot
                    .core
                    .notices
                    .clear();
            }
        }
    }

    pub fn refresh_local_printers(&self) {
        match self {
            Backend::Local(shared) => shared.refresh_local_printers(),
            Backend::Remote(remote) => {
                if let Err(err) = remote.send(Request::RefreshPrinters) {
                    tracing::warn!("could not refresh printers in the service: {err:#}");
                }
            }
        }
    }

    pub fn request_cancel(&self, id: Uuid) {
        match self {
            Backend::Local(shared) => shared.request_cancel(id),
            Backend::Remote(remote) => {
                if let Err(err) = remote.send(Request::CancelJob { id }) {
                    tracing::warn!("could not cancel in the service: {err:#}");
                }
            }
        }
    }

    pub fn cancel_client_job(&self, job: &JobRecord) {
        match self {
            Backend::Local(shared) => crate::client::cancel_client_job(shared, job),
            Backend::Remote(remote) => {
                if let Err(err) = remote.send(Request::CancelClientJob { id: job.id }) {
                    tracing::warn!("could not cancel in the service: {err:#}");
                }
            }
        }
    }

    pub fn revoke_client(&self, id: Uuid) -> Result<bool> {
        match self {
            Backend::Local(shared) => {
                let mut store = shared.clients.lock().expect("clients lock");
                if !store.revoke(id) {
                    return Ok(false);
                }
                config::save_paired_clients(&store)?;
                Ok(true)
            }
            Backend::Remote(remote) => match remote.call(Request::RevokeClient { id }, REQUEST_TIMEOUT)? {
                Response::Ok { .. } => Ok(true),
                Response::Error { message } => bail!(message),
                Response::Saved { .. } => bail!("unexpected reply from the peerPrint service"),
            },
        }
    }

    pub async fn pair(
        &self,
        host: &DiscoveredHost,
        printer_id: Uuid,
        pin: &str,
    ) -> Result<SavedPrinter> {
        match self {
            Backend::Local(shared) => {
                crate::client::add_printer(shared.clone(), host, printer_id, pin).await
            }
            Backend::Remote(_) => {
                let response = send_request(
                    &Request::Pair {
                        host: host.clone(),
                        printer_id,
                        pin: pin.to_string(),
                    },
                    PAIR_TIMEOUT,
                )
                .await?;
                match response {
                    Response::Saved { printer } => Ok(*printer),
                    Response::Error { message } => bail!(message),
                    Response::Ok { .. } => bail!("unexpected reply from the peerPrint service"),
                }
            }
        }
    }

    pub async fn remove_printer(&self, id: Uuid) -> Result<()> {
        match self {
            Backend::Local(shared) => crate::client::remove_printer(shared.clone(), id).await,
            Backend::Remote(_) => match send_request(&Request::RemovePrinter { id }, PAIR_TIMEOUT).await? {
                Response::Error { message } => bail!(message),
                _ => Ok(()),
            },
        }
    }

    pub async fn test_page(&self, queue: String) -> Result<()> {
        match self {
            Backend::Local(_) => crate::client::test_page(queue).await,
            Backend::Remote(_) => match send_request(&Request::TestPage { queue }, PAIR_TIMEOUT).await? {
                Response::Error { message } => bail!(message),
                _ => Ok(()),
            },
        }
    }

    pub async fn probe(&self, addr: std::net::SocketAddr) -> Result<()> {
        match self {
            Backend::Local(shared) => crate::discovery::probe_manual(shared, addr)
                .await
                .map(|_| ()),
            Backend::Remote(_) => match send_request(
                &Request::Probe {
                    addr: addr.to_string(),
                },
                PAIR_TIMEOUT,
            )
            .await?
            {
                Response::Error { message } => bail!(message),
                _ => Ok(()),
            },
        }
    }

    pub async fn setup_firewall(&self) -> Result<()> {
        match self {
            Backend::Local(shared) => {
                let code = tokio::task::spawn_blocking(|| {
                    crate::win::elevation::run_elevated("--firewall-setup")
                })
                .await
                .context("firewall task failed")?
                .map_err(|err| anyhow::anyhow!("{err:#}"))?;
                if code != 0 {
                    bail!("firewall setup failed (exit code {code})");
                }
                let mut cfg = shared.config();
                cfg.host.firewall_configured = true;
                shared.set_config(cfg)?;
                Ok(())
            }
            Backend::Remote(_) => match send_request(&Request::Firewall, PAIR_TIMEOUT).await? {
                Response::Error { message } => bail!(message),
                _ => Ok(()),
            },
        }
    }
}

pub fn local_snapshot(shared: &Shared, firewall_rules: &AtomicBool) -> Snapshot {
    Snapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        updated_at: config::now_unix(),
        data_dir: config::data_dir().display().to_string(),
        machine_wide: config::is_machine_wide(),
        config: shared.config(),
        core: shared.core(),
        paired_clients: shared.paired_clients(),
        identity: IdentityView {
            host_id: shared.identity.host_id.to_string(),
            tls_fp: hex::encode(shared.identity.fingerprint),
        },
        firewall_rules: firewall_rules.load(Ordering::SeqCst),
    }
}

// ---------------------------------------------------------------------------
// Engine side: accept UI connections.

pub fn start_engine_server(shared: Arc<Shared>, firewall_rules: Arc<AtomicBool>) {
    let runtime = shared.runtime.clone();
    runtime.spawn(engine_loop(shared, firewall_rules));
}

async fn engine_loop(shared: Arc<Shared>, firewall_rules: Arc<AtomicBool>) {
    let mut shutdown = shared.shutdown_tx.subscribe();
    let security = PipeSecurity::new();
    loop {
        if *shared.shutdown_tx.borrow() {
            break;
        }
        let options = tokio::net::windows::named_pipe::ServerOptions::new();
        let server = unsafe {
            match &security {
                Some(security) => options.create_with_security_attributes_raw(
                    ENGINE_PIPE,
                    security.attributes() as *mut std::ffi::c_void,
                ),
                None => options.create(ENGINE_PIPE),
            }
        };
        let server = match server {
            Ok(server) => server,
            Err(err) => {
                tracing::warn!("engine pipe unavailable: {err}");
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
        let shared = shared.clone();
        let firewall_rules = firewall_rules.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(server, shared, firewall_rules).await {
                tracing::debug!("engine request failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(
    mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    shared: Arc<Shared>,
    firewall_rules: Arc<AtomicBool>,
) -> Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = pipe.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.contains(&b'\n') || buffer.len() > MAX_REQUEST {
            break;
        }
    }
    let request: Request = serde_json::from_slice(&buffer).context("bad request")?;
    let response = dispatch(request, &shared, &firewall_rules).await;
    let mut line = serde_json::to_vec(&response)?;
    line.push(b'\n');
    pipe.write_all(&line).await?;
    pipe.flush().await?;
    let _ = pipe.disconnect();
    Ok(())
}

async fn dispatch(
    request: Request,
    shared: &Arc<Shared>,
    firewall_rules: &Arc<AtomicBool>,
) -> Response {
    let result: Result<Option<Box<Snapshot>>> = match request {
        Request::Snapshot => {
            return Response::Ok {
                snapshot: Some(Box::new(local_snapshot(shared, firewall_rules))),
            };
        }
        Request::Ping => Ok(None),
        Request::SetConfig { config } => shared.set_config(config).map(|_| None),
        Request::RefreshPrinters => {
            shared.refresh_local_printers();
            Ok(None)
        }
        Request::Notice { level, message } => {
            shared.notice(level, message);
            Ok(None)
        }
        Request::ClearNotices => {
            shared.clear_notices();
            Ok(None)
        }
        Request::CancelJob { id } => {
            shared.request_cancel(id);
            Ok(None)
        }
        Request::CancelClientJob { id } => {
            let job = shared.core().jobs.into_iter().find(|job| job.id == id);
            match job {
                Some(job) => {
                    crate::client::cancel_client_job(shared, &job);
                    Ok(None)
                }
                None => Err(anyhow::anyhow!("job not found")),
            }
        }
        Request::RevokeClient { id } => {
            let mut store = shared.clients.lock().expect("clients lock");
            if store.revoke(id) {
                config::save_paired_clients(&store).map(|_| None)
            } else {
                Ok(None)
            }
        }
        Request::Pair {
            host,
            printer_id,
            pin,
        } => match crate::client::add_printer(shared.clone(), &host, printer_id, &pin).await {
            Ok(printer) => return Response::Saved {
                printer: Box::new(printer),
            },
            Err(err) => Err(err),
        },
        Request::RemovePrinter { id } => {
            crate::client::remove_printer(shared.clone(), id).await.map(|_| None)
        }
        Request::TestPage { queue } => crate::client::test_page(queue).await.map(|_| None),
        Request::Probe { addr } => match addr.parse::<std::net::SocketAddr>() {
            Ok(addr) => crate::discovery::probe_manual(shared, addr).await.map(|_| None),
            Err(err) => Err(anyhow::anyhow!("bad address: {err}")),
        },
        Request::Firewall => {
            let port = shared.config().host.listen_port;
            match crate::win::firewall::ensure_rules(port) {
                Ok(()) => {
                    let mut cfg = shared.config();
                    cfg.host.firewall_configured = true;
                    let saved = shared.set_config(cfg);
                    firewall_rules.store(true, Ordering::SeqCst);
                    saved.map(|_| None)
                }
                Err(err) => Err(err),
            }
        }
    };

    match result {
        Ok(None) => ok(),
        Ok(Some(_)) => ok(),
        Err(err) => Response::Error {
            message: format!("{err:#}"),
        },
    }
}

/// Grants built-in users read/write access to the engine pipe; without this a
/// LocalSystem service creates a pipe that standard users cannot open.
struct PipeSecurity {
    descriptor: windows::Win32::Security::PSECURITY_DESCRIPTOR,
    attributes: windows::Win32::Security::SECURITY_ATTRIBUTES,
}

impl PipeSecurity {
    fn new() -> Option<Self> {
        use windows::core::PCWSTR;
        use windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        let sddl: Vec<u16> = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;BU)"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .is_err()
            {
                tracing::warn!("could not build engine pipe security descriptor");
                return None;
            }
        }
        let attributes = windows::Win32::Security::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: windows::core::BOOL(0),
        };
        Some(Self {
            descriptor,
            attributes,
        })
    }

    fn attributes(&self) -> *const windows::Win32::Security::SECURITY_ATTRIBUTES {
        &self.attributes
    }
}

// The descriptor is immutable process memory; the async engine loop may resume
// this value on another runtime thread between awaits.
unsafe impl Send for PipeSecurity {}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
                self.descriptor.0,
            )));
        }
    }
}

/// Blocking snapshot fetch used by `--diagnose` in machine-wide mode.
pub fn fetch_snapshot(runtime: &Handle) -> Result<Snapshot> {
    let response = runtime.block_on(send_request(&Request::Snapshot, REQUEST_TIMEOUT))?;
    take_ok(response)?.context("the service returned no state")
}
