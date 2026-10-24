//! `mangkon-cli` (ADR-0002). In M2a it only prints help and version; `ingest` and `rebuild` come
//! in M2d, the kill-test driver in M2c, `bench` in M4.

use std::process::ExitCode;

const HELP: &str = "\
Mangkon AI command line

Usage: mangkon-cli [--help | --version]

No commands yet. ingest, rebuild and bench come in later milestones.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            let info = mk_core::app_info();
            println!("mangkon-cli {} ({})", info.version, info.ocr_engine);
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unknown argument {other:?}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
