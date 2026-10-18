//! Tool-level tracing (`fleetify.log`, JSON lines) and the panic hook.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use tracing_subscriber::EnvFilter;

use crate::events::Redactor;

static PANIC_LOG: OnceLock<PathBuf> = OnceLock::new();
/// Set once the credentials are loaded. Until then nothing secret has been read.
static REDACTOR: RwLock<Option<Redactor>> = RwLock::new(None);

/// Installs the redactor applied to every log line written from now on.
pub fn set_redactor(redactor: Redactor) {
    *REDACTOR.write().unwrap_or_else(|p| p.into_inner()) = Some(redactor);
}

/// Applies the installed redactor, if any, to arbitrary text.
pub fn redact(text: &str) -> String {
    match REDACTOR.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
        Some(redactor) => redactor.redact(text),
        None => text.to_string(),
    }
}

/// Log file writer that masks secrets in every write. Each tracing event is
/// written in one call, so a secret cannot be split across two writes.
struct RedactingFile(File);

impl Write for RedactingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.0.write_all(redact(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Exit code used when the tool itself crashes (see PLAN.md, "Exit codes").
pub const EXIT_CRASH: u8 = 4;

/// Writes JSON trace lines to `path`. `RUST_LOG` overrides the default level
/// (`info`, or `debug` with `--debug`).
pub fn init(path: &Path, debug: bool) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file: File = OpenOptions::new().create(true).append(true).open(path)?;
    let default_level = if debug { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    // Errors only if a global subscriber already exists, which is harmless here.
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(Mutex::new(RedactingFile(file)))
        .try_init();
    Ok(())
}

/// Installs a hook that records the panic and its backtrace in `log_path`,
/// prints a pointer to stderr and exits with [`EXIT_CRASH`].
pub fn install_panic_hook(log_path: PathBuf) {
    let _ = PANIC_LOG.set(log_path);
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(%location, message = %payload, %backtrace, "fleetify panicked");
        let log_hint = PANIC_LOG
            .get()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        eprintln!("fleetify: internal error: {payload} at {location}");
        eprintln!("{backtrace}");
        eprintln!("details written to {log_hint}");
        std::process::exit(i32::from(EXIT_CRASH));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_writes_are_redacted_with_the_installed_redactor() {
        let dir = std::env::temp_dir().join(format!("fleetify-log-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let mut writer = RedactingFile(file);

        set_redactor(Redactor::new(["glpat-unit-test-token"]));
        writer
            .write_all(b"{\"msg\":\"auth glpat-unit-test-token\"}\n")
            .unwrap();
        writer.flush().unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(!written.contains("glpat-unit-test-token"), "{written}");
        assert!(written.contains("***"));
        let _ = fs::remove_dir_all(&dir);
    }
}
