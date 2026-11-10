use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::{data_dir, write_json};
use crate::win::dpapi;

#[derive(Clone)]
pub struct HostIdentity {
    pub host_id: Uuid,
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    pub fingerprint: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    host_id: Uuid,
    cert_der_b64: String,
    key_der_dpapi: String,
}

fn identity_path() -> std::path::PathBuf {
    data_dir().join("identity").join("host.json")
}

pub fn load_or_create() -> Result<HostIdentity> {
    let path = identity_path();
    if path.exists() {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let stored: StoredIdentity = serde_json::from_slice(&bytes)?;
        let key_der = dpapi::unprotect_b64(&stored.key_der_dpapi)?;
        let cert_der = STANDARD
            .decode(&stored.cert_der_b64)
            .context("decoding host certificate")?;
        let fingerprint = sha256(&cert_der);
        return Ok(HostIdentity {
            host_id: stored.host_id,
            cert_der,
            key_der,
            fingerprint,
        });
    }

    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "peerprint".to_string());
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["peerprint.local".to_string(), hostname])?;
    let cert_der = cert.der().to_vec();
    let key_der = signing_key.serialize_der();
    let identity = HostIdentity {
        host_id: Uuid::new_v4(),
        fingerprint: sha256(&cert_der),
        cert_der,
        key_der,
    };
    let stored = StoredIdentity {
        host_id: identity.host_id,
        cert_der_b64: STANDARD.encode(&identity.cert_der),
        key_der_dpapi: dpapi::protect_b64(&identity.key_der)?,
    };
    write_json(&path, &stored)?;
    Ok(identity)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}
