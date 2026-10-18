//! Statically linked application clients. Receiver configuration is deliberately
//! not an input: app commands act with the app user's credentials.
mod auth;
mod iam;
mod kaneo;
mod sso;

use std::ffi::OsString;

pub fn run_task(argv: &[OsString]) -> i32 {
    kaneo::run(argv)
}

pub fn run_sso(argv: &[OsString]) -> i32 {
    sso::run(argv)
}

pub fn run_iam(argv: &[OsString]) -> i32 {
    iam::run(argv)
}
