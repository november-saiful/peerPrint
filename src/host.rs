use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use rustls::ServerConfig;
use uuid::Uuid;

use crate::config;
use crate::net::framing::{read_msg, write_msg, MsgReader};
use crate::net::protocol::{ClientMsg, ErrCode, HostMsg, PROTO_VERSION};
use crate::net::tls::{self, Conn};
use crate::pairing::{self, PairError};
use crate::service::{JobDirection, JobRecord, NoticeLevel, Shared};
use crate::win::spooler::{self, RawJob};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const JOB_DATA_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_JOB_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const OFFLINE_GRACE: Duration = Duration::from_secs(120);
const PRINT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

pub struct HostCtx {
    pub shared: Arc<Shared>,
    pub tls_config: Arc<ServerConfig>,
    pub inflight: Mutex<HashSet<Uuid>>,
    pub connections: AtomicU32,
}

pub fn spawn_host(shared: Arc<Shared>) {
    let runtime = shared.runtime.clone();
    runtime.spawn(async move {
        let tls_config = match tls::server_config(&shared.identity) {
            Ok(config) => config,
            Err(err) => {
                shared.set_host_state(false, Some(format!("TLS setup failed: {err:#}")));
                shared.notice(NoticeLevel::Error, format!("TLS setup failed: {err:#}"));
                return;
            }
        };
        let ctx = Arc::new(HostCtx {
            shared: shared.clone(),
            tls_config,
            inflight: Mutex::new(HashSet::new()),
            connections: AtomicU32::new(0),
        });
        let limit = Arc::new(Semaphore::new(32));

        cleanup_stale_jobs(shared.clone());

        let mut listener: Option<(TcpListener, u16)> = None;
        let mut shutdown = shared.shutdown_tx.subscribe();
        loop {
            let cfg = shared.config();
            let wanted = cfg.host.enabled && !cfg.host.shared.is_empty();

            match &listener {
                Some((_, port)) if !wanted || *port != cfg.host.listen_port => {
                    listener = None;
                    shared.set_host_state(false, None);
                }
                None if wanted => {
                    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.host.listen_port));
                    match TcpListener::bind(addr).await {
                        Ok(bound) => {
                            tracing::info!("host listening on {addr}");
                            shared.set_host_state(true, None);
                            listener = Some((bound, cfg.host.listen_port));
                        }
                        Err(err) => {
                            let message = format!("cannot listen on {addr}: {err}");
                            tracing::warn!("{message}");
                            shared.set_host_state(false, Some(message.clone()));
                            shared.notice(NoticeLevel::Error, message);
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                                _ = shutdown.changed() => break,
                            }
                        }
                    }
                }
                _ => {}
            }

            if let Some((listener, _)) = &listener {
                tokio::select! {
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((tcp, peer)) => {
                                let Ok(permit) = limit.clone().try_acquire_owned() else {
                                    tracing::warn!("rejecting {peer}: too many connections");
                                    continue;
                                };
                                ctx.connections.fetch_add(1, Ordering::Relaxed);
                                let ctx = ctx.clone();
                                tokio::spawn(async move {
                                    handle_connection(ctx.clone(), tcp, peer, permit).await;
                                    ctx.connections.fetch_sub(1, Ordering::Relaxed);
                                });
                            }
                            Err(err) => {
                                tracing::warn!("accept failed: {err}");
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = shutdown.changed() => break,
                }
            } else {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = shutdown.changed() => break,
                }
            }
        }
        shared.set_host_state(false, None);
    });
}

fn cleanup_stale_jobs(shared: Arc<Shared>) {
    let printers: Vec<String> = shared
        .config()
        .host
        .shared
        .iter()
        .map(|printer| printer.spooler_name.clone())
        .collect();
    if printers.is_empty() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        for printer in printers {
            let script = format!(
                "$ErrorActionPreference='SilentlyContinue'; Get-PrintJob -PrinterName {name} | Where-Object {{ $_.DocumentName -like 'PeerPrint:*' }} | Remove-PrintJob",
                name = crate::win::powershell::ps_quote(&printer)
            );
            let _ = crate::win::powershell::run(&script, Duration::from_secs(30));
        }
    });
}

