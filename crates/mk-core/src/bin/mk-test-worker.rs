//! A worker binary for `mk-core`'s own tests: Cargo gives integration tests the path of a binary
//! only from their own package. Not shipped; the apps run the worker through
//! `mk_core::worker_main_if_requested`.

fn main() -> std::process::ExitCode {
    mk_core::worker_main_if_requested().unwrap_or(std::process::ExitCode::from(2))
}
