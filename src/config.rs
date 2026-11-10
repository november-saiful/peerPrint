use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::win::dpapi;

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_LISTEN_PORT: u16 = 39100;
pub const DEFAULT_RELAY_PORT_BASE: u16 = 39101;
pub const DEFAULT_OUTBOX_LIMIT_MB: u64 = 500;

static MACHINE_WIDE: AtomicBool = AtomicBool::new(false);

/// Switches state, identity and logs to the machine-wide ProgramData folder.
/// Set once at startup, before anything touches the data directory.
pub fn set_machine_wide(enabled: bool) {
    MACHINE_WIDE.store(enabled, Ordering::SeqCst);
}

pub fn is_machine_wide() -> bool {
    MACHINE_WIDE.load(Ordering::SeqCst)
}

fn default_true() -> bool {
    true
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}

fn default_listen_port() -> u16 {
    DEFAULT_LISTEN_PORT
}

fn default_relay_port_base() -> u16 {
    DEFAULT_RELAY_PORT_BASE
}

fn default_outbox_limit() -> u64 {
    DEFAULT_OUTBOX_LIMIT_MB
}

fn default_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "peerPrint Host".to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Client,
    Both,
}

impl Role {
    pub fn hosts(self) -> bool {
        matches!(self, Role::Host | Role::Both)
    }

    pub fn clients(self) -> bool {
        matches!(self, Role::Client | Role::Both)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "host" => Some(Role::Host),
            "client" => Some(Role::Client),
            "both" => Some(Role::Both),
            _ => None,
        }
    }
}

impl Default for Role {
    fn default() -> Self {
        Role::Both
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub host: HostConfig,
    #[serde(default)]
    pub client: ClientConfig,
    #[serde(default = "default_true")]
    pub autostart: bool,
    /// What this installation is allowed to do; chosen by the installer.
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub updates: UpdateConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            host: HostConfig::default(),
            client: ClientConfig::default(),
            autostart: true,
            role: Role::Both,
            updates: UpdateConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_true")]
    pub auto_check: bool,
    /// Unix time of the last check attempt, successful or not.
    #[serde(default)]
    pub last_check: i64,
    /// The release version the user was already notified about.
    #[serde(default)]
    pub last_notified_version: Option<String>,
    /// The newer release the last successful check found, kept so the Settings
    /// card stays accurate between daily checks.
    #[serde(default)]
    pub available: Option<CachedRelease>,
    /// Why the last check failed, cleared by the next successful one.
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRelease {
    pub version: String,
    pub html_url: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            last_check: 0,
            last_notified_version: None,
            available: None,
            last_error: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_hostname")]
    pub display_name: String,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub require_pin: bool,
    /// DPAPI-protected PIN. SPAKE2 needs the PIN value, not a one-way hash.
    #[serde(default)]
    pub pin_dpapi: Option<String>,
    #[serde(default)]
    pub shared: Vec<SharedPrinter>,
    #[serde(default)]
    pub firewall_configured: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            display_name: default_hostname(),
            listen_port: DEFAULT_LISTEN_PORT,
            require_pin: false,
            pin_dpapi: None,
            shared: Vec::new(),
            firewall_configured: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedPrinter {
    pub id: Uuid,
    /// Local Windows queue name on the host.
    pub spooler_name: String,
    /// Name shown to clients.
    pub display_name: String,
    pub model: String,
    pub driver_name: String,
    pub driver_env: String,
    pub driver_version: u64,
    pub port_name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default)]
    pub saved: Vec<SavedPrinter>,
    #[serde(default = "default_relay_port_base")]
    pub port_base: u16,
    #[serde(default = "default_outbox_limit")]
    pub outbox_limit_mb: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            saved: Vec::new(),
            port_base: DEFAULT_RELAY_PORT_BASE,
            outbox_limit_mb: default_outbox_limit(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedPrinter {
    pub id: Uuid,
    #[serde(default)]
    pub client_id: Uuid,
    pub host_id: String,
    pub host_addr: String,
    pub host_display: String,
    pub printer_id: Uuid,
    pub printer_name: String,
    pub printer_model: String,
    pub driver_name: String,
    pub driver_env: String,
    pub relay_port: u16,
    pub port_name: String,
    pub queue_name: String,
    /// Hex SHA-256 of the host certificate.
    pub tls_fp: String,
    /// DPAPI-protected 32-byte auth token.
    pub token_dpapi: String,
    pub installed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairedClient {
    pub id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PairedClients {
    pub clients: Vec<PairedClient>,
}

pub fn hash_token(client_id: Uuid, token: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token);
    hasher.update(client_id.as_bytes());
    hasher.finalize().into()
}

impl PairedClients {
    pub fn verify(&self, client_id: Uuid, token: &[u8; 32]) -> bool {
        let expected = hash_token(client_id, token);
        let expected_hex = hex::encode(expected);
        self.clients.iter().any(|client| {
            !client.revoked
                && client.id == client_id
                && client.token_hash.as_bytes().ct_eq(expected_hex.as_bytes()).into()
        })
    }

    pub fn touch(&mut self, client_id: Uuid) {
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) {
            client.last_seen = now_unix();
        }
    }

    pub fn revoke(&mut self, client_id: Uuid) -> bool {
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) {
            client.revoked = true;
            return true;
        }
        false
    }

    pub fn add(&mut self, id: Uuid, name: String, token_hash: String) {
        self.clients.push(PairedClient {
            id,
            name,
            token_hash,
            created_at: now_unix(),
            last_seen: now_unix(),
            revoked: false,
        });
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub fn data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("PEERPRINT_DATA_DIR") {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom);
        }
    }
    if is_machine_wide() {
        if let Ok(program_data) = std::env::var("ProgramData") {
            if !program_data.trim().is_empty() {
                return PathBuf::from(program_data).join("peerPrint");
            }
        }
        return PathBuf::from(r"C:\ProgramData\peerPrint");
    }
    user_data_dir()
}

/// The signed-in user's own state folder. The machine-wide tray UI uses it for
/// logs because ProgramData is locked to SYSTEM and Administrators.
pub fn user_data_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("PeerPrint")
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("PeerPrintData")
    }
}

