//! Windows service host: runs the engine as LocalSystem at boot, before any
//! user signs in. The tray application talks to it over the engine pipe.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_SERVICE_DOES_NOT_EXIST, HANDLE};
use windows::Win32::System::Services::*;

use crate::win::elevation;

pub const SERVICE_NAME: &str = "peerPrint";
const DISPLAY_NAME: &str = "peerPrint printer sharing";

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn report(state: SERVICE_STATUS_CURRENT_STATE, controls: u32, checkpoint: u32, hint: u32) {
    let handle = STATUS_HANDLE.load(Ordering::SeqCst);
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: checkpoint,
        dwWaitHint: hint,
    };
    unsafe {
        let _ = SetServiceStatus(SERVICE_STATUS_HANDLE(handle), &status);
    }
}

unsafe extern "system" fn handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            if !STOP_REQUESTED.swap(true, Ordering::SeqCst) {
                report(SERVICE_STOP_PENDING, 0, 1, 10_000);
            }
        }
        SERVICE_CONTROL_INTERROGATE => {
            report(
                SERVICE_RUNNING,
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
                0,
                0,
            );
        }
        _ => {}
    }
    0
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows::core::PWSTR) {
    let name = wide(SERVICE_NAME);
    let handle = match RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler), None) {
        Ok(handle) => handle,
        Err(_) => return,
    };
    STATUS_HANDLE.store(handle.0, Ordering::SeqCst);
    report(SERVICE_START_PENDING, 0, 1, 15_000);

    let result = run_engine();
    let code = match &result {
        Ok(()) => 0,
        Err(err) => {
            tracing::error!("peerPrint service stopped: {err:#}");
            1
        }
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_STOPPED,
        dwControlsAccepted: 0,
        dwWin32ExitCode: if code == 0 {
            0
        } else {
            windows::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR.0
        },
        dwServiceSpecificExitCode: code,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    unsafe {
        let _ = SetServiceStatus(handle, &status);
    }
}

fn run_engine() -> Result<()> {
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("peerprint-service")
        .build()
        .context("starting the service runtime")?;

    let shared = {
        let _guard = runtime.enter();
        crate::service::Shared::new(runtime.handle().clone()).context("loading peerPrint state")?
    };

    runtime.block_on(crate::service::start_services(shared.clone(), false, true));
    crate::client::resume_all_queues(&shared);
    report(
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
        0,
        0,
    );
    tracing::info!("peerPrint service is running");

    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }

    tracing::info!("peerPrint service is stopping");
    crate::service::shutdown(&shared);
    crate::client::pause_all_queues(&shared);
    runtime.shutdown_timeout(Duration::from_secs(3));
    Ok(())
}

/// Blocks until the service manager stops the service.
pub fn run() -> Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: windows::core::PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe {
        StartServiceCtrlDispatcherW(table.as_ptr()).context(
            "peerPrint must be started by the Windows service manager (install it with --install-service)",
        )?;
    }
    Ok(())
}

