mod api;
mod auth;
mod commands;
pub(super) mod config;
mod output;

use std::ffi::OsString;
use std::io::{self, Write};

use output::{Error, Result};

pub(super) fn run(argv: &[OsString]) -> i32 {
    let json = argv.iter().any(|s| s == "--json");
    let matches = match commands::parser()
        .try_get_matches_from(std::iter::once(OsString::from("apeiron task")).chain(argv.iter().cloned()))
    {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return if write!(io::stdout().lock(), "{error}").is_ok() {
                0
            } else {
                1
            };
        }
        Err(error) => return output::failure(commands::argument_error(&error), json),
    };
    let execute = || -> Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::internal())?;
        let result = runtime.block_on(async {
            let config = config::Config::read(&matches).await?;
            let mut api = api::Api::new(config)?;
            // Dropping the command future releases its credential lock before
            // exiting. Abrupt process death still uses the existing stale-lock protocol.
            let result = tokio::select! {
                result = commands::execute(&matches, &mut api) => result,
                code = interrupted() => return Err(Error::new("INTERRUPTED", "Command interrupted", code?)),
            };
            let (data, meta) = result.map_err(|e| {
                if api.write_sent && (e.code == "INVALID_RESPONSE" || e.code == "INTERNAL_ERROR") {
                    Error::unknown(
                        "The write may have succeeded, but its result could not be interpreted",
                        None,
                    )
                } else {
                    e
                }
            })?;
            output::success(data, meta, config::flag(&matches, "json"))
        });
        // Tokio's stdin reader may still be blocked in an OS read after a
        // signal. Do not wait for EOF before allowing this process to exit.
        runtime.shutdown_timeout(std::time::Duration::ZERO);
        result
    };
    match execute() {
        Ok(()) => 0,
        Err(e) if e.code == "INTERRUPTED" => {
            // Cleanup happens when the selected command future is dropped.
            // Preserve signal termination rather than emitting a new JSON error.
            #[cfg(unix)]
            unsafe {
                let signal = e.exit - 128;
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            e.exit
        }
        Err(e) => output::failure(e, json),
    }
}

async fn interrupted() -> Result<i32> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).map_err(|_| Error::internal())?;
        tokio::select! {
            _=tokio::signal::ctrl_c()=>Ok(130),
            _=terminate.recv()=>Ok(143),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.map_err(|_| Error::internal())?;
        Ok(130)
    }
}
