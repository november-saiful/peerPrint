use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Keeps a named mutex alive for the lifetime of the process.
pub struct InstanceGuard {
    handle: Option<HANDLE>,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}

/// Returns `None` when another process already owns the mutex.
pub fn acquire(name: &str) -> Option<InstanceGuard> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) };
    match handle {
        Ok(handle) => {
            // GetLastError must be read right after the call that sets it.
            let already = unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS;
            if already {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                None
            } else {
                Some(InstanceGuard { handle: Some(handle) })
            }
        }
        Err(err) => {
            // Without a mutex we cannot detect a second instance; let this one run.
            tracing::warn!("single-instance mutex unavailable: {err}");
            Some(InstanceGuard { handle: None })
        }
    }
}
