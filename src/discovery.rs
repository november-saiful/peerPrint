use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo, TxtProperties};
use uuid::Uuid;

use crate::config::now_unix;
use crate::service::{DiscoveredHost, Shared};

pub const SERVICE_TYPE: &str = "_peerprint._tcp.local.";
const ADVERT_INTERVAL: Duration = Duration::from_secs(2);
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

pub struct Advert {
    pub host_id: Uuid,
    pub display_name: String,
    pub pin_required: bool,
    pub tls_fp: String,
    pub printer_count: u32,
}

fn txt(props: &TxtProperties, key: &str) -> Option<String> {
    props
        .get_property_val_str(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn parse_txt(props: &TxtProperties) -> Option<Advert> {
    if txt(props, "v").as_deref() != Some("1") {
        return None;
    }
    let host_id = txt(props, "id").and_then(|value| Uuid::parse_str(&value).ok())?;
    Some(Advert {
        host_id,
        display_name: txt(props, "n").unwrap_or_else(|| "peerPrint host".to_string()),
        pin_required: txt(props, "pin").as_deref() == Some("1"),
        tls_fp: txt(props, "fp").unwrap_or_default(),
        printer_count: txt(props, "p")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    })
}

fn preferred_addr(addrs: &std::collections::HashSet<mdns_sd::ScopedIp>) -> Option<IpAddr> {
    let mut fallback = None;
    for scoped in addrs {
        if scoped.is_loopback() {
            continue;
        }
        let ip = scoped.to_ip_addr();
        match ip {
            IpAddr::V4(v4) if v4.is_private() => return Some(ip),
            IpAddr::V4(_) => fallback.get_or_insert(ip),
            IpAddr::V6(_) => fallback.get_or_insert(ip),
        };
    }
    fallback
}

fn instance_name(display_name: &str, host_id: Uuid) -> String {
    let cleaned: String = display_name
        .chars()
        .filter(|c| !c.is_control() && *c != '.')
        .take(48)
        .collect();
    if cleaned.trim().is_empty() {
        format!("peerPrint {}", &host_id.to_string()[..8])
    } else {
        cleaned.trim().to_string()
    }
}

pub fn spawn_advertiser(shared: Arc<Shared>) {
    let runtime = shared.runtime.clone();
    runtime.spawn(async move {
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => {
                shared.notice(
                    crate::service::NoticeLevel::Error,
                    format!("mDNS unavailable: {err}. Use manual IP:port add."),
                );
                return;
            }
        };

        let mut registered: Option<(String, String)> = None;
        let mut shutdown = shared.shutdown_tx.subscribe();
        loop {
            if *shutdown.borrow() {
                break;
            }

            let cfg = shared.config();
            let printers: Vec<_> = cfg
                .host
                .shared
                .iter()
                .filter(|printer| printer.enabled)
                .collect();
            let ip = crate::service::local_ipv4();
            let wanted = cfg.host.enabled && !printers.is_empty() && ip.is_some();

            if wanted {
                let ip = ip.unwrap_or(Ipv4Addr::LOCALHOST);
                let pin_required = cfg.host.require_pin && cfg.host.pin_dpapi.is_some();
                let fingerprint = hex::encode(shared.identity.fingerprint);
                let signature = format!(
                    "{}|{}|{}|{}|{}",
                    cfg.host.display_name,
                    cfg.host.listen_port,
                    pin_required,
                    fingerprint,
                    printers.len()
                );
                let stale = match &registered {
                    Some((_, current)) => current != &signature,
                    None => true,
                };
                if stale {
                    if let Some((fullname, _)) = registered.take() {
                        let _ = daemon.unregister(&fullname);
                    }
                    let instance = instance_name(&cfg.host.display_name, shared.identity.host_id);
                    let properties = vec![
                        ("v", "1".to_string()),
                        ("id", shared.identity.host_id.to_string()),
                        ("n", cfg.host.display_name.clone()),
                        ("pin", if pin_required { "1" } else { "0" }.to_string()),
                        ("fp", fingerprint.clone()),
                        ("p", printers.len().to_string()),
                    ];
                    let host_name = format!("peerprint-{}.local.", &shared.identity.host_id.to_string()[..8]);
                    match ServiceInfo::new(
                        SERVICE_TYPE,
                        &instance,
                        &host_name,
                        std::net::IpAddr::V4(ip),
                        cfg.host.listen_port,
                        properties.as_slice(),                    ) {
                        Ok(info) => {
                            let fullname = info.get_fullname().to_string();
                            match daemon.register(info) {
                                Ok(()) => {
                                    registered = Some((fullname, signature));
                                    tracing::info!("advertising {SERVICE_TYPE} on {ip}:{}", cfg.host.listen_port);
                                }
                                Err(err) => shared.notice(
                                    crate::service::NoticeLevel::Error,
                                    format!("mDNS advertise failed: {err}"),
                                ),
                            }
                        }
                        Err(err) => shared.notice(
                            crate::service::NoticeLevel::Error,
                            format!("mDNS service definition rejected: {err}"),
                        ),
                    }
                }
            } else if let Some((fullname, _)) = registered.take() {
                let _ = daemon.unregister(&fullname);
            }

            tokio::select! {
                _ = tokio::time::sleep(ADVERT_INTERVAL) => {}
                _ = shutdown.changed() => {}
            }
        }

        if let Some((fullname, _)) = registered.take() {
            let _ = daemon.unregister(&fullname);
        }
        let _ = daemon.shutdown();
    });
}

pub fn spawn_browser(shared: Arc<Shared>) {
    let runtime = shared.runtime.clone();
    runtime.spawn(async move {
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => {
                tracing::warn!("mDNS browse unavailable: {err}");
                return;
            }
        };
        let receiver = match daemon.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(err) => {
                tracing::warn!("mDNS browse failed: {err}");
                return;
            }
        };

        let mut names: HashMap<String, Uuid> = HashMap::new();
        let mut refresh = tokio::time::interval(REFRESH_INTERVAL);
        let mut shutdown = shared.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                event = receiver.recv_async() => {
                    match event {
                        Ok(ServiceEvent::ServiceResolved(info)) => {
                            let Some(advert) = parse_txt(&info.get_properties()) else {
                                continue;
                            };
                            if advert.host_id == shared.identity.host_id {
                                continue;
                            }
                            let Some(ip) = preferred_addr(info.get_addresses()) else {
                                continue;
                            };
                            names.insert(info.get_fullname().to_string(), advert.host_id);
                            let addr = SocketAddr::new(ip, info.get_port());
                            let display_name = advert.display_name.clone();
                            shared.upsert_discovered(DiscoveredHost {
                                host_id: advert.host_id,
                                instance: info.get_fullname().to_string(),
                                addr: addr.to_string(),
                                display_name,
                                pin_required: advert.pin_required,
                                tls_fp: advert.tls_fp,
                                printer_count: advert.printer_count,
                                printers: Vec::new(),
                                last_seen: now_unix(),
                            });
                            let shared = shared.clone();
                            tokio::spawn(async move {
                                enrich(&shared, addr).await;
                            });
                        }
                        Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                            if let Some(host_id) = names.remove(&fullname) {
                                shared.remove_discovered(&fullname);
                                tracing::debug!("host {host_id} disappeared");
                            }
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                _ = refresh.tick() => {
                    let targets: Vec<SocketAddr> = shared
                        .core()
                        .discovered
                        .iter()
                        .filter_map(|host| host.addr.parse().ok())
                        .collect();
                    for addr in targets {
                        let shared = shared.clone();
                        tokio::spawn(async move {
                            enrich(&shared, addr).await;
                        });
                    }
                }
                _ = shutdown.changed() => break,
            }
        }

        let _ = daemon.shutdown();
    });
}

