//! `mangkon-cli` (ADR-0002): `ingest` and `rebuild` (M2d), `render` (M3b), `projects`, `retry`,
//! `remove` and `relocate` (M3c), the kill-test driver (NFR-REL-02/03/04, M2c), `bench` (M4b),
//! `endpoint test` (M5a) and the `--worker` entry.

mod bench;
mod endpoint;
mod ingest;
mod kill_test;
mod manage;

use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
Mangkon AI command line

Usage: mangkon-cli [--help | --version]
       mangkon-cli ingest --project DIR [--ocr-lang LANGS] [--tessdata DIR] [--json] SOURCE...
       mangkon-cli rebuild --project DIR [--json]
       mangkon-cli render --project DIR (--mention ID | --key KEY | --document ID --page N)
                          --out FILE [--tessdata DIR] [--json]
       mangkon-cli projects (list | create NAME --in DIR | forget DIR) [--app-data DIR] [--json]
       mangkon-cli retry --project DIR (--all-failed | --path PATH...) [--tessdata DIR] [--json]
       mangkon-cli remove --project DIR [--json] PATH...
       mangkon-cli relocate --project DIR --root OLD --to NEW [--tessdata DIR] [--json]
       mangkon-cli kill-test --source DIR [--runs N] [--seed N] [--work DIR] [--json]
       mangkon-cli bench [--testdata DIR] [--baseline FILE [--update-baseline]] [--json FILE]
                         [--tessdata DIR] [--work DIR]
       mangkon-cli endpoint test BASE_URL --model NAME [--api-key-env VAR] [--json]

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
  projects    The app's project list, newest first: list it, create the project NAME in a
              new folder DIR/NAME, or remove the project in DIR from the list (its folder is
              kept).
              --app-data  the app's data folder, which holds the list (default: the OS one)
  retry       Queue failed and not-processed files again (all, or those at or inside each
              PATH) and read them.
  remove      Remove the files at or inside each PATH from the project: their data goes
              unless another path has the same content, and connections are rebuilt. The
              files themselves are not touched. Prints how many connections lost evidence.
  relocate    The source OLD was moved to NEW: point the project at NEW and re-scan. Files
              that did not change are not read again.
  kill-test   Ingest DIR (its text-format files) again and again, kill the process at a random
              moment each time, then reopen, check and finish the project. Every run must
              leave a healthy project that ends up equal to a run without a kill.
              --runs   kills (default 20)
              --seed   seed for the kill moments (default 1)
              --work   keep the projects in DIR (default: a temporary folder, removed when
                       every run passes)
              --json   print one JSON summary instead of a table
  bench       Read the TD-1 test corpus into new projects (synthetic/ with the phone region
              GB, public/ with US), score the identifiers found against its ground truth
              per type, text-native and OCR'd, and run the negative checks. Prints no
              identifier value. Exits 1 if a negative check fails, a stored document matches
              no ground-truth document, or a metric fell more than 2 points below the
              baseline.
              --testdata         the corpus folder (default testdata/td1)
              --baseline         the report to compare with
              --update-baseline  write this run's report to the --baseline file instead
              --json             also write this run's report to FILE
              --tessdata         the OCR language folder, as for ingest
              --work             keep the projects in DIR (default: a temporary folder)
  endpoint    test: check the OpenAI-compatible endpoint at BASE_URL (for example
              http://192.168.1.20:8080, /v1 is added when there is no path) with the app's
              fixed test texts: a chat call, structured output and embeddings. Prints where
              the host points, the model it reports and the times. Saves nothing and sends no
              project data. Exits 1 if the chat call fails.
              --model        the model name to ask for
              --api-key-env  the environment variable that holds the API key (never printed)";

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
        Some("projects") => match projects_options(&args[1..]) {
            Ok(opts) => manage::projects(&opts),
            Err(e) => usage_error(&e),
        },
        Some("retry") => match retry_options(&args[1..]) {
            Ok(opts) => manage::retry(&opts),
            Err(e) => usage_error(&e),
        },
        Some("remove") => match remove_options(&args[1..]) {
            Ok(opts) => manage::remove(&opts),
            Err(e) => usage_error(&e),
        },
        Some("relocate") => match relocate_options(&args[1..]) {
            Ok(opts) => manage::relocate(&opts),
            Err(e) => usage_error(&e),
        },
        Some("kill-test") => match kill_test_options(&args[1..]) {
            Ok(opts) => kill_test::run(&opts),
            Err(e) => usage_error(&e),
        },
        Some("endpoint") => match endpoint_options(&args[1..]) {
            Ok(opts) => endpoint::test(&opts),
            Err(e) => usage_error(&e),
        },
        Some("bench") => match bench_options(&args[1..]) {
            Ok(opts) => bench::run(&opts),
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

fn endpoint_options(args: &[String]) -> Result<endpoint::TestOptions, String> {
    let (mut model, mut api_key_env, mut json) = (None, None, false);
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--model" => model = Some(value()?.clone()),
            "--api-key-env" => api_key_env = Some(value()?.clone()),
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown endpoint argument {other:?}"));
            }
            word => words.push(word),
        }
    }
    let ["test", base_url] = words.as_slice() else {
        return Err("endpoint takes test BASE_URL --model NAME".into());
    };
    Ok(endpoint::TestOptions {
        base_url: (*base_url).to_string(),
        model: model.ok_or("endpoint test needs --model NAME")?,
        api_key_env,
        json,
    })
}

