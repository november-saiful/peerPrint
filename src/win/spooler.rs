use std::ptr;

use anyhow::{Context, Result};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Graphics::Printing::{
    AbortPrinter, ClosePrinter, EnumPrintersW, EndDocPrinter, GetJobW, GetPrinterDriverW, GetPrinterW,
    OpenPrinterW, SetJobW, SetPrinterW, StartDocPrinterW, WritePrinter, DOC_INFO_1W, DRIVER_INFO_6W,
    JOB_CONTROL_CANCEL, JOB_INFO_1W, JOB_STATUS_COMPLETE, JOB_STATUS_DELETED, JOB_STATUS_ERROR,
    JOB_STATUS_PRINTED, PRINTER_ACCESS_USE,
    PRINTER_ALL_ACCESS, PRINTER_ATTRIBUTE_ENABLE_BIDI, PRINTER_DEFAULTSW, PRINTER_ENUM_CONNECTIONS,
    PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_2W, PRINTER_STATUS_BUSY,
    PRINTER_STATUS_DOOR_OPEN, PRINTER_STATUS_ERROR, PRINTER_STATUS_NO_TONER, PRINTER_STATUS_OFFLINE,
    PRINTER_STATUS_OUTPUT_BIN_FULL, PRINTER_STATUS_OUT_OF_MEMORY, PRINTER_STATUS_PAPER_JAM,
    PRINTER_STATUS_PAPER_OUT, PRINTER_STATUS_PAUSED, PRINTER_STATUS_PRINTING,
    PRINTER_STATUS_TONER_LOW, PRINTER_STATUS_USER_INTERVENTION, PRINTER_STATUS_WARMING_UP,
    PRINTER_ACCESS_RIGHTS, PRINTER_READ,
};

#[derive(Clone, Debug)]
pub struct LocalPrinter {
    pub name: String,
    pub port_name: String,
    pub driver_name: String,
    pub driver_env: String,
    pub driver_version: u64,
    pub hardware_id: String,
    pub status: u32,
    pub jobs: u32,
}

#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub status: u32,
    pub total_pages: u32,
    pub pages_printed: u32,
}

pub struct PrinterHandle(pub PRINTER_HANDLE);

