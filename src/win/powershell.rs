use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct PowerShellOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl PowerShellOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }

    pub fn combined(&self) -> String {
        let mut text = self.stdout.trim().to_string();
        if !self.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(self.stderr.trim());
        }
        text
    }
}

pub fn ps_quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            escaped.push('\'');
        }
        escaped.push(ch);
    }
    escaped.push('\'');
    escaped
}

fn encode_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

pub fn run(script: &str, timeout: Duration) -> Result<PowerShellOutput> {
    let encoded = encode_command(&format!("$ProgressPreference='SilentlyContinue'; {script}"));
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("starting powershell.exe")?;

    let started = Instant::now();
    loop {
        if let Some(_status) = child.try_wait()? {
            break;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let output = child.wait_with_output()?;
            bail!(
                "PowerShell timed out after {}s: {}",
                timeout.as_secs(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let output = child.wait_with_output()?;
    Ok(PowerShellOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn run_default(script: &str) -> Result<PowerShellOutput> {
    run(script, Duration::from_secs(120))
}