fn bench_options(args: &[String]) -> Result<bench::Options, String> {
    let mut opts = bench::Options {
        testdata: PathBuf::from("testdata/td1"),
        baseline: None,
        update_baseline: false,
        json: None,
        tessdata: None,
        work: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--testdata" => opts.testdata = PathBuf::from(value()?),
            "--baseline" => opts.baseline = Some(PathBuf::from(value()?)),
            "--update-baseline" => opts.update_baseline = true,
            "--json" => opts.json = Some(PathBuf::from(value()?)),
            "--tessdata" => opts.tessdata = Some(PathBuf::from(value()?)),
            "--work" => opts.work = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown bench argument {other:?}")),
        }
    }
    if opts.update_baseline && opts.baseline.is_none() {
        return Err("--update-baseline needs --baseline FILE".into());
    }
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

fn projects_options(args: &[String]) -> Result<manage::ProjectsOptions, String> {
    let (mut app_data, mut json, mut location) = (None, false, None);
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--app-data" => app_data = Some(PathBuf::from(value()?)),
            "--in" => location = Some(PathBuf::from(value()?)),
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown projects argument {other:?}"));
            }
            word => words.push(word),
        }
    }
    let command = match (words.as_slice(), location) {
        (["list"], None) => manage::ProjectsCommand::List,
        (["create", name], Some(location)) => manage::ProjectsCommand::Create {
            name: (*name).to_string(),
            location,
        },
        (["forget", path], None) => manage::ProjectsCommand::Forget {
            path: PathBuf::from(path),
        },
        _ => return Err("projects takes list, create NAME --in DIR or forget DIR".into()),
    };
    Ok(manage::ProjectsOptions {
        command,
        app_data,
        json,
    })
}

fn retry_options(args: &[String]) -> Result<manage::RetryOptions, String> {
    let (mut project, mut tessdata, mut json, mut all) = (None, None, false, false);
    let mut paths = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--project" => project = Some(PathBuf::from(value()?)),
            "--tessdata" => tessdata = Some(PathBuf::from(value()?)),
            "--all-failed" => all = true,
            "--path" => paths.push(PathBuf::from(value()?)),
            "--json" => json = true,
            other => return Err(format!("unknown retry argument {other:?}")),
        }
    }
    let target = match (all, paths.is_empty()) {
        (true, true) => manage::RetryTarget::AllFailed,
        (false, false) => manage::RetryTarget::Paths(paths),
        _ => return Err("retry takes --all-failed or --path PATH".into()),
    };
    Ok(manage::RetryOptions {
        project: project.ok_or("retry needs --project DIR")?,
        target,
        tessdata,
        json,
    })
}

fn remove_options(args: &[String]) -> Result<manage::RemoveOptions, String> {
    let (mut project, mut json) = (None, false);
    let mut paths = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" => {
                project = Some(PathBuf::from(it.next().ok_or("--project needs a value")?))
            }
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown remove argument {other:?}"));
            }
            path => paths.push(PathBuf::from(path)),
        }
    }
    if paths.is_empty() {
        return Err("remove needs at least one PATH".into());
    }
    Ok(manage::RemoveOptions {
        project: project.ok_or("remove needs --project DIR")?,
        paths,
        json,
    })
}

fn relocate_options(args: &[String]) -> Result<manage::RelocateOptions, String> {
    let (mut project, mut root, mut to, mut tessdata, mut json) = (None, None, None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--project" => project = Some(PathBuf::from(value()?)),
            "--root" => root = Some(PathBuf::from(value()?)),
            "--to" => to = Some(PathBuf::from(value()?)),
            "--tessdata" => tessdata = Some(PathBuf::from(value()?)),
            "--json" => json = true,
            other => return Err(format!("unknown relocate argument {other:?}")),
        }
    }
    Ok(manage::RelocateOptions {
        project: project.ok_or("relocate needs --project DIR")?,
        root: root.ok_or("relocate needs --root OLD")?,
        to: to.ok_or("relocate needs --to NEW")?,
        tessdata,
        json,
    })
}