impl Drop for PrinterHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = ClosePrinter(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(context: &str) -> anyhow::Error {
    let err = windows::core::Error::from_thread();
    anyhow::anyhow!("{context}: {err}")
}

pub fn open_printer(name: &str) -> Result<PrinterHandle> {
    open_printer_with(name, PRINTER_ACCESS_USE)
}

pub fn open_printer_with(name: &str, access: PRINTER_ACCESS_RIGHTS) -> Result<PrinterHandle> {
    let name_w = wide(name);
    let defaults = PRINTER_DEFAULTSW {
        DesiredAccess: access,
        ..Default::default()
    };
    let mut handle = PRINTER_HANDLE::default();
    unsafe {
        OpenPrinterW(
            PCWSTR(name_w.as_ptr()),
            &mut handle,
            Some(&defaults as *const PRINTER_DEFAULTSW),
        )
        .map_err(|e| anyhow::anyhow!("opening printer '{name}': {e}"))?;
    }
    Ok(PrinterHandle(handle))
}

fn read_printer_info2(handle: &PrinterHandle) -> Result<(PRINTER_INFO_2W, Vec<u8>)> {
    let mut needed = 0u32;
    let _ = unsafe { GetPrinterW(handle.0, 2, None, &mut needed) };
    if needed == 0 {
        return Err(last_error("GetPrinterW(size)"));
    }
    let mut buffer = vec![0u8; needed as usize];
    unsafe {
        GetPrinterW(handle.0, 2, Some(&mut buffer), &mut needed)
            .map_err(|e| anyhow::anyhow!("GetPrinterW(level 2): {e}"))?;
        let info = ptr::read_unaligned(buffer.as_ptr() as *const PRINTER_INFO_2W);
        Ok((info, buffer))
    }
}

fn read_driver6(handle: &PrinterHandle) -> Result<DRIVER_INFO_6W> {
    let mut needed = 0u32;
    let _ = unsafe { GetPrinterDriverW(handle.0, PCWSTR::null(), 6, None, &mut needed) };
    if needed == 0 {
        return Err(last_error("GetPrinterDriverW(size)"));
    }
    let mut buffer = vec![0u8; needed as usize];
    unsafe {
        let ok = GetPrinterDriverW(handle.0, PCWSTR::null(), 6, Some(&mut buffer), &mut needed);
        if !ok.as_bool() {
            return Err(last_error("GetPrinterDriverW(level 6)"));
        }
        Ok(ptr::read_unaligned(buffer.as_ptr() as *const DRIVER_INFO_6W))
    }
}

fn pwstr_to_string(value: PWSTR) -> String {
    if value.0.is_null() {
        String::new()
    } else {
        unsafe { value.to_string().unwrap_or_default() }
    }
}

pub fn list_printers() -> Result<Vec<LocalPrinter>> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let mut needed = 0u32;
    let mut returned = 0u32;
    let _ = unsafe {
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            2,
            None,
            &mut needed,
            &mut returned,
        )
    };
    if needed == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = vec![0u8; needed as usize];
    unsafe {
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            2,
            Some(&mut buffer),
            &mut needed,
            &mut returned,
        )
        .context("EnumPrintersW")?;
    }

    let mut printers = Vec::with_capacity(returned as usize);
    for index in 0..returned as usize {
        let info = unsafe {
            ptr::read_unaligned((buffer.as_ptr() as *const PRINTER_INFO_2W).add(index))
        };
        let name = pwstr_to_string(info.pPrinterName);
        if name.is_empty() {
            continue;
        }
        let port_name = pwstr_to_string(info.pPortName);
        let driver_name = pwstr_to_string(info.pDriverName);

        let (driver_env, driver_version, hardware_id) = match open_printer(&name) {
            Ok(handle) => match read_driver6(&handle) {
                Ok(driver) => (
                    pwstr_to_string(driver.pEnvironment),
                    driver.dwlDriverVersion,
                    pwstr_to_string(driver.pszHardwareID),
                ),
                Err(_) => (String::new(), 0, String::new()),
            },
            Err(_) => (String::new(), 0, String::new()),
        };

        printers.push(LocalPrinter {
            name,
            port_name,
            driver_name,
            driver_env,
            driver_version,
            hardware_id,
            status: info.Status,
            jobs: info.cJobs,
        });
    }
    Ok(printers)
}

pub fn is_shareable(printer: &LocalPrinter) -> bool {
    let port = printer.port_name.to_ascii_uppercase();
    let is_local_bus = port.starts_with("USB") || port.starts_with("DOT4");
    let is_x64 = printer.driver_env.eq_ignore_ascii_case("Windows x64");
    let virtual_markers = [
        "microsoft print to pdf",
        "send to microsoft onenote",
        "microsoft xps document writer",
        "nitro pdf",
        "adobe pdf",
    ];
    let lower_driver = printer.driver_name.to_ascii_lowercase();
    let is_virtual = virtual_markers
        .iter()
        .any(|marker| lower_driver.contains(marker));
    is_local_bus && is_x64 && !is_virtual
}

pub struct RawJob {
    handle: PrinterHandle,
    job_id: u32,
    open: bool,
}

impl RawJob {
    pub fn start(printer: &str, doc_name: &str) -> Result<Self> {
        let handle = open_printer(printer)?;
        let mut doc_name = wide(&format!("PeerPrint:{doc_name}"));
        let mut datatype = wide("RAW");
        let info = DOC_INFO_1W {
            pDocName: PWSTR(doc_name.as_mut_ptr()),
            pOutputFile: PWSTR::null(),
            pDatatype: PWSTR(datatype.as_mut_ptr()),
        };
        let job_id = unsafe { StartDocPrinterW(handle.0, 1, &info) };
        if job_id == 0 {
            return Err(last_error("StartDocPrinterW"));
        }
        Ok(Self {
            handle,
            job_id,
            open: true,
        })
    }

