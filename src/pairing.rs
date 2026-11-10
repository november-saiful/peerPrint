use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use tokio::net::TcpStream;
use tokio_rustls::TlsStream;
use uuid::Uuid;

use crate::net::framing::{read_msg, write_msg};
use crate::net::protocol::{ClientMsg, ErrCode, HostMsg, PrinterMeta};
use crate::net::tls;

type HmacSha256 = Hmac<Sha256>;

const ID_CLIENT: &[u8] = b"peerprint-client";
const ID_HOST: &[u8] = b"peerprint-host";
const PAIR_LABEL: &[u8] = b"peerprint-pair-v1";

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("incorrect PIN")]
    BadPin,
    #[error("too many incorrect PIN attempts")]
    LockedOut,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug)]
pub struct PairClientResult {
    pub host_id: Uuid,
    pub host_display: String,
    pub printers: Vec<PrinterMeta>,
    pub token: [u8; 32],
    pub tls_fp: [u8; 32],
    pub client_id: Uuid,
}

pub struct PairHostResult {
    pub client_id: Uuid,
    pub client_name: String,
    pub token: [u8; 32],
    pub confirm_b: [u8; 32],
}

pub fn validate_pin(pin: &str) -> Result<()> {
    if !(4..=8).contains(&pin.len()) || !pin.chars().all(|c| c.is_ascii_digit()) {
        bail!("PIN must be 4 to 8 digits");
    }
    Ok(())
}

fn confirmation(
    shared: &[u8],
    exporter: &[u8; 32],
    spake_a: &[u8],
    spake_b: &[u8],
    role: &[u8],
) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(shared).expect("HMAC accepts any key length");
    mac.update(PAIR_LABEL);
    mac.update(exporter);
    mac.update(spake_a);
    mac.update(spake_b);
    mac.update(role);
    mac.finalize().into_bytes().into()
}

fn tokens_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.ct_eq(b).into()
}

pub async fn pair_client(
    stream: &mut TlsStream<TcpStream>,
    pin: &str,
    client_name: &str,
) -> Result<PairClientResult> {
    let identity_client = Identity::new(ID_CLIENT);
    let identity_host = Identity::new(ID_HOST);
    let (state, spake_a) =
        Spake2::<Ed25519Group>::start_a(&Password::new(pin.as_bytes()), &identity_client, &identity_host);

    write_msg(
        stream,
        &ClientMsg::PairInit {
            spake_a: spake_a.clone(),
            client_name: client_name.to_string(),
        },
    )
    .await?;

    let spake_b = match read_msg::<_, HostMsg>(stream).await? {
        HostMsg::PairChallenge { spake_b } => spake_b,
        HostMsg::Error { message, .. } => bail!("host rejected pairing: {message}"),
        other => bail!("unexpected pairing message: {other:?}"),
    };

    let shared = state
        .finish(&spake_b)
        .map_err(|_| anyhow::anyhow!("SPAKE2 exchange failed"))?;
    let exporter = tls::exporter(stream)?;
    let confirm_a = confirmation(&shared, &exporter, &spake_a, &spake_b, b"client");
    let expected_confirm_b = confirmation(&shared, &exporter, &spake_a, &spake_b, b"host");
    let client_id = Uuid::new_v4();

    write_msg(
        stream,
        &ClientMsg::PairFinish {
            confirm_a,
            client_id,
        },
    )
    .await?;

    match read_msg::<_, HostMsg>(stream).await? {
        HostMsg::PairOk {
            confirm_b,
            client_id: echoed_id,
            host_id,
            token,
            display_name,
            printers,
        } => {
            if !tokens_equal(&confirm_b, &expected_confirm_b) {
                bail!("host could not prove knowledge of the PIN");
            }
            if echoed_id != client_id {
                bail!("host echoed the wrong client id");
            }
            let tls_fp = tls::peer_fingerprint(stream)
                .context("reading host certificate fingerprint")?;
            Ok(PairClientResult {
                host_id,
                host_display: display_name,
                printers,
                token,
                tls_fp,
                client_id,
            })
        }
        HostMsg::Error { code, message, .. } => {
            let detail = match code {
                ErrCode::BadPin => "Incorrect PIN".to_string(),
                ErrCode::LockedOut => "Too many incorrect PIN attempts".to_string(),
                _ => message,
            };
            bail!(detail)
        }
        other => bail!("unexpected pairing response: {other:?}"),
    }
}