async fn enrich(shared: &Arc<Shared>, addr: SocketAddr) {
    match crate::client::fetch_info(addr, None).await {
        Ok(info) => {
            let mut core = shared.core.lock().expect("core lock");
            if let Some(host) = core
                .discovered
                .iter_mut()
                .find(|host| host.host_id == info.host_id)
            {
                host.display_name = info.display_name;
                host.printers = info.printers;
                host.printer_count = host.printers.len() as u32;
                host.pin_required = info.pin_required;
                host.tls_fp = hex::encode(info.tls_fp);
                host.last_seen = now_unix();
            }
        }
        Err(err) => tracing::debug!("info fetch from {addr} failed: {err:#}"),
    }
}

pub async fn probe_manual(shared: &Arc<Shared>, addr: SocketAddr) -> Result<DiscoveredHost> {
    let info = crate::client::fetch_info(addr, None).await?;
    if info.host_id == shared.identity.host_id {
        bail!("that address is this PC");
    }
    let host = DiscoveredHost {
        host_id: info.host_id,
        instance: format!("manual:{addr}"),
        addr: addr.to_string(),
        display_name: info.display_name,
        pin_required: info.pin_required,
        tls_fp: hex::encode(info.tls_fp),
        printer_count: info.printers.len() as u32,
        printers: info.printers,
        last_seen: now_unix(),
    };
    shared.remove_discovered(&format!("manual:{addr}"));
    shared.upsert_discovered(host.clone());
    Ok(host)
}
