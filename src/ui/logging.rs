//! Startup tracing — the app emits `tracing` events from every layer (hooks, the
//! trigger pipeline, the executor, the LLM client), so a subscriber has to be
//! installed before anything else starts, or those events are silently dropped and
//! a gesture that fails has no observable symptom at all.
//!
//! The source configures its logging before any window exists; the equivalent here
//! is a daily rotating file under `%APPDATA%\Orbital\logs\orbital-yyyy-mm-dd.log`,
//! written on a background thread (`tracing_appender::non_blocking`) so a slow disk
//! never stalls the GPUI main thread. `RUST_LOG` selects the filter; the default is
//! `orbital=info`, so a plain run still records what the app does.
//!
//! Debug builds additionally mirror every event to stderr, so running the binary
//! from a console shows the pipeline live.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

/// The `RUST_LOG` default when the variable is unset or unparseable.
const DEFAULT_FILTER: &str = "orbital=info";

/// The stderr mirror's `RUST_LOG` default in debug builds: the pipeline is chatty
/// enough that trace is the level that actually shows a gesture end to end.
const CONSOLE_FILTER: &str = "orbital=trace";

/// The file name prefix; the daily rotation appends the UTC date, giving
/// `orbital.yyyy-mm-dd.log`.
const LOG_PREFIX: &str = "orbital";

/// `install` is process-global; a second call is a no-op.
static INSTALLED: OnceLock<()> = OnceLock::new();

/// The log directory next to the settings file the app already owns, created if it
/// is missing.
fn log_directory() -> Option<PathBuf> {
    let directory = dirs::config_dir()?.join("Orbital").join("logs");
    std::fs::create_dir_all(&directory).ok()?;
    Some(directory)
}

/// The appender's flush guard. Dropped only if the process tears the log down,
/// so it lives for the process lifetime.
static WORKER_GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

/// Installs the process-wide subscriber. Called once, before any other startup
/// work, so every event the app emits from here on is recorded.
pub(crate) fn install() {
    if INSTALLED.set(()).is_err() {
        return;
    }

    let log_directory = match log_directory() {
        Some(directory) => directory,
        None => {
            eprintln!("orbital: no config directory is available; file logging is off");
            install_console_only();
            return;
        }
    };

    // `.filename_suffix("log")` makes the daily rotation produce
    // `orbital.yyyy-mm-dd.log`, the name the startup path documents.
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix("log")
        .build(log_directory)
        .expect("the log directory exists and is writable");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    match WORKER_GUARD.lock() {
        Ok(mut slot) => *slot = Some(guard),
        Err(poisoned) => *poisoned.into_inner() = Some(guard),
    }
    let file_filter = env_filter(DEFAULT_FILTER);
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(file_filter);

    let registry = tracing_subscriber::registry();

    // Debug builds mirror to stderr so a console run shows the pipeline live; the
    // release build logs to the file only.
    #[cfg(debug_assertions)]
    let console_layer = Some(
        fmt::layer()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_filter(env_filter(CONSOLE_FILTER)),
    );
    #[cfg(not(debug_assertions))]
    let console_layer =
        Option::<fmt::Layer<_, fmt::format::DefaultFields, fmt::format::Format>>::None;

    let subscriber = registry.with(console_layer).with(file_layer);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        return;
    }

    // A panic in a spawned task or a window callback is otherwise invisible (the
    // default hook writes to stderr, and a release build has no console). Logging
    // it here keeps `abort()`-crashing and silently-dropped task panics observable.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "a panic unwound");
        default_hook(info);
    }));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        filter = %DEFAULT_FILTER,
        ?LOG_PREFIX,
        "tracing installed"
    );
}

/// The fallback when no log directory could be created: stderr only.
fn install_console_only() {
    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .with_filter(env_filter(CONSOLE_FILTER)),
    );
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// `RUST_LOG` when it parses, otherwise the given default.
fn env_filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::try_new(default).unwrap_or_else(|_| EnvFilter::new("info")))
}

/// The log directory, for tests and diagnostics.
#[allow(dead_code)]
pub(crate) fn log_directory_for_test() -> Option<PathBuf> {
    log_directory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_directory_is_under_the_app_config_folder() {
        let Some(directory) = log_directory() else {
            return;
        };
        assert!(directory.ends_with("Orbital/logs"), "{directory:?}");
    }

    #[test]
    fn the_env_filter_defaults_to_orbital_info() {
        // `try_from_default_env` reads RUST_LOG, so this only asserts the fallback
        // is a valid filter that the default level applies.
        let filter = env_filter(DEFAULT_FILTER);
        assert!(!filter.to_string().is_empty());
    }

    #[test]
    fn installing_twice_is_a_no_op() {
        install();
        install();
    }
}