async fn handle_connection(
    ctx: Arc<HostCtx>,
    tcp: TcpStream,
    peer: SocketAddr,
    permit: OwnedSemaphorePermit,
) {
    let _permit = permit;
    let result = tokio::time::timeout(
        Duration::from_secs(60 * 60),
        handle_session(ctx.clone(), tcp, peer),
    )
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::debug!("session with {peer} ended: {err:#}"),
        Err(_) => tracing::debug!("session with {peer} exceeded maximum duration"),
    }
}

async fn handle_session(ctx: Arc<HostCtx>, tcp: TcpStream, peer: SocketAddr) -> Result<()> {
    let shared = ctx.shared.clone();
    let mut stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, tls::accept(tcp, ctx.tls_config.clone()))
        .await
        .context("TLS handshake timed out")??;

    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_msg::<_, ClientMsg>(&mut stream))
        .await
        .context("client handshake timed out")??;
    let (client_proto, app_version) = match hello {
        ClientMsg::Hello { proto, app_version } => (proto, app_version),
        other => bail!("expected Hello, got {other:?}"),
    };
    if client_proto != PROTO_VERSION {
        write_msg(
            &mut stream,
            &HostMsg::Error {
                code: ErrCode::Protocol,
                message: format!("host uses protocol {PROTO_VERSION}"),
                fatal: true,
            },
        )
        .await?;
        return Ok(());
    }
    tracing::debug!("client {peer} ({app_version}) connected");

    let cfg = shared.config();
    write_msg(
        &mut stream,
        &HostMsg::HelloAck {
            proto: PROTO_VERSION,
            host_id: shared.identity.host_id,
            display_name: cfg.host.display_name.clone(),
            pin_required: cfg.host.require_pin && cfg.host.pin_dpapi.is_some(),
            tls_fp: shared.identity.fingerprint,
        },
    )
    .await?;

    let mut client: Option<Uuid> = None;
    loop {
        let message = match tokio::time::timeout(IDLE_TIMEOUT, read_msg::<_, ClientMsg>(&mut stream)).await {
            Ok(Ok(message)) => message,
            Ok(Err(err)) => {
                tracing::debug!("client {peer} disconnected: {err:#}");
                return Ok(());
            }
            Err(_) => {
                tracing::debug!("client {peer} idle timeout");
                return Ok(());
            }
        };

        match message {
            ClientMsg::Info => {
                let printers = shared.host_printers_meta();
                write_msg(
                    &mut stream,
                    &HostMsg::InfoReply {
                        host_id: shared.identity.host_id,
                        display_name: shared.config().host.display_name,
                        printers,
                    },
                )
                .await?;
            }
            ClientMsg::Ping { nonce } => {
                write_msg(&mut stream, &HostMsg::Pong { nonce }).await?;
            }
            ClientMsg::PairInit { spake_a, client_name } => {
                if client.is_some() {
                    write_msg(
                        &mut stream,
                        &HostMsg::Error {
                            code: ErrCode::Protocol,
                            message: "already authenticated".to_string(),
                            fatal: true,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                handle_pairing(shared.clone(), &mut stream, peer, spake_a, client_name).await?;
                return Ok(());
            }
            ClientMsg::Auth { client_id, token } => {
                let (valid, revoked) = {
                    let clients = shared.clients.lock().expect("clients lock");
                    (
                        clients.verify(client_id, &token),
                        clients
                            .clients
                            .iter()
                            .any(|entry| entry.id == client_id && entry.revoked),
                    )
                };
                if !valid {
                    let (code, message) = if revoked {
                        (ErrCode::Revoked, "this PC was removed by the host")
                    } else {
                        (ErrCode::Unauthorized, "unknown or invalid access token")
                    };
                    write_msg(
                        &mut stream,
                        &HostMsg::Error {
                            code,
                            message: message.to_string(),
                            fatal: true,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                {
                    let mut clients = shared.clients.lock().expect("clients lock");
                    clients.touch(client_id);
                    let _ = config::save_paired_clients(&clients);
                }
                client = Some(client_id);
                write_msg(
                    &mut stream,
                    &HostMsg::AuthOk {
                        display_name: shared.config().host.display_name,
                        printers: shared.host_printers_meta(),
                    },
                )
                .await?;
            }
            ClientMsg::Status => {
                if client.is_none() {
                    write_msg(
                        &mut stream,
                        &HostMsg::Error {
                            code: ErrCode::Unauthorized,
                            message: "authenticate first".to_string(),
                            fatal: true,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                write_msg(
                    &mut stream,
                    &HostMsg::StatusReply {
                        printers: shared.host_printer_statuses(),
                    },
                )
                .await?;
            }
            ClientMsg::JobStart {
                tx,
                printer_id,
                client_job_ref,
                doc_name,
                len,
                sha256,
            } => {
                if client.is_none() {
                    write_msg(
                        &mut stream,
                        &HostMsg::Error {
                            code: ErrCode::Unauthorized,
                            message: "authenticate first".to_string(),
                            fatal: true,
                        },
                    )
                    .await?;
                    return Ok(());
                }
                handle_job(
                    ctx.clone(),
                    &mut stream,
                    peer,
                    printer_id,
                    client_job_ref,
                    doc_name,
                    len,
                    sha256,
                    tx,
                )
                .await?;
                return Ok(());
            }
            ClientMsg::JobData { .. }
            | ClientMsg::JobEnd { .. }
            | ClientMsg::JobCancel { .. }
            | ClientMsg::PairFinish { .. } => {
                write_msg(
                    &mut stream,
                    &HostMsg::Error {
                        code: ErrCode::Protocol,
                        message: "no job in progress".to_string(),
                        fatal: true,
                    },
                )
                .await?;
                return Ok(());
            }
            ClientMsg::Hello { .. } => {
                write_msg(
                    &mut stream,
                    &HostMsg::Error {
                        code: ErrCode::Protocol,
                        message: "duplicate handshake".to_string(),
                        fatal: true,
                    },
                )
                .await?;
                return Ok(());
            }
        }
    }
}

async fn handle_pairing(
    shared: Arc<Shared>,
    stream: &mut Conn,
    peer: SocketAddr,
    spake_a: Vec<u8>,
    client_name: String,
) -> Result<()> {
    if let Some(retry) = shared.pin_limiter.retry_after() {
        write_msg(
            stream,
            &HostMsg::Error {
                code: ErrCode::LockedOut,
                message: format!("too many incorrect PIN attempts; retry in {retry}s"),
                fatal: true,
            },
        )
        .await?;
        return Ok(());
    }

    let cfg = shared.config();
    let pin_required = cfg.host.require_pin && cfg.host.pin_dpapi.is_some();
    let pin = if pin_required {
        match cfg.host_pin() {
            Some(pin) => pin,
            None => {
                write_msg(
                    stream,
                    &HostMsg::Error {
                        code: ErrCode::Internal,
                        message: "the host PIN could not be read".to_string(),
                        fatal: true,
                    },
                )
                .await?;
                return Ok(());
            }
        }
    } else {
        String::new()
    };

    let limiter = &shared.pin_limiter;
    let result =
        pairing::pair_host_from(stream, spake_a, client_name.clone(), &pin, || limiter.record_failure())
            .await;

    match result {
        Ok(pair) => {
            shared.pin_limiter.record_success();
            let name = sanitize_client_name(&pair.client_name, peer);
            let token_hash = hex::encode(config::hash_token(pair.client_id, &pair.token));
            {
                let mut clients = shared.clients.lock().expect("clients lock");
                clients.add(pair.client_id, name.clone(), token_hash);
                if let Err(err) = config::save_paired_clients(&clients) {
                    tracing::error!("saving paired clients: {err:#}");
                }
            }
            write_msg(
                stream,
                &HostMsg::PairOk {
                    confirm_b: pair.confirm_b,
                    client_id: pair.client_id,
                    host_id: shared.identity.host_id,
                    token: pair.token,
                    display_name: shared.config().host.display_name,
                    printers: shared.host_printers_meta(),
                },
            )
            .await?;
            tracing::info!("paired with {name} from {peer}");
            shared.notice(NoticeLevel::Info, format!("Paired with {name}"));
        }
        Err(PairError::BadPin) => {
            tracing::warn!("pairing from {peer} failed: incorrect PIN");
        }
        Err(PairError::LockedOut) => {}
        Err(PairError::Other(err)) => {
            tracing::warn!("pairing from {peer} failed: {err:#}");
            let _ = write_msg(
                stream,
                &HostMsg::Error {
                    code: ErrCode::Internal,
                    message: "pairing failed".to_string(),
                    fatal: true,
                },
            )
            .await;
        }
    }
    Ok(())
}

fn sanitize_client_name(raw: &str, peer: SocketAddr) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).take(64).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        peer.ip().to_string()
    } else {
        cleaned.to_string()
    }
}

fn incoming_dir() -> PathBuf {
    config::data_dir().join("state").join("incoming")
}

#[allow(clippy::too_many_arguments)]
async fn handle_job(
    ctx: Arc<HostCtx>,
    stream: &mut Conn,
    peer: SocketAddr,
    printer_id: Uuid,
    client_job_ref: Uuid,
    doc_name: String,
    len: u64,
    sha256: [u8; 32],
    tx: u32,
) -> Result<()> {
    let shared = ctx.shared.clone();
    let Some(printer) = shared.shared_printer_by_id(printer_id) else {
        write_failed(stream, tx, ErrCode::UnknownPrinter, "printer is not shared", false).await?;
        return Ok(());
    };
    let doc_name: String = doc_name
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();

    if let Some(recent) = shared.recent_host_job(client_job_ref) {
        write_msg(stream, &HostMsg::JobAccepted { tx, job_id: 0 }).await?;
        match recent.failure {
            None => {
                write_msg(
                    stream,
                    &HostMsg::JobDone {
                        tx,
                        pages: recent.pages,
                    },
                )
                .await?;
            }
            Some(message) => {
                write_failed(stream, tx, ErrCode::Internal, &message, false).await?;
            }
        }
        return Ok(());
    }
    let already_inflight = {
        let mut inflight = ctx.inflight.lock().expect("inflight lock");
        !inflight.insert(client_job_ref)
    };
    if already_inflight {
        write_failed(stream, tx, ErrCode::Busy, "job is already being transferred", true).await?;
        return Ok(());
    }

    let outcome = receive_and_print(
        shared.clone(),
        stream,
        peer,
        printer.spooler_name.clone(),
        printer.display_name.clone(),
        client_job_ref,
        doc_name.clone(),
        len,
        sha256,
        tx,
    )
    .await;

    ctx.inflight.lock().expect("inflight lock").remove(&client_job_ref);

    if let Err(err) = outcome {
        let message = format!("{err:#}");
        tracing::warn!("job {client_job_ref} from {peer} failed: {message}");
        let existing = shared.job_state(client_job_ref);
        if let Some(state) = existing {
            if state != "Cancelled" {
                shared.update_job(client_job_ref, |job| {
                    job.state = "Failed".to_string();
                    job.error = Some(message);
                    job.finished = Some(config::now_unix());
                });
            }
        } else {
            let mut job = JobRecord::new(
                client_job_ref,
                JobDirection::Host,
                printer.display_name.clone(),
                peer.to_string(),
                doc_name,
            );
            job.state = "Failed".to_string();
            job.error = Some(message);
            job.finished = Some(config::now_unix());
            shared.record_job(job, true);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn receive_and_print(
    shared: Arc<Shared>,
    stream: &mut Conn,
    peer: SocketAddr,
    spooler_name: String,
    printer_display: String,
    client_job_ref: Uuid,
    doc_name: String,
    len: u64,
    sha256: [u8; 32],
    tx: u32,
) -> Result<()> {
    if len == 0 {
        write_failed(stream, tx, ErrCode::Protocol, "empty job", false).await?;
        bail!("client sent an empty job");
    }
    if len > MAX_JOB_BYTES {
        write_failed(stream, tx, ErrCode::Protocol, "job is too large", false).await?;
        bail!("job of {len} bytes exceeds the limit");
    }

    write_msg(stream, &HostMsg::JobAccepted { tx, job_id: 0 }).await?;

    let mut job = JobRecord::new(
        client_job_ref,
        JobDirection::Host,
        printer_display.clone(),
        peer.to_string(),
        doc_name.clone(),
    );
    job.bytes = len;
    job.state = "Receiving".to_string();
    shared.record_job(job, true);

    let dir = incoming_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{client_job_ref}.bin"));
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(file) => file,
        Err(err) => {
            write_failed(
                stream,
                tx,
                ErrCode::Internal,
                &format!("cannot buffer the job: {err}"),
                true,
            )
            .await?;
            bail!("creating incoming file: {err}");
        }
    };
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut cancelled_by_client = false;

    let receive = async {
        loop {
            let message = match tokio::time::timeout(JOB_DATA_TIMEOUT, read_msg::<_, ClientMsg>(stream)).await {
                Ok(Ok(message)) => message,
                Ok(Err(err)) => bail!("connection lost during transfer: {err:#}"),
                Err(_) => bail!("client stopped sending data"),
            };
            match message {
                ClientMsg::JobData { tx: chunk_tx, data } => {
                    if chunk_tx != tx {
                        bail!("data for the wrong job");
                    }
                    received += data.len() as u64;
                    if received > len {
                        bail!("client sent more data than declared");
                    }
                    hasher.update(&data);
                    file.write_all(&data).await.context("buffering job")?;
                }
                ClientMsg::JobEnd { tx: end_tx } if end_tx == tx => break,
                ClientMsg::JobCancel { tx: cancel_tx, reason } if cancel_tx == tx => {
                    cancelled_by_client = true;
                    bail!("client cancelled the job: {reason}");
                }
                ClientMsg::Ping { nonce } => {
                    write_msg(stream, &HostMsg::Pong { nonce }).await?;
                }
                other => bail!("unexpected message during transfer: {other:?}"),
            }
        }
        Ok::<(), anyhow::Error>(())
    };
    let receive_result = receive.await;
    let _ = file.flush().await;
    drop(file);

    if let Err(err) = receive_result {
        let _ = tokio::fs::remove_file(&path).await;
        if cancelled_by_client {
            shared.mark_cancelled(client_job_ref, "cancelled by the client");
        }
        write_failed(stream, tx, ErrCode::Cancelled, &format!("{err:#}"), false).await?;
        return Err(err);
    }

    let digest: [u8; 32] = hasher.finalize().into();
    if received != len || digest != sha256 {
        let _ = tokio::fs::remove_file(&path).await;
        write_failed(
            stream,
            tx,
            ErrCode::Protocol,
            "job integrity check failed",
            true,
        )
        .await?;
        bail!("integrity mismatch (received {received} of {len} bytes)");
    }

    if shared.cancel_requested(client_job_ref) {
        shared.clear_cancel(client_job_ref);
        let _ = tokio::fs::remove_file(&path).await;
        shared.mark_cancelled(client_job_ref, "cancelled on this PC");
        write_failed(stream, tx, ErrCode::Cancelled, "cancelled on this PC", false).await?;
        return Ok(());
    }

    shared.update_job(client_job_ref, |job| {
        job.state = "Spooling".to_string();
    });

    let spool_path = path.clone();
    let spool_printer = spooler_name.clone();
    let spool_doc = doc_name.clone();
    let started = tokio::task::spawn_blocking(move || -> Result<u32> {
        let mut source = std::fs::File::open(&spool_path).context("opening buffered job")?;
        let mut raw = RawJob::start(&spool_printer, &spool_doc)?;
        let mut buffer = vec![0u8; 64 * 1024];
        use std::io::Read;
        loop {
            let n = source.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            raw.write_chunk(&buffer[..n])?;
        }
        raw.finish()
    })
    .await
    .context("spooling task panicked")?;

    let job_id = match started {
        Ok(job_id) => job_id,
        Err(err) => {
            let _ = tokio::fs::remove_file(&path).await;
            write_failed(stream, tx, ErrCode::Internal, &format!("{err:#}"), true).await?;
            bail!("{err:#}");
        }
    };

    shared.update_job(client_job_ref, |job| {
        job.state = "Printing".to_string();
    });
    let (read_half, mut write_half) = tokio::io::split(&mut *stream);
    let mut reader = MsgReader::new(read_half);
    let _ = write_msg(
        &mut write_half,
        &HostMsg::JobProgress {
            tx,
            pages_printed: 0,
            total_pages: 0,
        },
    )
    .await;

    let printed = track_spool_job(
        shared.clone(),
        &mut reader,
        &mut write_half,
        tx,
        &spooler_name,
        job_id,
        client_job_ref,
    )
    .await;

    let _ = tokio::fs::remove_file(&path).await;
    let (pages, failure, cancelled_reason) = match printed {
        Ok(TrackOutcome::Printed(pages)) => (pages, None, None),
        Ok(TrackOutcome::Cancelled { pages, by_client }) => {
            let reason = if by_client {
                "cancelled by the client"
            } else {
                "cancelled on this PC"
            };
            (pages, Some(reason.to_string()), Some(reason))
        }
        Err(err) => (0, Some(format!("{err:#}")), None),
    };
    shared.remember_host_job(client_job_ref, pages, failure.clone());
    shared.update_job(client_job_ref, |job| {
        job.pages = pages;
        job.finished = Some(config::now_unix());
        if let Some(reason) = cancelled_reason {
            job.state = "Cancelled".to_string();
            job.error = Some(reason.to_string());
        } else {
            match &failure {
                Some(error) => {
                    job.state = "Failed".to_string();
                    job.error = Some(error.clone());
                }
                None => {
                    job.state = "Printed".to_string();
                    job.error = None;
                }
            }
        }
    });
    if let Some(job) = shared
        .core()
        .jobs
        .into_iter()
        .find(|job| job.id == client_job_ref)
    {
        crate::service::append_job(&job);
    }

    // The outcome is already recorded; a client that left before the reply
    // must not turn a printed job into a failed one.
    match (cancelled_reason, failure) {
        (Some(reason), _) => {
            let _ = write_failed(&mut write_half, tx, ErrCode::Cancelled, reason, false).await;
        }
        (None, None) => {
            let _ = write_msg(&mut write_half, &HostMsg::JobDone { tx, pages }).await;
        }
        (None, Some(error)) => {
            let _ = write_failed(&mut write_half, tx, ErrCode::Internal, &error, true).await;
        }
    }
    Ok(())
}

enum TrackOutcome {
    Printed(u32),
    Cancelled { pages: u32, by_client: bool },
}

async fn track_spool_job<R, W>(
    shared: Arc<Shared>,
    reader: &mut MsgReader<R>,
    writer: &mut W,
    tx: u32,
    printer: &str,
    job_id: u32,
    client_job_ref: Uuid,
) -> Result<TrackOutcome>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let started = Instant::now();
    let mut last_pages = 0u32;
    let mut last_sent = Instant::now();
    let mut offline_since: Option<Instant> = None;
    let mut client_connected = true;

    loop {
        if client_connected {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                message = reader.read::<ClientMsg>() => match message {
                    Ok(ClientMsg::JobCancel { tx: cancel_tx, reason }) if cancel_tx == tx => {
                        shared.clear_cancel(client_job_ref);
                        let _ = spooler::cancel_job(printer, job_id);
                        tracing::info!("client cancelled printing job {client_job_ref}: {reason}");
                        return Ok(TrackOutcome::Cancelled {
                            pages: last_pages,
                            by_client: true,
                        });
                    }
                    Ok(ClientMsg::Ping { nonce }) => {
                        let _ = write_msg(writer, &HostMsg::Pong { nonce }).await;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        client_connected = false;
                        tracing::debug!("client left while job {client_job_ref} printed");
                    }
                },
            }
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if *shared.shutdown_tx.borrow() {
            let _ = spooler::cancel_job(printer, job_id);
            bail!("host is shutting down");
        }
        if shared.cancel_requested(client_job_ref) {
            shared.clear_cancel(client_job_ref);
            let _ = spooler::cancel_job(printer, job_id);
            return Ok(TrackOutcome::Cancelled {
                pages: last_pages,
                by_client: false,
            });
        }
        if started.elapsed() > PRINT_TIMEOUT {
            let _ = spooler::cancel_job(printer, job_id);
            bail!("job did not finish in time");
        }

        let snapshot = match spooler::job_status(printer, job_id) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                tracing::warn!("job status failed: {err:#}");
                None
            }
        };

        let Some(snapshot) = snapshot else {
            shared.update_job(client_job_ref, |job| {
                job.pages = last_pages.max(1);
            });
            return Ok(TrackOutcome::Printed(last_pages.max(1)));
        };

        if snapshot.total_pages > 0 {
            shared.update_job(client_job_ref, |job| {
                job.pages = snapshot.pages_printed;
            });
        }
        if snapshot.pages_printed > last_pages || snapshot.total_pages > 0 {
            last_pages = snapshot.pages_printed.max(last_pages);
            if last_sent.elapsed() > Duration::from_millis(900) {
                let _ = write_msg(
                    writer,
                    &HostMsg::JobProgress {
                        tx,
                        pages_printed: snapshot.pages_printed,
                        total_pages: snapshot.total_pages,
                    },
                )
                .await;
                last_sent = Instant::now();
            }
        }

        if spooler::job_printed(snapshot.status) {
            return Ok(TrackOutcome::Printed(
                snapshot.pages_printed.max(snapshot.total_pages).max(last_pages),
            ));
        }
        if spooler::job_deleted(snapshot.status) {
            bail!("the print job was deleted in the Windows queue");
        }
        if spooler::job_error(snapshot.status) {
            bail!("the Windows spooler reported an error");
        }

        let offline = match spooler::printer_status(printer) {
            Ok((status, _)) => spooler::printer_offline(status),
            Err(_) => true,
        };
        if offline {
            let since = *offline_since.get_or_insert_with(Instant::now);
            if since.elapsed() > OFFLINE_GRACE {
                let _ = spooler::cancel_job(printer, job_id);
                bail!("printer is offline");
            }
        } else {
            offline_since = None;
        }
    }
}

async fn write_failed<W>(
    stream: &mut W,
    tx: u32,
    code: ErrCode,
    message: &str,
    retryable: bool,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    write_msg(
        stream,
        &HostMsg::JobFailed {
            tx,
            code,
            message: message.to_string(),
            retryable,
        },
    )
    .await
}

#[allow(dead_code)]
pub fn host_profile(shared: &Shared) -> serde_json::Value {
    serde_json::json!({
        "host_id": shared.identity.host_id,
        "display_name": shared.config().host.display_name,
        "listen_port": shared.config().host.listen_port,
        "printers": shared.host_printers_meta(),
    })
}
