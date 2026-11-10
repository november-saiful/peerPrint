use std::mem::size_of;

use anyhow::{bail, Context, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::core::PCWSTR;

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn quote_argument(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if !value.contains([' ', '\t', '"']) {
        return value.to_string();
    }
    let escaped = value.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Relaunches this executable with `runas`, waits for it, and returns the exit code.
pub fn run_elevated(arguments: &str) -> Result<u32> {
    let exe = std::env::current_exe().context("resolving executable path")?;
    let exe_w = wide(&exe.to_string_lossy());
    let verb_w = wide("runas");
    let args_w = wide(arguments);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb_w.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: 0,
        ..Default::default()
    };

    unsafe {
        if let Err(err) = ShellExecuteExW(&mut info) {
            if err.code().0 as u32 == 1223 {
                bail!("Administrator permission was declined");
            }
            return Err(anyhow::anyhow!("starting elevated helper: {err}"));
        }
        if info.hProcess.is_invalid() {
            bail!("elevated helper did not start");
        }
        WaitForSingleObject(info.hProcess, u32::MAX);
        let mut code = 0u32;
        GetExitCodeProcess(info.hProcess, &mut code).context("reading elevated exit code")?;
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}

#[allow(dead_code)]
pub fn process_is_alive(handle: HANDLE) -> bool {
    !handle.is_invalid()
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut std::ffi::c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}