pub async fn pair_host(
    stream: &mut TlsStream<TcpStream>,
    pin: &str,
    fail: impl FnOnce(),
) -> Result<PairHostResult, PairError> {
    let init = match read_msg::<_, ClientMsg>(stream)
        .await
        .map_err(PairError::Other)?
    {
        ClientMsg::PairInit { spake_a, client_name } => (spake_a, client_name),
        other => {
            return Err(PairError::Other(anyhow::anyhow!(
                "unexpected message before pairing: {other:?}"
            )))
        }
    };
    let (spake_a, client_name) = init;
    pair_host_from(stream, spake_a, client_name, pin, fail).await
}

/// Continues pairing after the caller has already consumed `PairInit`.
pub async fn pair_host_from(
    stream: &mut TlsStream<TcpStream>,
    spake_a: Vec<u8>,
    client_name: String,
    pin: &str,
    fail: impl FnOnce(),
) -> Result<PairHostResult, PairError> {
    let identity_client = Identity::new(ID_CLIENT);
    let identity_host = Identity::new(ID_HOST);
    let (state, spake_b) =
        Spake2::<Ed25519Group>::start_b(&Password::new(pin.as_bytes()), &identity_client, &identity_host);

    write_msg(stream, &HostMsg::PairChallenge { spake_b: spake_b.clone() })
        .await
        .map_err(PairError::Other)?;

    let finish = match read_msg::<_, ClientMsg>(stream)
        .await
        .map_err(PairError::Other)?
    {
        ClientMsg::PairFinish { confirm_a, client_id } => (confirm_a, client_id),
        other => {
            return Err(PairError::Other(anyhow::anyhow!(
                "unexpected message during pairing: {other:?}"
            )))
        }
    };
    let (confirm_a, client_id) = finish;

    let shared = state
        .finish(&spake_a)
        .map_err(|_| PairError::Other(anyhow::anyhow!("SPAKE2 exchange failed")))?;
    let exporter = tls::exporter(stream).map_err(PairError::Other)?;
    let expected_a = confirmation(&shared, &exporter, &spake_a, &spake_b, b"client");
    if !tokens_equal(&confirm_a, &expected_a) {
        fail();
        let _ = write_msg(
            stream,
            &HostMsg::Error {
                code: ErrCode::BadPin,
                message: "Incorrect PIN".to_string(),
                fatal: true,
            },
        )
        .await;
        return Err(PairError::BadPin);
    }

    let mut token = [0u8; 32];
    getrandom::fill(&mut token).map_err(|e| PairError::Other(anyhow::anyhow!("generating token: {e}")))?;
    let confirm_b = confirmation(&shared, &exporter, &spake_a, &spake_b, b"host");

    Ok(PairHostResult {
        client_id,
        client_name,
        token,
        confirm_b,
    })
}

pub struct PinLimiter {
    inner: Mutex<LimiterState>,
}

struct LimiterState {
    failures: u32,
    locked_until: Option<Instant>,
    last_failure: Option<Instant>,
}

impl Default for PinLimiter {
    fn default() -> Self {
        Self {
            inner: Mutex::new(LimiterState {
                failures: 0,
                locked_until: None,
                last_failure: None,
            }),
        }
    }
}

impl PinLimiter {
    pub fn retry_after(&self) -> Option<u64> {
        let state = self.inner.lock().ok()?;
        let until = state.locked_until?;
        let now = Instant::now();
        if until > now {
            Some((until - now).as_secs().max(1))
        } else {
            None
        }
    }

    pub fn record_failure(&self) {
        if let Ok(mut state) = self.inner.lock() {
            let now = Instant::now();
            if let Some(last) = state.last_failure {
                if now.duration_since(last) > Duration::from_secs(15 * 60) {
                    state.failures = 0;
                }
            }
            state.failures = state.failures.saturating_add(1);
            state.last_failure = Some(now);
            let lock_secs = match state.failures {
                0..=4 => 0,
                5..=9 => 60,
                10..=14 => 300,
                15..=19 => 900,
                _ => 3600,
            };
            if lock_secs > 0 {
                state.locked_until = Some(now + Duration::from_secs(lock_secs));
            }
        }
    }

