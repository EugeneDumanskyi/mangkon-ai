//! Spike S0, PDF rows (M0b): PDF engine bake-off over TD-1 and TD-3. Throwaway; removed in M2a.
//! Method and results: `docs/08-engine-bakeoff.md`.
//!
//! ```text
//! pdf-bakeoff fetch-pdfium
//! pdf-bakeoff throughput    --engine <e>
//! pdf-bakeoff render-timing --engine <e> [--pages 20] [--seed 1]
//! pdf-bakeoff roundtrip     --engine <e> [--samples 200] [--seed 1] [--all]
//! pdf-bakeoff td3           --engine <e>
//! pdf-bakeoff text          --engine <e> --file <pdf> [--page 1] [--raw]
//! pdf-bakeoff licenses
//! pdf-bakeoff report        [--engines a,b,c]
//! ```

mod bench;
mod corpus;
// The no-engine build exists only as the size baseline for the build probe.
#[cfg_attr(
    not(any(feature = "hayro", feature = "oxide", feature = "pdfium")),
    allow(dead_code)
)]
mod engine;
mod engines;
mod fetch;
mod layout;
mod licenses;
mod report;
mod roundtrip;
mod td3;

use std::path::PathBuf;
use std::process::ExitCode;

/// `spikes/`.
pub fn spikes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn testdata() -> PathBuf {
    std::env::var_os("MK_TESTDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| spikes_dir().join("../testdata"))
}

pub fn out_dir() -> PathBuf {
    spikes_dir().join("out")
}

struct Args(Vec<String>);

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        let i = self.0.iter().position(|a| a == name)?;
        self.0.get(i + 1).map(String::as_str)
    }
    fn num(&self, name: &str, default: u64) -> u64 {
        self.get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn engine(&self) -> Result<Box<dyn engine::PdfEngine>, String> {
        engine::by_name(self.get("--engine").ok_or("--engine <name> is required")?)
    }
}

fn corpus() -> Result<corpus::Corpus, String> {
    corpus::Corpus::load(&testdata().join("td1"))
}

fn print<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serializes"));
}

fn run(cmd: &str, args: &Args) -> Result<(), String> {
    match cmd {
        "fetch-pdfium" => fetch::pdfium(),
        "throughput" => {
            print(&bench::throughput(
                &*args.engine()?,
                &corpus()?,
                &testdata().join("td3"),
            ));
            Ok(())
        }
        "render-timing" => {
            let t = bench::render_timing(
                &*args.engine()?,
                &corpus()?,
                args.num("--pages", 20) as usize,
                args.num("--seed", 1),
            );
            print(&t);
            Ok(())
        }
        "roundtrip" => {
            let c = corpus()?;
            let pool = c.pdf_text_pool();
            let samples = if args.has("--all") {
                pool
            } else {
                corpus::sample(
                    &pool,
                    args.num("--samples", 200) as usize,
                    args.num("--seed", 1),
                )
            };
            let results = roundtrip::run(&*args.engine()?, &c, &samples);
            for r in results.iter().filter(|r| r.step != roundtrip::Step::Pass) {
                println!(
                    "FAIL {:?} {} p{} {:?}: {}",
                    r.step, r.file, r.page, r.raw, r.detail
                );
            }
            let t = roundtrip::Tally::of(&results);
            println!(
                "pass {}/{} = {:.1}%  failed by step {:?}",
                t.passed,
                t.total,
                t.rate() * 100.0,
                t.failed_by_step
            );
            Ok(())
        }
        "td3" => {
            let name = args.get("--engine").ok_or("--engine <name> is required")?;
            engine::by_name(name)?;
            print(&td3::run(name, &testdata().join("td3"), &out_dir()));
            Ok(())
        }
        "text" => {
            let file = args.get("--file").ok_or("--file <path> is required")?;
            let page = args.num("--page", 1) as usize - 1;
            let e = args.engine()?;
            let doc = e
                .open(std::path::Path::new(file))
                .map_err(|e| e.to_string())?;
            let info = doc.page_info(page).map_err(|e| e.to_string())?;
            let chars = doc.chars(page).map_err(|e| e.to_string())?;
            if args.has("--raw") {
                for c in &chars {
                    let r = c.rect;
                    println!(
                        "{:?} [{:.1},{:.1},{:.1},{:.1}]",
                        c.ch,
                        r.x0(),
                        r.y0(),
                        r.x1(),
                        r.y1()
                    );
                }
            }
            println!("{info:?}\n{}", layout::layout(&chars, &info).text);
            Ok(())
        }
        "td3-one" => {
            let (name, file) = (args.0.get(1).ok_or("engine")?, args.0.get(2).ok_or("file")?);
            let e = engine::by_name(name)?;
            println!(
                "{}",
                serde_json::to_string(&td3::one(&*e, std::path::Path::new(file)))
                    .expect("serializes")
            );
            Ok(())
        }
        "licenses" => {
            print(&licenses::by_engine()?);
            Ok(())
        }
        "report" => {
            let names: Vec<String> = match args.get("--engines") {
                Some(list) => list.split(',').map(str::to_string).collect(),
                None => engine::available()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            };
            report::run(&names, &corpus()?)
        }
        _ => Err(format!("unknown command {cmd:?}; see the crate docs")),
    }
}

fn main() -> ExitCode {
    let args = Args(std::env::args().skip(1).collect());
    let Some(cmd) = args.0.first().cloned() else {
        eprintln!("usage: pdf-bakeoff <command> (see src/main.rs)");
        return ExitCode::FAILURE;
    };
    match run(&cmd, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
