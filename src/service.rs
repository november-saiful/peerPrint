use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::{
    self, AppConfig, PairedClient, PairedClients, SavedPrinter, SharedPrinter,
};
use crate::identity::HostIdentity;
use crate::net::protocol::{PrinterMeta, PrinterStatusInfo};
use crate::pairing::PinLimiter;
use crate::win::spooler::{self, LocalPrinter};

const CANCEL_REQUEST_TTL: i64 = 120;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobDirection {
    Host,
    Client,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: Uuid,
    pub direction: JobDirection,
    pub printer_name: String,
    pub peer: String,
    pub doc_name: String,
    pub bytes: u64,
    pub state: String,
    pub pages: u32,
    pub error: Option<String>,
    pub created: i64,
    pub finished: Option<i64>,
}

impl JobRecord {
    pub fn new(
        id: Uuid,
        direction: JobDirection,
        printer_name: String,
        peer: String,
        doc_name: String,
    ) -> Self {
        Self {
            id,
            direction,
            printer_name,
            peer,
            doc_name,
            bytes: 0,
            state: "Queued".to_string(),
            pages: 0,
            error: None,
            created: config::now_unix(),
            finished: None,
        }
    }

    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "Printed" | "Failed" | "Cancelled" | "Revoked"
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalPrinterView {
    pub name: String,
    pub port_name: String,
    pub driver_name: String,
    pub driver_env: String,
    pub driver_version: u64,
    pub hardware_id: String,
    pub status: u32,
    pub status_text: String,
    pub online: bool,
    pub jobs: u32,
    pub shareable: bool,
}

impl From<&LocalPrinter> for LocalPrinterView {
    fn from(printer: &LocalPrinter) -> Self {
        let offline = printer.status
            & (windows::Win32::Graphics::Printing::PRINTER_STATUS_OFFLINE
                | windows::Win32::Graphics::Printing::PRINTER_STATUS_ERROR
                | windows::Win32::Graphics::Printing::PRINTER_STATUS_NOT_AVAILABLE)
            != 0;
        Self {
            name: printer.name.clone(),
            port_name: printer.port_name.clone(),
            driver_name: printer.driver_name.clone(),
            driver_env: printer.driver_env.clone(),
            driver_version: printer.driver_version,
            hardware_id: printer.hardware_id.clone(),
            status: printer.status,
            status_text: spooler::status_text(printer.status),
            online: !offline,
            jobs: printer.jobs,
            shareable: spooler::is_shareable(printer),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayView {
    pub saved_id: Uuid,
    pub queue_name: String,
    pub port: u16,
    pub listening: bool,
    pub last_error: Option<String>,
    pub last_job: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredHost {
    pub host_id: Uuid,
    pub instance: String,
    pub addr: String,
    pub display_name: String,
    pub pin_required: bool,
    pub tls_fp: String,
    pub printer_count: u32,
    pub printers: Vec<PrinterMeta>,
    pub last_seen: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Notice {
    /// Ordered across service restarts so a UI can tell new notices from old ones.
    #[serde(default)]
    pub id: u64,
    pub level: NoticeLevel,
    pub message: String,
    pub at: i64,
}

fn notice_id_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
        .saturating_mul(1000)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CoreState {
    pub host_running: bool,
    pub host_error: Option<String>,
    pub local_printers: Vec<LocalPrinterView>,
    pub printer_status: HashMap<Uuid, PrinterStatusInfo>,
    pub discovered: Vec<DiscoveredHost>,
    pub jobs: Vec<JobRecord>,
    pub relays: Vec<RelayView>,
    pub notices: VecDeque<Notice>,
}

#[derive(Clone, Debug)]
pub struct RecentJob {
    pub finished: i64,
    pub pages: u32,
    pub failure: Option<String>,
}

pub struct Shared {
    pub config: Mutex<AppConfig>,
    pub core: Mutex<CoreState>,
    pub clients: Mutex<PairedClients>,
    /// Recently finished host jobs, used to make retried uploads idempotent.
    pub recent_jobs: Mutex<HashMap<Uuid, RecentJob>>,
    /// Jobs the user asked to cancel, with the request time (unix seconds).
    pub cancel_requests: Mutex<HashMap<Uuid, i64>>,
    pub identity: HostIdentity,
    pub pin_limiter: PinLimiter,
    pub notice_seq: AtomicU64,
    pub shutdown_tx: watch::Sender<bool>,
    pub runtime: Handle,
}

impl Shared {
    pub fn new(runtime: Handle) -> Result<Arc<Self>> {
        config::ensure_dirs()?;
        let cfg = AppConfig::load();
        let identity = crate::identity::load_or_create()?;
        let clients = config::load_paired_clients();
        let jobs = load_jobs();
        let (shutdown_tx, _) = watch::channel(false);
        let shared = Arc::new(Self {
            config: Mutex::new(cfg),
            core: Mutex::new(CoreState {
                jobs,
                ..Default::default()
            }),
            clients: Mutex::new(clients),
            recent_jobs: Mutex::new(HashMap::new()),
            cancel_requests: Mutex::new(HashMap::new()),
            identity,
            pin_limiter: PinLimiter::default(),
            notice_seq: AtomicU64::new(notice_id_seed()),
            shutdown_tx,
            runtime,
        });
        shared.refresh_local_printers();
        Ok(shared)
    }

    pub fn config(&self) -> AppConfig {
        self.config.lock().expect("config lock").clone()
    }

    pub fn core(&self) -> CoreState {
        self.core.lock().expect("core lock").clone()
    }

    pub fn paired_clients(&self) -> Vec<PairedClient> {
        self.clients.lock().expect("clients lock").clients.clone()
    }

    pub fn persist_config(&self) -> Result<()> {
        let cfg = self.config.lock().expect("config lock").clone();
        cfg.save()
    }

    pub fn set_config(&self, cfg: AppConfig) -> Result<()> {
        cfg.save()?;
        *self.config.lock().expect("config lock") = cfg;
        Ok(())
    }

    pub fn set_host_state(&self, running: bool, error: Option<String>) {
        let mut core = self.core.lock().expect("core lock");
        core.host_running = running;
        core.host_error = error;
    }

    pub fn remember_host_job(&self, client_job_ref: Uuid, pages: u32, failure: Option<String>) {
        let mut recent = self.recent_jobs.lock().expect("recent jobs lock");
        let cutoff = config::now_unix() - 24 * 60 * 60;
        recent.retain(|_, job| job.finished > cutoff);
        recent.insert(
            client_job_ref,
            RecentJob {
                finished: config::now_unix(),
                pages,
                failure,
            },
        );
    }

    pub fn recent_host_job(&self, client_job_ref: Uuid) -> Option<RecentJob> {
        self.recent_jobs
            .lock()
            .expect("recent jobs lock")
            .get(&client_job_ref)
            .cloned()
    }

    pub fn request_cancel(&self, id: Uuid) {
        let now = config::now_unix();
        let mut cancels = self.cancel_requests.lock().expect("cancel lock");
        cancels.retain(|_, at| now - *at < CANCEL_REQUEST_TTL);
        cancels.insert(id, now);
    }

    pub fn cancel_requested(&self, id: Uuid) -> bool {
        let cancels = self.cancel_requests.lock().expect("cancel lock");
        cancels
            .get(&id)
            .map(|at| config::now_unix() - *at < CANCEL_REQUEST_TTL)
            .unwrap_or(false)
    }

    pub fn clear_cancel(&self, id: Uuid) {
        self.cancel_requests
            .lock()
            .expect("cancel lock")
            .remove(&id);
    }

    pub fn job_state(&self, id: Uuid) -> Option<String> {
        self.core
            .lock()
            .expect("core lock")
            .jobs
            .iter()
            .find(|job| job.id == id)
            .map(|job| job.state.clone())
    }

    /// Marks a known job as cancelled and appends it to the history file.
    pub fn mark_cancelled(&self, id: Uuid, reason: &str) {
        self.update_job(id, |job| {
            job.state = "Cancelled".to_string();
            job.error = Some(reason.to_string());
            job.finished = Some(config::now_unix());
        });
        if let Some(job) = self.core().jobs.into_iter().find(|job| job.id == id) {
            append_job(&job);
        }
    }

    pub fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        let id = self.notice_seq.fetch_add(1, Ordering::Relaxed);
        let mut core = self.core.lock().expect("core lock");
        core.notices.push_front(Notice {
            id,
            level,
            message: message.into(),
            at: config::now_unix(),
        });
        core.notices.truncate(50);
    }

    pub fn clear_notices(&self) {
        self.core.lock().expect("core lock").notices.clear();
    }

    pub fn refresh_local_printers(&self) {
        match spooler::list_printers() {
            Ok(printers) => {
                let cfg = self.config();
                let shared_names: Vec<String> = cfg
                    .host
                    .shared
                    .iter()
                    .map(|p| p.spooler_name.clone())
                    .collect();
                let views: Vec<LocalPrinterView> = printers
                    .iter()
                    .filter(|p| spooler::is_shareable(p) || shared_names.contains(&p.name))
                    .map(LocalPrinterView::from)
                    .collect();
                let mut core = self.core.lock().expect("core lock");
                core.local_printers = views;
                core.host_error = None;
            }
            Err(err) => {
                let mut core = self.core.lock().expect("core lock");
                core.host_error = Some(format!("Could not enumerate printers: {err:#}"));
            }
        }
    }

    pub fn shared_printer_by_id(&self, id: Uuid) -> Option<SharedPrinter> {
        self.config()
            .host
            .shared
            .into_iter()
            .find(|printer| printer.id == id && printer.enabled)
    }

    pub fn host_printers_meta(&self) -> Vec<PrinterMeta> {
        let cfg = self.config();
        let core = self.core.lock().expect("core lock");
        cfg.host
            .shared
            .iter()
            .filter(|printer| printer.enabled)
            .map(|printer| {
                let status = core.printer_status.get(&printer.id);
                PrinterMeta {
                    id: printer.id,
                    name: printer.display_name.clone(),
                    model: printer.model.clone(),
                    driver_name: printer.driver_name.clone(),
                    driver_env: printer.driver_env.clone(),
                    driver_version: printer.driver_version,
                    online: status.map(|s| s.online).unwrap_or(false),
                    status: status
                        .map(|s| s.status.clone())
                        .unwrap_or_else(|| "Unknown".to_string()),
                    jobs: status.map(|s| s.jobs).unwrap_or(0),
                }
            })
            .collect()
    }

    pub fn host_printer_statuses(&self) -> Vec<PrinterStatusInfo> {
        let cfg = self.config();
        let core = self.core.lock().expect("core lock");
        cfg.host
            .shared
            .iter()
            .filter(|printer| printer.enabled)
            .map(|printer| {
                core.printer_status.get(&printer.id).cloned().unwrap_or(PrinterStatusInfo {
                    id: printer.id,
                    online: false,
                    status: "Unknown".to_string(),
                    jobs: 0,
                })
            })
            .collect()
    }

    pub fn record_job(&self, job: JobRecord, persist: bool) {
        {
            let mut core = self.core.lock().expect("core lock");
            if let Some(existing) = core.jobs.iter_mut().find(|existing| existing.id == job.id) {
                *existing = job.clone();
            } else {
                core.jobs.insert(0, job.clone());
                core.jobs.truncate(200);
            }
        }
        if persist {
            append_job(&job);
        }
    }

    pub fn update_job(&self, id: Uuid, update: impl FnOnce(&mut JobRecord)) {
        let mut core = self.core.lock().expect("core lock");
        if let Some(job) = core.jobs.iter_mut().find(|job| job.id == id) {
            update(job);
        }
    }

    pub fn set_relay(&self, view: RelayView) {
        let mut core = self.core.lock().expect("core lock");
        if let Some(existing) = core
            .relays
            .iter_mut()
            .find(|relay| relay.saved_id == view.saved_id)
        {
            *existing = view;
        } else {
            core.relays.push(view);
        }
    }

    pub fn remove_relay(&self, saved_id: Uuid) {
        let mut core = self.core.lock().expect("core lock");
        core.relays.retain(|relay| relay.saved_id != saved_id);
    }

    pub fn relay_for(&self, saved_id: Uuid) -> Option<RelayView> {
        self.core
            .lock()
            .expect("core lock")
            .relays
            .iter()
            .find(|relay| relay.saved_id == saved_id)
            .cloned()
    }

    pub fn upsert_discovered(&self, host: DiscoveredHost) {
        let mut core = self.core.lock().expect("core lock");
        if let Some(existing) = core
            .discovered
            .iter_mut()
            .find(|existing| existing.host_id == host.host_id)
        {
            let printers = if host.printers.is_empty() {
                std::mem::take(&mut existing.printers)
            } else {
                host.printers.clone()
            };
            *existing = DiscoveredHost { printers, ..host };
        } else {
            core.discovered.push(host);
        }
        core.discovered.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    }

    pub fn remove_discovered(&self, instance: &str) {
        let mut core = self.core.lock().expect("core lock");
        core.discovered.retain(|host| host.instance != instance);
    }

    pub fn saved_printers(&self) -> Vec<SavedPrinter> {
        self.config().client.saved
    }
}

fn jobs_path() -> std::path::PathBuf {
    config::data_dir().join("jobs.jsonl")
}

pub fn append_job(job: &JobRecord) {
    use std::io::Write;
    let path = jobs_path();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        if let Ok(line) = serde_json::to_string(job) {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn load_jobs() -> Vec<JobRecord> {
    let text = std::fs::read_to_string(jobs_path()).unwrap_or_default();
    let mut by_id: HashMap<Uuid, JobRecord> = HashMap::new();
    for line in text.lines() {
        if let Ok(job) = serde_json::from_str::<JobRecord>(line) {
            by_id.insert(job.id, job);
        }
    }
    let mut jobs: Vec<JobRecord> = by_id.into_values().collect();
    jobs.sort_by(|a, b| b.created.cmp(&a.created));
    jobs.truncate(200);
    jobs
}

pub async fn start_services(shared: Arc<Shared>, interactive: bool, engine: bool) {
    crate::net::tls::ensure_crypto();

    let role = shared.config().role;
    let firewall_rules = Arc::new(std::sync::atomic::AtomicBool::new(
        crate::win::firewall::has_rules(),
    ));
    if engine {
        crate::backend::start_engine_server(shared.clone(), firewall_rules);
    } else {
        crate::ipc::start_control_server(shared.clone(), interactive);
    }
    if role.hosts() {
        crate::host::spawn_host(shared.clone());
        crate::discovery::spawn_advertiser(shared.clone());
    }
    if role.clients() {
        crate::discovery::spawn_browser(shared.clone());
        crate::client::spawn_relay_supervisor(shared.clone());
    }

    let status_shared = shared.clone();
    tokio::spawn(async move {
        let mode = if engine {
            "service"
        } else if interactive {
            "ui"
        } else {
            "background"
        };
        let mut ticker = tokio::time::interval(Duration::from_secs(3));
        loop {
            ticker.tick().await;
            if *status_shared.shutdown_tx.borrow() {
                break;
            }
            crate::ipc::write_status(&status_shared, mode);
        }
    });

    let status_shared = shared.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(3));
        loop {
            ticker.tick().await;
            if *status_shared.shutdown_tx.borrow() {
                break;
            }
            let shared_printers = status_shared.config().host.shared;
            if shared_printers.is_empty() {
                continue;
            }
            let mut updates = Vec::new();
            for printer in shared_printers {
                if !printer.enabled {
                    continue;
                }
                match spooler::printer_status(&printer.spooler_name) {
                    Ok((status, jobs)) => {
                        let offline = status
                            & (windows::Win32::Graphics::Printing::PRINTER_STATUS_OFFLINE
                                | windows::Win32::Graphics::Printing::PRINTER_STATUS_ERROR
                                | windows::Win32::Graphics::Printing::PRINTER_STATUS_NOT_AVAILABLE)
                            != 0;
                        updates.push(PrinterStatusInfo {
                            id: printer.id,
                            online: !offline,
                            status: spooler::status_text(status),
                            jobs,
                        });
                    }
                    Err(_) => updates.push(PrinterStatusInfo {
                        id: printer.id,
                        online: false,
                        status: "Not available".to_string(),
                        jobs: 0,
                    }),
                }
            }
            let mut core = status_shared.core.lock().expect("core lock");
            for update in updates {
                core.printer_status.insert(update.id, update);
            }
        }
    });
}

pub fn shutdown(shared: &Shared) {
    let _ = shared.shutdown_tx.send(true);
}

pub fn spawn_background<F>(shared: &Shared, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    shared.runtime.spawn(future);
}

pub fn local_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

pub fn run_blocking<F, T>(_shared: &Shared, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::block_in_place(|| f()).context("background operation failed")
}

#[allow(dead_code)]
pub fn ensure_ready(shared: &Shared) -> bool {
    shared.core.lock().map(|c| c.host_running).unwrap_or(false)
}
