//! The stand-in `llama-server` for `mangkon-cli`'s own tests (feature `testkit`): Cargo gives
//! integration tests the path of a binary only from their own package. Never shipped.

fn main() -> std::process::ExitCode {
    mk_models::testkit::llama::main()
}
