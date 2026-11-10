use anyhow::{Context, Result};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "PeerPrint";

pub fn is_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .is_ok()
}

/// True when the Run entry exists and points at this executable.
pub fn is_current() -> bool {
    let Ok(expected) = command() else {
        return false;
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
        .map(|value| value == expected)
        .unwrap_or(false)
}

fn command() -> Result<String> {
    let exe = std::env::current_exe().context("resolving executable path")?;
    Ok(format!("\"{}\" --autostart", exe.display()))
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey_with_flags(RUN_KEY, KEY_WRITE)
        .context("opening HKCU Run key")?;
    if enabled {
        key.set_value(VALUE_NAME, &command()?)?;
    } else {
        let _ = key.delete_value(VALUE_NAME);
    }
    Ok(())
}
