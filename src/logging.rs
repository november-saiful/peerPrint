use std::io::{self, Write};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;

use crate::config;

pub struct LogGuard(#[allow(dead_code)] WorkerGuard);

struct RedactingWriter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=position).collect();
            let text = String::from_utf8_lossy(&line[..line.len().saturating_sub(1)]);
            writeln!(self.inner, "{}", redact_line(&text))?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let text = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            write!(self.inner, "{}", redact_line(&text))?;
        }
        self.inner.flush()
    }
}

const SENSITIVE_KEYS: [&str; 7] = ["pin", "token", "password", "secret", "key", "authorization", "credential"];

pub fn redact_line(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut redact_next = false;
    for chunk in line.split_inclusive(char::is_whitespace) {
        let trimmed = chunk.trim_end();
        let suffix = &chunk[trimmed.len()..];
        if redact_next {
            result.push_str("«redacted»");
            result.push_str(suffix);
            redact_next = false;
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            result.push_str("Bearer «redacted»");
            result.push_str(suffix);
            continue;
        }
        let sensitive = SENSITIVE_KEYS.iter().any(|key| {
            lower == *key
                || lower == format!("{key}:")
                || lower == format!("{key}=")
                || lower.starts_with(&format!("{key}="))
                || lower.starts_with(&format!("{key}:"))
        });
        if !sensitive {
            result.push_str(chunk);
            continue;
        }
        match trimmed.find(['=', ':']) {
            Some(position) if position + 1 < trimmed.len() => {
                result.push_str(&trimmed[..=position]);
                result.push_str("«redacted»");
                result.push_str(suffix);
            }
            Some(position) => {
                result.push_str(&trimmed[..=position]);
                result.push_str("«redacted»");
                result.push_str(suffix);
                redact_next = true;
            }
            None => {
                result.push_str("«redacted»");
                result.push_str(suffix);
                redact_next = true;
            }
        }
    }
    result
}

pub fn init(verbose: bool) -> LogGuard {
    let dir = log_dir();
    let appender = tracing_appender::rolling::daily(&dir, "peerprint.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let level = if verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(move || RedactingWriter::new(writer.clone()))
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    LogGuard(guard)
}

fn log_dir() -> std::path::PathBuf {
    let preferred = config::data_dir().join("logs");
    if std::fs::create_dir_all(&preferred).is_ok() && writable(&preferred) {
        return preferred;
    }
    let fallback = config::user_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    if ok {
        let _ = std::fs::remove_file(&probe);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::redact_line;

    #[test]
    fn redacts_secret_assignments() {
        let line = "pairing pin=482913 token=deadbeef password: hunter2 ok";
        let redacted = redact_line(line);
        assert!(!redacted.contains("482913"));
        assert!(!redacted.contains("deadbeef"));
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("pin=«redacted»"));
        assert!(redacted.contains("ok"));
    }
}