/// Creates or updates the boot-start service and starts it. Requires elevation.
pub fn install() -> Result<()> {
    if !elevation::is_elevated() {
        bail!("installing the peerPrint service needs administrator permission");
    }
    let exe = std::env::current_exe().context("resolving executable path")?;
    let command = format!("\"{}\" --service", exe.display());
    let command_w = wide(&command);
    let display_w = wide(DISPLAY_NAME);
    let name_w = wide(SERVICE_NAME);
    let dependencies = wide("Spooler");
    let mut dependencies: Vec<u16> = dependencies
        .into_iter()
        .chain(std::iter::once(0))
        .collect();

    let manager = unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CREATE_SERVICE | SC_MANAGER_CONNECT)
            .context("opening the service manager")?
    };
    let service = unsafe {
        match OpenServiceW(manager, PCWSTR(name_w.as_ptr()), SERVICE_ALL_ACCESS) {
            Ok(service) => {
                ChangeServiceConfigW(
                    service,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(command_w.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR(dependencies.as_mut_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(display_w.as_ptr()),
                )
                .context("updating the peerPrint service")?;
                tracing::info!("updated the existing peerPrint service");
                (service, true)
            }
            Err(err) if err.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => (
                CreateServiceW(
                    manager,
                    PCWSTR(name_w.as_ptr()),
                    PCWSTR(display_w.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(command_w.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR(dependencies.as_mut_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
                .context("creating the peerPrint service")?,
                false,
            ),
            Err(err) => return Err(err).context("opening the peerPrint service"),
        }
    };
    let (service, existed) = service;

    let description = wide("Shares USB printers with other PCs on this network, starting before anyone signs in.");
    let mut description_info = SERVICE_DESCRIPTIONW {
        lpDescription: windows::core::PWSTR(description.as_ptr() as *mut u16),
    };
    unsafe {
        let _ = ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            Some(&mut description_info as *mut _ as *mut c_void),
        );
    }
    // An existing service still runs the old binary; restart it so upgrades
    // take effect immediately instead of at the next boot.
    if existed {
        stop_service(service);
    }
    unsafe {
        let _ = CloseHandle(HANDLE(manager.0));
        let _ = CloseHandle(HANDLE(service.0));
    }

    prepare_data_dir()?;

    match start() {
        Ok(()) => println!("peerPrint service installed and started."),
        Err(err) => println!("peerPrint service installed; it will start automatically at boot ({err:#})."),
    }
    Ok(())
}

/// Stops and deletes the service. Requires elevation.
pub fn uninstall() -> Result<()> {
    if !elevation::is_elevated() {
        bail!("removing the peerPrint service needs administrator permission");
    }
    let name_w = wide(SERVICE_NAME);
    let manager = unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
            .context("opening the service manager")?
    };
    let service = unsafe { OpenServiceW(manager, PCWSTR(name_w.as_ptr()), SERVICE_ALL_ACCESS) };
    let service = match service {
        Ok(service) => service,
        Err(err) if err.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => {
            println!("peerPrint service is not installed.");
            unsafe {
                let _ = CloseHandle(HANDLE(manager.0));
            }
            return Ok(());
        }
        Err(err) => {
            unsafe {
                let _ = CloseHandle(HANDLE(manager.0));
            }
            return Err(err).context("opening the peerPrint service");
        }
    };

    unsafe {
        stop_service(service);
        let deleted = DeleteService(service).is_ok();
        let _ = CloseHandle(HANDLE(service.0));
        let _ = CloseHandle(HANDLE(manager.0));
        if !deleted {
            bail!("Windows did not remove the peerPrint service");
        }
    }
    println!("peerPrint service removed.");
    Ok(())
}

/// Asks the service to stop and waits up to ten seconds for STOPPED.
fn stop_service(service: windows::Win32::System::Services::SC_HANDLE) {
    unsafe {
        let mut status = SERVICE_STATUS::default();
        let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);
        for _ in 0..100 {
            let mut status = SERVICE_STATUS::default();
            if QueryServiceStatus(service, &mut status).is_ok()
                && status.dwCurrentState == SERVICE_STOPPED
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn start() -> Result<()> {
    let name_w = wide(SERVICE_NAME);
    let manager = unsafe {
        OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
            .context("opening the service manager")?
    };
    let service = unsafe { OpenServiceW(manager, PCWSTR(name_w.as_ptr()), SERVICE_START) };
    let service = match service {
        Ok(service) => service,
        Err(err) => {
            unsafe {
                let _ = CloseHandle(HANDLE(manager.0));
            }
            return Err(err).context("opening the peerPrint service");
        }
    };
    let result = unsafe { StartServiceW(service, None) };
    unsafe {
        let _ = CloseHandle(HANDLE(service.0));
        let _ = CloseHandle(HANDLE(manager.0));
    }
    result.context("starting the peerPrint service")
}

/// ProgramData is readable by every local user by default; machine-scoped
/// secrets (client tokens, the host PIN) must not be.
fn prepare_data_dir() -> Result<()> {
    let dir = crate::config::data_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let status = std::process::Command::new("icacls.exe")
        .arg(&dir)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg("*S-1-5-18:(OI)(CI)F")
        .arg("*S-1-5-32-544:(OI)(CI)F")
        .status()
        .context("running icacls.exe")?;
    if !status.success() {
        bail!("could not protect {}", dir.display());
    }
    Ok(())
}

#[allow(dead_code)]
pub fn is_installed() -> bool {
    let name_w = wide(SERVICE_NAME);
    unsafe {
        let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
            return false;
        };
        let service = OpenServiceW(manager, PCWSTR(name_w.as_ptr()), SERVICE_QUERY_STATUS);
        let installed = service.is_ok();
        if let Ok(service) = service {
            let _ = CloseHandle(HANDLE(service.0));
        }
        let _ = CloseHandle(HANDLE(manager.0));
        installed
    }
}
