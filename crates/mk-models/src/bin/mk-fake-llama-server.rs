//! A stand-in `llama-server` for the runtime tests (feature `testkit`). Never shipped.

fn main() -> std::process::ExitCode {
    mk_models::testkit::llama::main()
}
