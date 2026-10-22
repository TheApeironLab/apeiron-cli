//! IAM behavior is owned by apeiron-iam; this adapter supplies shared SSO/output.
use super::{
    auth::{error::Error, output},
    sso,
};
use std::{
    ffi::OsString,
    io::{self, Write},
    time::Duration,
};
struct PlatformSession;
impl apeiron_iam_cli::Session for PlatformSession {
    async fn platform_credential<'a>(
        &'a self,
        http: &'a reqwest::Client,
        timeout: Duration,
    ) -> apeiron_iam_cli::error::Result<(reqwest::Url, String)> {
        sso::platform_credential(http, timeout)
            .await
            .map_err(|e| apeiron_iam_cli::error::Error {
                code: e.code,
                message: e.message,
                exit: e.exit,
                outcome: e.outcome,
                status: e.status,
            })
    }
}
fn host_error(e: apeiron_iam_cli::error::Error) -> Error {
    Error {
        code: e.code,
        message: e.message,
        exit: e.exit,
        outcome: e.outcome,
        status: e.status,
    }
}
pub(super) fn run(argv: &[OsString]) -> i32 {
    let json = argv.iter().any(|a| a == "--json");
    let args = match apeiron_iam_cli::command()
        .try_get_matches_from(std::iter::once(OsString::from("apeiron iam")).chain(argv.iter().cloned()))
    {
        Ok(args) => args,
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            return if write!(io::stdout().lock(), "{e}").is_ok() {
                0
            } else {
                1
            }
        }
        Err(_) => return output::failure(Error::invalid("Invalid IAM arguments; run apeiron iam --help"), json),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(_) => return output::failure(Error::internal(), json),
    };
    let result = runtime
        .block_on(apeiron_iam_cli::execute(&args, &PlatformSession))
        .map_err(host_error);
    runtime.shutdown_timeout(Duration::ZERO);
    match result {
        Ok(data) => output::success(data, None, json).map_or_else(|e| output::failure(e, json), |_| 0),
        Err(e) => output::failure(e, json),
    }
}
