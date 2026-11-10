use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::identity::HostIdentity;
use crate::net::protocol::ALPN;

pub type Conn = TlsStream<TcpStream>;

pub fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn server_config(identity: &HostIdentity) -> Result<Arc<ServerConfig>> {
    ensure_crypto();
    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .context("building TLS server config")?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(config))
}

pub fn client_config(pin: Option<[u8; 32]>) -> Arc<ClientConfig> {
    ensure_crypto();
    let verifier = Arc::new(PinnedVerifier { expected: pin });
    let builder = ClientConfig::builder();
    let config = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let mut config = config;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Arc::new(config)
}

pub async fn connect(
    addr: std::net::SocketAddr,
    config: Arc<ClientConfig>,
) -> Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connecting to {addr}"))?;
    tcp.set_nodelay(true).ok();
    let server_name = ServerName::try_from("peerprint.local".to_string())
        .map_err(|_| anyhow::anyhow!("invalid TLS server name"))?;
    let stream = TlsConnector::from(config)
        .connect(server_name, tcp)
        .await
        .context("TLS handshake")?;
    Ok(TlsStream::Client(stream))
}

pub async fn accept(
    tcp: TcpStream,
    config: Arc<ServerConfig>,
) -> Result<TlsStream<TcpStream>> {
    tcp.set_nodelay(true).ok();
    let stream = TlsAcceptor::from(config)
        .accept(tcp)
        .await
        .context("TLS handshake")?;
    Ok(TlsStream::Server(stream))
}

pub fn peer_fingerprint(stream: &TlsStream<TcpStream>) -> Option<[u8; 32]> {
    let (_, connection) = stream.get_ref();
    let cert = connection.peer_certificates()?.first()?;
    Some(sha256(cert.as_ref()))
}

pub fn exporter(stream: &Conn) -> Result<[u8; 32]> {
    let output = [0u8; 32];
    let result = match stream {
        TlsStream::Client(inner) => {
            let (_, connection) = inner.get_ref();
            connection
                .export_keying_material(output, b"peerprint-pair", None)
                .map(|bytes| bytes.to_vec())
        }
        TlsStream::Server(inner) => {
            let (_, connection) = inner.get_ref();
            connection
                .export_keying_material(output, b"peerprint-pair", None)
                .map(|bytes| bytes.to_vec())
        }
    };
    let bytes = result.context("TLS keying material export")?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

#[derive(Debug)]
struct PinnedVerifier {
    /// `None` accepts any certificate (pairing only, protected by SPAKE2).
    expected: Option<[u8; 32]>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if let Some(expected) = self.expected {
            let actual = sha256(end_entity.as_ref());
            if actual != expected {
                return Err(TlsError::General(
                    "host certificate fingerprint does not match the saved printer".to_string(),
                ));
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
