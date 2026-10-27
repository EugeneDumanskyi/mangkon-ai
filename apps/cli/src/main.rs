//! `mangkon-cli` (ADR-0002). Since M2c: the kill-test driver (NFR-REL-02/03/04) and the
//! `--worker` entry; `ingest` and `rebuild` come in M2d, `bench` in M4.

mod kill_test;

use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
Mangkon AI command line

Usage: mangkon-cli [--help | --version]
       mangkon-cli kill-test --source DIR [--runs N] [--seed N] [--work DIR] [--json]

Commands:
  kill-test   Ingest DIR (TXT and CSV files) again and again, kill the process at a random
              moment each time, then reopen, check and finish the project. Every run must
              leave a healthy project that ends up equal to a run without a kill.
              --runs   kills (default 20)
              --seed   seed for the kill moments (default 1)
              --work   keep the projects in DIR (default: a temporary folder, removed when
                       every run passes)
              --json   print one JSON summary instead of a table

ingest, rebuild and bench come in later milestones.";

fn main() -> ExitCode {
    // `mangkon-cli --worker`: a parser worker (ADR-0001, ADR-0007). Not in the help.
    if let Some(code) = mk_core::worker_main_if_requested() {
        return code;
    }
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
        Some("kill-test") => match kill_test_options(&args[1..]) {
            Ok(opts) => kill_test::run(&opts),
            Err(e) => usage_error(&e),
        },
        // The process the kill test kills. Not in the help; M2d brings the public `ingest`.
        Some("kill-test-target") => match &args[1..] {
            [project, source] => kill_test::target(&PathBuf::from(project), &PathBuf::from(source)),
            _ => usage_error("kill-test-target takes PROJECT SOURCE"),
        },
        Some(other) => usage_error(&format!("unknown argument {other:?}")),
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("error: {msg}\n\n{HELP}");
    ExitCode::from(2)
}

fn kill_test_options(args: &[String]) -> Result<kill_test::Options, String> {
    let mut opts = kill_test::Options {
        source: PathBuf::new(),
        runs: 20,
        seed: 1,
        work: None,
        json: false,
    };
    let mut source = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--source" => source = Some(PathBuf::from(value()?)),
            "--runs" => opts.runs = value()?.parse().map_err(|_| "--runs takes a number")?,
            "--seed" => opts.seed = value()?.parse().map_err(|_| "--seed takes a number")?,
            "--work" => opts.work = Some(PathBuf::from(value()?)),
            "--json" => opts.json = true,
            other => return Err(format!("unknown kill-test argument {other:?}")),
        }
    }
    opts.source = source.ok_or("kill-test needs --source DIR")?;
    Ok(opts)
}