    pub fn write_chunk(&mut self, data: &[u8]) -> Result<()> {
        for chunk in data.chunks(64 * 1024) {
            let mut written = 0u32;
            let ok = unsafe {
                WritePrinter(
                    self.handle.0,
                    chunk.as_ptr() as *const _,
                    chunk.len() as u32,
                    &mut written,
                )
            };
            if !ok.as_bool() || written as usize != chunk.len() {
                return Err(last_error("WritePrinter"));
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<u32> {
        let ok = unsafe { EndDocPrinter(self.handle.0) };
        if !ok.as_bool() {
            return Err(last_error("EndDocPrinter"));
        }
        self.open = false;
        Ok(self.job_id)
    }
}

impl Drop for RawJob {
    fn drop(&mut self) {
        if self.open {
            unsafe {
                let _ = AbortPrinter(self.handle.0);
            }
        }
    }
}

pub fn print_raw(printer: &str, data: &[u8], doc_name: &str) -> Result<u32> {
    let mut job = RawJob::start(printer, doc_name)?;
    job.write_chunk(data)?;
    job.finish()
}

pub fn job_status(printer: &str, job_id: u32) -> Result<Option<JobSnapshot>> {
    let handle = open_printer(printer)?;
    let mut needed = 0u32;
    let ok = unsafe { GetJobW(handle.0, job_id, 1, None, &mut needed) };
    if !ok.as_bool() || needed == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; needed as usize];
    let ok = unsafe { GetJobW(handle.0, job_id, 1, Some(&mut buffer), &mut needed) };
    if !ok.as_bool() {
        return Ok(None);
    }
    let info = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const JOB_INFO_1W) };
    Ok(Some(JobSnapshot {
        status: info.Status,
        total_pages: info.TotalPages,
        pages_printed: info.PagesPrinted,
    }))
}

pub fn cancel_job(printer: &str, job_id: u32) -> Result<()> {
    let handle = open_printer(printer)?;
    let ok = unsafe { SetJobW(handle.0, job_id, 0, None, JOB_CONTROL_CANCEL) };
    if !ok.as_bool() {
        return Err(last_error("SetJobW(cancel)"));
    }
    Ok(())
}

pub fn printer_status(printer: &str) -> Result<(u32, u32)> {
    let handle = open_printer(printer)?;
    let (info, _buffer) = read_printer_info2(&handle)?;
    Ok((info.Status, info.cJobs))
}

pub fn clear_bidi(printer: &str) -> Result<()> {
    let handle = open_printer_with(printer, PRINTER_ALL_ACCESS)?;
    let (mut info, _buffer) = read_printer_info2(&handle)?;
    if info.Attributes & PRINTER_ATTRIBUTE_ENABLE_BIDI == 0 {
        return Ok(());
    }
    info.Attributes &= !PRINTER_ATTRIBUTE_ENABLE_BIDI;
    unsafe {
        SetPrinterW(handle.0, 2, Some(&info as *const _ as *const u8), 0)
            .map_err(|e| anyhow::anyhow!("SetPrinterW(disable bidi): {e}"))?;
    }
    Ok(())
}

/// The print processor the installed v3 driver declares and Windows has registered.
///
/// peerPrint queues must use it: vendor processors carry behaviour that `winprint`
/// does not reproduce, such as Canon's manual-duplex flip prompt.
pub fn driver_print_processor(driver: &str) -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let driver_key = format!(
        r"SYSTEM\CurrentControlSet\Control\Print\Environments\Windows x64\Drivers\Version-3\{driver}"
    );
    let processor: String = hklm
        .open_subkey(driver_key)
        .ok()?
        .get_value("Print Processor")
        .ok()?;
    let processor = processor.trim().to_string();
    if processor.is_empty() {
        return None;
    }
    let registered = hklm
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Print\Environments\Windows x64\Print Processors")
        .ok()?
        .enum_keys()
        .filter_map(Result::ok)
        .any(|name| name.eq_ignore_ascii_case(&processor));
    registered.then_some(processor)
}

