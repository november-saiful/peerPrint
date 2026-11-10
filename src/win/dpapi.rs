use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};

static MACHINE_SCOPE: AtomicBool = AtomicBool::new(false);

/// Machine-scoped secrets can be read by any process on this PC (the host
/// service and whichever user is signed in), instead of one user profile.
pub fn set_machine_scope(enabled: bool) {
    MACHINE_SCOPE.store(enabled, Ordering::SeqCst);
}

fn flags() -> u32 {
    let machine = if MACHINE_SCOPE.load(Ordering::SeqCst) {
        CRYPTPROTECT_LOCAL_MACHINE
    } else {
        0
    };
    CRYPTPROTECT_UI_FORBIDDEN | machine
}

pub fn protect_bytes(plain: &[u8]) -> Result<Vec<u8>> {
    if plain.is_empty() {
        return Ok(Vec::new());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            flags(),
            &mut output,
        )
        .context("CryptProtectData failed")?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(bytes)
    }
}

pub fn unprotect_bytes(cipher: &[u8]) -> Result<Vec<u8>> {
    if cipher.is_empty() {
        return Ok(Vec::new());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            flags(),
            &mut output,
        )
        .context("CryptUnprotectData failed")?;
        if output.pbData.is_null() {
            bail!("CryptUnprotectData returned no data");
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(bytes)
    }
}

pub fn protect_string(plain: &str) -> Result<String> {
    Ok(STANDARD.encode(protect_bytes(plain.as_bytes())?))
}

pub fn unprotect_string(cipher: &str) -> Result<String> {
    let bytes = unprotect_bytes(&STANDARD.decode(cipher)?)?;
    String::from_utf8(bytes).context("secret is not valid UTF-8")
}

pub fn protect_b64(plain: &[u8]) -> Result<String> {
    Ok(STANDARD.encode(protect_bytes(plain)?))
}

pub fn unprotect_b64(cipher: &str) -> Result<Vec<u8>> {
    unprotect_bytes(&STANDARD.decode(cipher)?)
}
