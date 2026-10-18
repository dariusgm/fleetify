//! Tool-level tracing (`fleetify.log`, JSON lines) and the panic hook.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_subscriber::EnvFilter;

static PANIC_LOG: OnceLock<PathBuf> = OnceLock::new();

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
        .with_writer(Mutex::new(file))
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