pub fn queue_print_processor(queue: &str) -> Option<String> {
    let handle = open_printer_with(queue, PRINTER_READ).ok()?;
    let (info, _buffer) = read_printer_info2(&handle).ok()?;
    let processor = pwstr_to_string(info.pPrintProcessor);
    (!processor.is_empty()).then_some(processor)
}

pub fn queue_needs_print_processor(queue: &str, driver: &str) -> bool {
    match (driver_print_processor(driver), queue_print_processor(queue)) {
        (Some(expected), Some(actual)) => !actual.eq_ignore_ascii_case(&expected),
        _ => false,
    }
}

pub fn set_print_processor(queue: &str, processor: &str) -> Result<()> {
    let handle = open_printer_with(queue, PRINTER_ALL_ACCESS)?;
    let (mut info, _buffer) = read_printer_info2(&handle)?;
    if pwstr_to_string(info.pPrintProcessor).eq_ignore_ascii_case(processor) {
        return Ok(());
    }
    let mut processor_w = wide(processor);
    info.pPrintProcessor = PWSTR(processor_w.as_mut_ptr());
    unsafe {
        SetPrinterW(handle.0, 2, Some(&info as *const _ as *const u8), 0)
            .map_err(|e| anyhow::anyhow!("SetPrinterW(print processor): {e}"))?;
    }
    Ok(())
}

pub fn status_text(status: u32) -> String {
    if status == 0 {
        return "Ready".to_string();
    }
    let mut parts = Vec::new();
    let flags = [
        (PRINTER_STATUS_OFFLINE, "Offline"),
        (PRINTER_STATUS_PAPER_OUT, "Paper out"),
        (PRINTER_STATUS_PAPER_JAM, "Paper jam"),
        (PRINTER_STATUS_DOOR_OPEN, "Cover open"),
        (PRINTER_STATUS_NO_TONER, "No toner"),
        (PRINTER_STATUS_TONER_LOW, "Toner low"),
        (PRINTER_STATUS_OUTPUT_BIN_FULL, "Output bin full"),
        (PRINTER_STATUS_OUT_OF_MEMORY, "Out of memory"),
        (PRINTER_STATUS_PAUSED, "Paused"),
        (PRINTER_STATUS_ERROR, "Error"),
        (PRINTER_STATUS_USER_INTERVENTION, "Needs attention"),
        (PRINTER_STATUS_BUSY, "Busy"),
        (PRINTER_STATUS_PRINTING, "Printing"),
        (PRINTER_STATUS_WARMING_UP, "Warming up"),
    ];
    for (flag, label) in flags {
        if status & flag != 0 {
            parts.push(label);
        }
    }
    if parts.is_empty() {
        format!("Status 0x{status:08X}")
    } else {
        parts.join(", ")
    }
}

pub fn job_printed(status: u32) -> bool {
    status & (JOB_STATUS_PRINTED | JOB_STATUS_COMPLETE) != 0
}

pub fn job_deleted(status: u32) -> bool {
    status & JOB_STATUS_DELETED != 0
}

pub fn job_error(status: u32) -> bool {
    status & JOB_STATUS_ERROR != 0
}

pub fn printer_offline(status: u32) -> bool {
    status
        & (PRINTER_STATUS_OFFLINE
            | PRINTER_STATUS_PAPER_OUT
            | PRINTER_STATUS_ERROR
            | PRINTER_STATUS_USER_INTERVENTION)
        != 0
}

#[allow(dead_code)]
pub fn open_all_access(printer: &str) -> Result<PrinterHandle> {
    open_printer_with(printer, PRINTER_ALL_ACCESS)
}

/// Windows 11 24H2+ can block third-party v3 drivers via Windows protected print mode.
pub fn protected_print_mode() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    hklm.open_subkey(r"SOFTWARE\Policies\Microsoft\Windows NT\Printers\WPP")
        .and_then(|key| key.get_value::<u32, _>("WindowsProtectedPrintMode"))
        .map(|value| value == 1)
        .unwrap_or(false)
}