pub fn ensure_dirs() -> Result<()> {
    for dir in [
        data_dir(),
        data_dir().join("identity"),
        data_dir().join("outbox"),
        data_dir().join("state"),
        data_dir().join("logs"),
        data_dir().join("ipc"),
    ] {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(())
}

fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

fn paired_clients_path() -> PathBuf {
    data_dir().join("paired_clients.json")
}

impl AppConfig {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<AppConfig>(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes)) {
                Ok(mut config) => {
                    if config.schema_version > SCHEMA_VERSION {
                        tracing::warn!("config schema is newer than this build");
                    }
                    config.schema_version = SCHEMA_VERSION;
                    if config.client.port_base < 1024 {
                        config.client.port_base = DEFAULT_RELAY_PORT_BASE;
                    }
                    if config.client.outbox_limit_mb == 0 {
                        config.client.outbox_limit_mb = default_outbox_limit();
                    }
                    config
                }
                Err(err) => {
                    tracing::error!("config parse failed: {err:#}");
                    let bad = path.with_extension(format!("bad.{}", now_unix()));
                    let _ = fs::copy(&path, &bad);
                    AppConfig::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => AppConfig::default(),
            Err(err) => {
                tracing::error!("config read failed: {err:#}");
                AppConfig::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn host_pin(&self) -> Option<String> {
        self.host
            .pin_dpapi
            .as_deref()
            .and_then(|value| dpapi::unprotect_string(value).ok())
    }

    pub fn set_host_pin(&mut self, pin: Option<&str>) -> Result<()> {
        self.host.pin_dpapi = match pin {
            Some(value) if !value.is_empty() => Some(dpapi::protect_string(value)?),
            _ => None,
        };
        Ok(())
    }
}

pub fn load_paired_clients() -> PairedClients {
    let path = paired_clients_path();
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PairedClients::default(),
        Err(err) => {
            tracing::error!("paired clients read failed: {err:#}");
            PairedClients::default()
        }
    }
}

pub fn save_paired_clients(clients: &PairedClients) -> Result<()> {
    let path = paired_clients_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(clients)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn token_from_saved(saved: &SavedPrinter) -> Option<[u8; 32]> {
    let mut token = [0u8; 32];
    let bytes = dpapi::unprotect_b64(&saved.token_dpapi).ok()?;
    if bytes.len() != token.len() {
        return None;
    }
    token.copy_from_slice(&bytes);
    Some(token)
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_verification_is_client_specific() {
        let token = [9u8; 32];
        let mut clients = PairedClients::default();
        let id = Uuid::new_v4();
        clients.add(id, "Laptop".to_string(), String::new());
        clients.clients[0].token_hash = hex::encode(hash_token(id, &token));

        assert!(clients.verify(id, &token));
        assert!(!clients.verify(id, &[8u8; 32]), "a different token must fail");
        assert!(
            !clients.verify(Uuid::new_v4(), &token),
            "the same token for another client id must fail"
        );
        assert!(clients.revoke(id));
        assert!(!clients.verify(id, &token), "revoked clients must fail");
    }

    #[test]
    fn queue_names_are_sanitized_for_windows() {
        assert_eq!(crate::client::sanitize_queue_name("Office, HP; laser"), "Office HP laser");
        assert_eq!(crate::client::sanitize_queue_name("  Canon  "), "Canon");
        assert!(!crate::client::sanitize_queue_name("CON").eq_ignore_ascii_case("CON"));
        assert_eq!(crate::client::sanitize_queue_name(""), "peerPrint printer");
    }

    #[test]
    fn install_spec_validation_rejects_unsafe_fields() {
        let good = crate::client::InstallSpec {
            port_name: "PeerPrint_abc123".to_string(),
            relay_port: 39101,
            queue_name: "Canon G2010 (PeerPrint)".to_string(),
            driver_name: "Canon G2010 series".to_string(),
            location: "Office PC".to_string(),
        };
        assert!(crate::client::validate_install_spec(&good).is_ok());

        let mut bad_port = good.clone();
        bad_port.port_name = "evil; Remove-Item".to_string();
        assert!(crate::client::validate_install_spec(&bad_port).is_err());

        let mut bad_relay = good.clone();
        bad_relay.relay_port = 80;
        assert!(crate::client::validate_install_spec(&bad_relay).is_err());

        let mut bad_queue = good.clone();
        bad_queue.queue_name = "bad,name".to_string();
        assert!(crate::client::validate_install_spec(&bad_queue).is_err());

        let mut bad_driver = good.clone();
        bad_driver.driver_name = "line\nbreak".to_string();
        assert!(crate::client::validate_install_spec(&bad_driver).is_err());
    }
}
