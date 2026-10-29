//! `mangkon-cli` (ADR-0002): `ingest` and `rebuild` (M2d), `render` (M3b), the kill-test driver
//! (NFR-REL-02/03/04, M2c) and the `--worker` entry; `bench` comes in M4.

mod ingest;
mod kill_test;

use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
Mangkon AI command line

Usage: mangkon-cli [--help | --version]
       mangkon-cli ingest --project DIR [--ocr-lang LANGS] [--tessdata DIR] [--json] SOURCE...
       mangkon-cli rebuild --project DIR [--json]
       mangkon-cli render --project DIR (--mention ID | --key KEY | --document ID --page N)
                          --out FILE [--tessdata DIR] [--json]
       mangkon-cli kill-test --source DIR [--runs N] [--seed N] [--work DIR] [--json]

Commands:
  ingest      Open the project in DIR (created if needed), add each SOURCE folder or file
              (a source already in the project is kept), find new and changed files, and
              read them. Prints the counts and every failed file with its reason. Exits 0
              unless the project itself fails; failed files do not change the exit code.
              --ocr-lang  the project's OCR languages, `+`-joined (default eng), for example
                          eng+deu. Other than English, each needs its .traineddata file in
                          the OCR language folder.
              --tessdata  the OCR language folder (default: tessdata/ in the app's data
                          folder; the built-in English file is written there)
  rebuild     Rebuild the connections of the project in DIR from what was read and the
              decisions made, and check that they equal the ones it had. Exits 1 if not.
  render      Show a PDF or image page as the page view does: write it to FILE as a PNG and
              print the highlight boxes of a mention (--mention), of the first mention of a
              connection key such as email:a@b.example (--key), or none (--document, --page).
              Exits 1 if the page image cannot be shown (the note says why).
  kill-test   Ingest DIR (its text-format files) again and again, kill the process at a random
              moment each time, then reopen, check and finish the project. Every run must
              leave a healthy project that ends up equal to a run without a kill.
              --runs   kills (default 20)
              --seed   seed for the kill moments (default 1)
              --work   keep the projects in DIR (default: a temporary folder, removed when
                       every run passes)
              --json   print one JSON summary instead of a table

bench comes in a later milestone.";

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
        Some("ingest") => match ingest_options(&args[1..]) {
            Ok(opts) => ingest::ingest(&opts),
            Err(e) => usage_error(&e),
        },
        Some("render") => match render_options(&args[1..]) {
            Ok(opts) => ingest::render(&opts),
            Err(e) => usage_error(&e),
        },
        Some("rebuild") => match rebuild_options(&args[1..]) {
            Ok(opts) => ingest::rebuild(&opts),
            Err(e) => usage_error(&e),
        },
        Some("kill-test") => match kill_test_options(&args[1..]) {
            Ok(opts) => kill_test::run(&opts),
            Err(e) => usage_error(&e),
        },
        // The process the kill test kills. Not in the help.
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

fn ingest_options(args: &[String]) -> Result<ingest::IngestOptions, String> {
    let mut project = None;
    let mut opts = ingest::IngestOptions {
        project: PathBuf::new(),
        sources: Vec::new(),
        ocr_languages: None,
        tessdata: None,
        json: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" => {
                project = Some(PathBuf::from(it.next().ok_or("--project needs a value")?))
            }
            "--ocr-lang" => {
                opts.ocr_languages = Some(it.next().ok_or("--ocr-lang needs a value")?.clone())
            }
            "--tessdata" => {
                opts.tessdata = Some(PathBuf::from(it.next().ok_or("--tessdata needs a value")?))
            }
            "--json" => opts.json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown ingest argument {other:?}"));
            }
            source => opts.sources.push(PathBuf::from(source)),
        }
    }
    opts.project = project.ok_or("ingest needs --project DIR")?;
    Ok(opts)
}

fn render_options(args: &[String]) -> Result<ingest::RenderOptions, String> {
    let (mut project, mut out, mut tessdata, mut json) = (None, None, None, false);
    let (mut mention, mut key, mut document, mut page) = (None, None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--project" => project = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--tessdata" => tessdata = Some(PathBuf::from(value()?)),
            "--mention" => {
                mention = Some(value()?.parse().map_err(|_| "--mention takes a number")?)
            }
            "--key" => key = Some(value()?.clone()),
            "--document" => {
                document = Some(value()?.parse().map_err(|_| "--document takes a number")?)
            }
            "--page" => page = Some(value()?.parse().map_err(|_| "--page takes a number")?),
            "--json" => json = true,
            other => return Err(format!("unknown render argument {other:?}")),
        }
    }
    let target = match (mention, key, document, page) {
        (Some(m), None, None, None) => ingest::RenderTarget::Mention(m),
        (None, Some(k), None, None) => ingest::RenderTarget::Key(k),
        (None, None, Some(document), Some(page)) => ingest::RenderTarget::Page { document, page },
        _ => {
            return Err(
                "render takes one of --mention ID, --key KEY or --document ID --page N".into(),
            );
        }
    };
    Ok(ingest::RenderOptions {
        project: project.ok_or("render needs --project DIR")?,
        target,
        out: out.ok_or("render needs --out FILE")?,
        tessdata,
        json,
    })
}

fn rebuild_options(args: &[String]) -> Result<ingest::RebuildOptions, String> {
    let mut project = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" => {
                project = Some(PathBuf::from(it.next().ok_or("--project needs a value")?))
            }
            "--json" => json = true,
            other => return Err(format!("unknown rebuild argument {other:?}")),
        }
    }
    Ok(ingest::RebuildOptions {
        project: project.ok_or("rebuild needs --project DIR")?,
        json,
    })
}
