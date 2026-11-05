//! `mangkon-cli` (ADR-0002): `ingest` and `rebuild` (M2d), `render` (M3b), `projects`, `retry`,
//! `remove` and `relocate` (M3c), the kill-test driver (NFR-REL-02/03/04, M2c), `bench` (M4b),
//! `endpoint test` (M5a), `hardware`, `models` and `model download|delete` (M5b), `runtime`
//! and `model try` (M5c), `ai` (M6a) and the `--worker` entry.

mod ai;
mod bench;
mod endpoint;
mod ingest;
mod kill_test;
mod manage;
mod models;

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
       mangkon-cli hardware [--app-data DIR] [--dir DIR] [--json]
       mangkon-cli models [--app-data DIR] [--dir DIR] [--json]
       mangkon-cli model (download ID | delete ID) [--app-data DIR] [--dir DIR] [--json]
       mangkon-cli runtime [--llama-server PATH] [--app-data DIR] [--json]
       mangkon-cli model try ID [--llama-server PATH] [--app-data DIR] [--dir DIR] [--json]
       mangkon-cli ai (status | run [--all | --documents PATH...] | rerun | cancel) --project DIR
                      [--model ID] [--llama-server PATH] [--app-data DIR] [--dir DIR] [--json]

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
              --api-key-env  the environment variable that holds the API key (never printed)
  hardware    Check this machine for local AI: OS, CPU, RAM, each GPU and its memory, free
              disk, and the verdict (Meets minimum or Below minimum, with the reasons).
  models      The model catalog shipped with this version: role, size, memory needed,
              license, Recommended for this machine, and the download state.
  model       download: download the model ID from the hosts in the catalog (or resume its
              partial download), check its SHA-256 and install it. Refused below the
              minimum (unless allowed in the app) or without the free disk it needs. Ctrl-C
              stops it; run it again to resume. Exits 1 unless the model is installed.
              delete: delete the installed model ID and print the free disk.
              try: start the installed model ID with llama-server on 127.0.0.1 (a random
              port and key), send the fixed test texts (chat and structured output, or one
              embedding), print where it listened and the times, then stop it. Exits 1 if
              the test fails.
              --app-data      the app's data folder (default: the OS one)
              --dir           the models folder (default: models/ in the app's data folder)
              --llama-server  the llama-server to use this time (default: the app's setting,
                              then PATH); the setting is not changed
  runtime     The llama-server the app would use: where it was found (settings or PATH), its
              build and backends (from --list-devices), and why it cannot be used, if so.
              Exits 1 if it cannot be used.
  ai          People and organizations found by the AI chosen for extraction. Text of the
              project goes to it. `ingest` does not start this.
              status: what it is doing, the model behind the results, and the counts.
              run: find them in what has not been read yet, and go on with a run that was
              stopped. Up to 1,600 units start by themselves; with more, nothing starts and the
              estimate is printed: pick with --documents PATH..., or --all.
              rerun: read everything again with the chosen model. The results stay as they are
              until it is done, then all change at once.
              cancel: cancel the re-run (the results do not change), or else the rest of the run
              (what it found stays).
              --model  the local model ID to use from now on (changes the app's AI choice)
              Exits 1 if it is Blocked; the reason is printed.";

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
        Some(cmd @ ("hardware" | "models" | "model" | "runtime")) => {
            match models_options(cmd, &args[1..]) {
                Ok(opts) => models::run(&opts),
                Err(e) => usage_error(&e),
            }
        }
        Some("ai") => match ai_options(&args[1..]) {
            Ok(opts) => ai::run(&opts),
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

fn models_options(cmd: &str, args: &[String]) -> Result<models::Options, String> {
    let (mut app_data, mut dir, mut json, mut llama_server) = (None, None, false, None);
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--app-data" => app_data = Some(PathBuf::from(value()?)),
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--llama-server" => llama_server = Some(PathBuf::from(value()?)),
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown {cmd} argument {other:?}"));
            }
            word => words.push(word),
        }
    }
    let command = match (cmd, words.as_slice()) {
        ("hardware", []) => models::Command::Hardware,
        ("models", []) => models::Command::Models,
        ("model", ["download", id]) => models::Command::Download((*id).to_string()),
        ("model", ["delete", id]) => models::Command::Delete((*id).to_string()),
        ("model", ["try", id]) => models::Command::Try((*id).to_string()),
        ("model", _) => return Err("model takes download ID, delete ID or try ID".into()),
        ("runtime", []) => models::Command::Runtime,
        _ => return Err(format!("{cmd} takes no other words")),
    };
    if llama_server.is_some()
        && !matches!(command, models::Command::Runtime | models::Command::Try(_))
    {
        return Err("--llama-server is for runtime and model try".into());
    }
    Ok(models::Options {
        command,
        app_data,
        dir,
        llama_server,
        json,
    })
}

fn ai_options(args: &[String]) -> Result<ai::Options, String> {
    let (mut project, mut app_data, mut dir) = (None, None, None);
    let (mut llama_server, mut model, mut json) = (None, None, false);
    let (mut all, mut documents) = (false, false);
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--project" => project = Some(PathBuf::from(value()?)),
            "--app-data" => app_data = Some(PathBuf::from(value()?)),
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--llama-server" => llama_server = Some(PathBuf::from(value()?)),
            "--model" => model = Some(value()?.clone()),
            "--all" => all = true,
            "--documents" => documents = true,
            "--json" => json = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown ai argument {other:?}"));
            }
            word => words.push(word),
        }
    }
    let command = match words.split_first() {
        Some((&"status", [])) => ai::Command::Status,
        Some((&"cancel", [])) => ai::Command::Cancel,
        Some((&"rerun", [])) => ai::Command::Rerun,
        Some((&"run", paths)) if documents && !all && !paths.is_empty() => ai::Command::Run(
            ai::Scope::Documents(paths.iter().map(PathBuf::from).collect()),
        ),
        Some((&"run", [])) if !documents => {
            ai::Command::Run(if all { ai::Scope::All } else { ai::Scope::Auto })
        }
        Some((&"run", _)) => return Err("ai run takes --all or --documents PATH...".into()),
        _ => return Err("ai takes status, run, rerun or cancel".into()),
    };
    if (all || documents) && !matches!(command, ai::Command::Run(_)) {
        return Err("--all and --documents are for ai run".into());
    }
    Ok(ai::Options {
        command,
        project: project.ok_or("ai needs --project DIR")?,
        app_data,
        dir,
        llama_server,
        model,
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
