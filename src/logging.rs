//! File-based logging.
//!
//! Never stdout: it corrupts the alternate screen, and a TUI that scribbles on
//! itself when something goes wrong is worse than one that says nothing.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::paths::Paths;

/// Start logging to `<log dir>/<app>.log`.
///
/// The filter comes from `$<APP>_LOG` when it is set -- the full
/// `tracing-subscriber` syntax, so a single noisy module can be turned up on
/// its own -- and otherwise from `verbose`, which is the `-v` flag both
/// applications have.
///
/// Returns a guard that must be held for the process lifetime. Dropping it
/// stops the writer thread, and the lines still in its queue -- which on a
/// crash are the interesting ones -- go with it.
pub fn init(paths: &Paths, verbose: bool) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let app = paths.app();
    let dir = paths.log_dir()?;
    std::fs::create_dir_all(&dir)?;

    let appender = tracing_appender::rolling::never(&dir, format!("{app}.log"));
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env(env_var(app))
        .unwrap_or_else(|_| EnvFilter::new(default_filter(app, verbose)));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        // Escape codes in a log file are noise in `less` and worse in `grep`.
        .with_ansi(false)
        .init();

    Ok(guard)
}

/// `staramp` -> `STARAMP_LOG`, matching the application's other variables.
fn env_var(app: &str) -> String {
    format!("{}_LOG", app.to_uppercase())
}

/// The application and this crate, at the same level.
///
/// This crate is named explicitly because half of what an application does now
/// happens in here -- the theme lookup, the terminal setup, the graphics probe
/// -- and a default filter naming only the application turns all of it off. A
/// bug report that says "nothing in the log" about code that is no longer in
/// the binary being debugged is the worst kind of quiet.
fn default_filter(app: &str, verbose: bool) -> String {
    let level = if verbose { "debug" } else { "info" };
    format!("{app}={level},starkit={level}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_is_named_after_the_application() {
        assert_eq!(env_var("staramp"), "STARAMP_LOG");
        assert_eq!(env_var("starcord"), "STARCORD_LOG");
    }

    // `init` installs a process-global subscriber and can only be called once,
    // so there is no test of it here: a second one would fail whichever test
    // ran second rather than whichever one was wrong. What can be checked is
    // that it names the file and the variable after the application, which is
    // the part that a second consumer made configurable.
    #[test]
    fn the_default_filter_covers_this_crate_too() {
        assert_eq!(
            default_filter("staramp", false),
            "staramp=info,starkit=info"
        );
        assert_eq!(
            default_filter("starcord", true),
            "starcord=debug,starkit=debug"
        );
    }

    #[test]
    fn the_log_file_is_named_after_the_application() {
        let paths = Paths::new("testapp", "TESTAPP_DIR", "TESTAPP_CONFIG_DIR");
        assert_eq!(format!("{}.log", paths.app()), "testapp.log");
    }
}
