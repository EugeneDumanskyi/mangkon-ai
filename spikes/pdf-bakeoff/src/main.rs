//! Spike S0: PDF engine bake-off (M0b) and OCR engine bake-off (M0c) over TD-1 and TD-3.
//! Throwaway; removed in M2a. Method and results: `docs/08-engine-bakeoff.md`.
//!
//! ```text
//! pdf-bakeoff fetch-pdfium
//! pdf-bakeoff fetch-ocr
//! pdf-bakeoff throughput    --engine <e>
//! pdf-bakeoff render-timing --engine <e> [--pages 20] [--seed 1]
//! pdf-bakeoff roundtrip     --engine <e> [--samples 200] [--seed 1] [--all]
//! pdf-bakeoff td3           --engine <e>
//! pdf-bakeoff text          --engine <e> --file <pdf> [--page 1] [--raw]
//! pdf-bakeoff ocr-text      --engine <e> --file <image|pdf> [--page 1] [--raw]
//! pdf-bakeoff ocr-quality   --engine <e>
//! pdf-bakeoff ocr-roundtrip --engine <e>
//! pdf-bakeoff ocr-td3       --engine <e>
//! pdf-bakeoff licenses
//! pdf-bakeoff report        [--engines a,b,c]
//! ```
//!
//! Every run is single-threaded: `main` sets `RAYON_NUM_THREADS`, `RTEN_NUM_THREADS` and
//! `OMP_THREAD_LIMIT` to 1 unless they are set, and the ONNX adapter asks for one thread.

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
#[cfg(feature = "ocr")]
mod ocr;
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
        "fetch-ocr" => fetch::ocr(),
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
        #[cfg(feature = "ocr")]
        "ocr-text" => {
            let file =
                std::path::PathBuf::from(args.get("--file").ok_or("--file <path> is required")?);
            let page = args.num("--page", 1) as usize;
            let e =
                ocr::engine::by_name(args.get("--engine").ok_or("--engine <name> is required")?)?;
            let t = std::time::Instant::now();
            let img = ocr::image::load_any(&file, page).map_err(|e| e.to_string())?;
            let t1 = std::time::Instant::now();
            let lines = e
                .recognize(&img, ocr::engine::LANGS)
                .map_err(|e| e.to_string())?;
            let t2 = std::time::Instant::now();
            let laid = ocr::layout::layout(
                &lines,
                mk_kernel::ImageSize {
                    width: img.width,
                    height: img.height,
                },
            );
            if args.has("--raw") {
                for w in laid.geometry.words() {
                    let r = w.rect;
                    println!(
                        "[{:.0},{:.0},{:.0},{:.0}] {:?} {}",
                        r.x0(),
                        r.y0(),
                        r.x1(),
                        r.y1(),
                        w.confidence,
                        laid.text
                            .chars()
                            .skip(w.span.start())
                            .take(w.span.len())
                            .collect::<String>()
                    );
                }
            }
            println!(
                "{}\n-- {}x{} px; decode {:.2} s, ocr {:.2} s, layout {:.3} s",
                laid.text,
                img.width,
                img.height,
                (t1 - t).as_secs_f64(),
                (t2 - t1).as_secs_f64(),
                t2.elapsed().as_secs_f64()
            );
            Ok(())
        }
        #[cfg(feature = "ocr")]
        "ocr-ink" => {
            // Debug: dark-pixel count and grey histogram (16 bins) inside --box x0,y0,x1,y1.
            let file = std::path::PathBuf::from(args.get("--file").ok_or("--file")?);
            let img = ocr::image::load_any(&file, args.num("--page", 1) as usize)
                .map_err(|e| e.to_string())?;
            let b: Vec<u32> = args
                .get("--box")
                .ok_or("--box")?
                .split(',')
                .map(|v| v.parse::<f32>().unwrap_or(0.0) as u32)
                .collect();
            let (mut dark, mut hist) = (0, [0u32; 16]);
            for y in b[1]..b[3].min(img.height) {
                for x in b[0]..b[2].min(img.width) {
                    let v = img.get(x, y);
                    dark += (v < roundtrip::DARK) as u32;
                    hist[v as usize / 16] += 1;
                }
            }
            println!(
                "dark {dark} of {} px; hist {hist:?}",
                (b[2] - b[0]) * (b[3] - b[1])
            );
            Ok(())
        }
        #[cfg(feature = "ocr")]
        "ocr-quality" | "ocr-roundtrip" => {
            let name = args.get("--engine").ok_or("--engine <name> is required")?;
            let c = corpus()?;
            let t = std::time::Instant::now();
            let e = ocr::engine::by_name(name)?;
            let (q, stored) = ocr::quality::run(name, &*e, t.elapsed().as_secs_f64(), &c);
            if cmd == "ocr-quality" {
                print(&q);
                return Ok(());
            }
            let results = ocr::roundtrip::run(&c, &ocr::roundtrip::pool(&c), &stored);
            for r in results.iter().filter(|r| r.step != roundtrip::Step::Pass) {
                println!(
                    "FAIL {:?} {} p{} {:?}: {}",
                    r.step, r.file, r.page, r.raw, r.detail
                );
            }
            let all = roundtrip::Tally::of(&results);
            let found =
                roundtrip::Tally::of(results.iter().filter(|r| r.step != roundtrip::Step::Find));
            println!(
                "found {}/{}; pass {}/{} of found = {:.1}%",
                found.total,
                all.total,
                found.passed,
                found.total,
                found.rate() * 100.0
            );
            Ok(())
        }
        #[cfg(feature = "ocr")]
        "ocr-td3" => {
            let name = args.get("--engine").ok_or("--engine <name> is required")?;
            ocr::engine::by_name(name)?;
            print(&ocr::td3::run(name, &testdata().join("td3"), &out_dir()));
            Ok(())
        }
        #[cfg(feature = "ocr")]
        "ocr-td3-one" => {
            let (name, file) = (args.0.get(1).ok_or("engine")?, args.0.get(2).ok_or("file")?);
            let e = ocr::engine::by_name(name)?;
            println!(
                "{}",
                serde_json::to_string(&ocr::td3::one(&*e, std::path::Path::new(file)))
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
                None => {
                    #[cfg_attr(not(feature = "ocr"), allow(unused_mut))]
                    let mut v = engine::available();
                    #[cfg(feature = "ocr")]
                    v.extend(ocr::engine::available());
                    v.into_iter().map(str::to_string).collect()
                }
            };
            report::run(&names, &corpus()?)
        }
        _ => Err(format!("unknown command {cmd:?}; see the crate docs")),
    }
}

fn main() -> ExitCode {
    for var in ["RAYON_NUM_THREADS", "RTEN_NUM_THREADS", "OMP_THREAD_LIMIT"] {
        if std::env::var_os(var).is_none() {
            // SAFETY: first thing in `main`, before any other thread exists.
            unsafe { std::env::set_var(var, "1") };
        }
    }
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