    pub fn record_success(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.failures = 0;
            state.locked_until = None;
            state.last_failure = None;
        }
    }

    pub fn failures(&self) -> u32 {
        self.inner.lock().map(|s| s.failures).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::HostIdentity;
    use crate::net::protocol::PROTO_VERSION;
    use std::net::SocketAddr;

    fn test_identity() -> HostIdentity {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["peerprint.local".to_string()]).unwrap();
        let cert_der = cert.der().to_vec();
        let key_der = signing_key.serialize_der();
        HostIdentity {
            host_id: Uuid::new_v4(),
            fingerprint: crate::net::tls::sha256(&cert_der),
            cert_der,
            key_der,
        }
    }

    async fn spawn_test_hosts(pin: &'static str) -> (SocketAddr, HostIdentity) {
        let identity = test_identity();
        let config = crate::net::tls::server_config(&identity).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_identity = identity.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let config = config.clone();
                let identity = host_identity.clone();
                tokio::spawn(async move {
                    let mut stream = match crate::net::tls::accept(tcp, config).await {
                        Ok(stream) => stream,
                        Err(_) => return,
                    };
                    let _hello: ClientMsg = read_msg(&mut stream).await.unwrap();
                    write_msg(
                        &mut stream,
                        &HostMsg::HelloAck {
                            proto: PROTO_VERSION,
                            host_id: identity.host_id,
                            display_name: "Test host".to_string(),
                            pin_required: true,
                            tls_fp: identity.fingerprint,
                        },
                    )
                    .await
                    .unwrap();
                    match pair_host(&mut stream, pin, || {}).await {
                        Ok(result) => {
                            write_msg(
                                &mut stream,
                                &HostMsg::PairOk {
                                    confirm_b: result.confirm_b,
                                    client_id: result.client_id,
                                    host_id: identity.host_id,
                                    token: result.token,
                                    display_name: "Test host".to_string(),
                                    printers: Vec::new(),
                                },
                            )
                            .await
                            .unwrap();
                        }
                        Err(_) => {}
                    }
                });
            }
        });
        (addr, identity)
    }

    #[tokio::test]
    async fn pairs_with_correct_pin_and_rejects_wrong_pin() {
        let (addr, identity) = spawn_test_hosts("482913").await;

        let mut stream = crate::net::tls::connect(addr, crate::net::tls::client_config(None))
            .await
            .unwrap();
        write_msg(
            &mut stream,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
                app_version: "test".to_string(),
            },
        )
        .await
        .unwrap();
        let ack: HostMsg = read_msg(&mut stream).await.unwrap();
        assert!(matches!(ack, HostMsg::HelloAck { .. }));
        let result = pair_client(&mut stream, "482913", "Laptop").await.unwrap();
        assert_eq!(result.host_id, identity.host_id);
        assert_eq!(result.tls_fp, identity.fingerprint);
        assert_ne!(result.token, [0u8; 32]);

        let mut stream = crate::net::tls::connect(addr, crate::net::tls::client_config(None))
            .await
            .unwrap();
        write_msg(
            &mut stream,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
                app_version: "test".to_string(),
            },
        )
        .await
        .unwrap();
        let _ack: HostMsg = read_msg(&mut stream).await.unwrap();
        let error = pair_client(&mut stream, "000000", "Laptop").await.unwrap_err();
        assert!(error.to_string().to_lowercase().contains("pin"));
    }

    #[tokio::test]
    async fn pinned_client_rejects_a_different_certificate() {
        let identity = test_identity();
        let config = crate::net::tls::server_config(&identity).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let config = config.clone();
                tokio::spawn(async move {
                    let _ = crate::net::tls::accept(tcp, config).await;
                });
            }
        });

        let good = crate::net::tls::connect(
            addr,
            crate::net::tls::client_config(Some(identity.fingerprint)),
        )
        .await;
        assert!(good.is_ok(), "matching fingerprint must connect");

        let wrong = crate::net::tls::connect(
            addr,
            crate::net::tls::client_config(Some([0xAB; 32])),
        )
        .await;
        assert!(wrong.is_err(), "wrong fingerprint must abort the handshake");
    }

    #[test]
    fn pin_limiter_escalates_and_resets() {
        let limiter = PinLimiter::default();
        for _ in 0..4 {
            limiter.record_failure();
        }
        assert!(limiter.retry_after().is_none());
        limiter.record_failure();
        assert!(limiter.retry_after().is_some());
        for _ in 0..5 {
            limiter.record_failure();
        }
        assert!(limiter.retry_after().unwrap() >= 60);
        limiter.record_success();
        assert!(limiter.retry_after().is_none());
        assert_eq!(limiter.failures(), 0);
    }
}
