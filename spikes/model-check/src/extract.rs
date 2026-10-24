//! People/org extraction (spike S1-X): one prompt, JSON-schema `response_format`, temperature 0,
//! thinking off, one request at a time unless `--np` says otherwise (NFR-PERF-25). Raw outputs
//! go to `out/extract-<label>.jsonl`, so scoring can be rerun without the model.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mk_bench::EntityKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Client, Usage};
use crate::probe::{Peaks, Sampler};
use crate::runtime::{self, Build};
use crate::segments::Segment;

/// Bumped whenever the prompt or schema changes, so runs say which one they used.
pub const PROMPT_VERSION: u32 = 1;

pub const SYSTEM_PROMPT: &str = "You find the names of people and organizations in a text.

Return every place where the text names a person or an organization:
- people: full names, an initial with a surname, a title with a surname (Mr, Ms, Dr), a first name or a surname alone;
- organizations: companies, public bodies, banks, newspapers, unions, registries, exchanges, including short forms and acronyms.

For each name give:
- \"name\": the name exactly as written in the text, character for character. Keep titles, initials, legal suffixes (Ltd, LLC, S.A.) and capital letters as printed. Do not expand, correct or translate it.
- \"type\": \"person\" or \"org\".
- \"evidence\": a short exact quote from the text (at most 12 words) that contains the name, copied character for character.
- \"confidence\": from 0 to 1, how sure you are that this is the name of a person or an organization.

List a name once for each different way it is written. Do not list job titles alone, places, products, email addresses, web addresses, domain names, account numbers, reference numbers (invoice, contract, tender) or other identifiers. If the text names no person and no organization, return an empty list.

Reply with compact JSON on one line, without indentation.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "mentions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "type": {"type": "string", "enum": ["person", "org"]},
                        "evidence": {"type": "string"},
                        "confidence": {"type": "number"}
                    },
                    "required": ["name", "type", "evidence", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["mentions"],
        "additionalProperties": false
    })
}

pub const MAX_TOKENS: u32 = 2048;

pub fn request(messages: &[Value], schema: &Value, name: &str) -> Value {
    json!({
        "messages": messages,
        "temperature": 0,
        "seed": 1,
        "max_tokens": MAX_TOKENS,
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": name, "strict": true, "schema": schema}
        },
        "chat_template_kwargs": {"enable_thinking": false}
    })
}

pub fn user_message(text: &str) -> String {
    format!("Text:\n<<<\n{text}\n>>>")
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Pred {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: EntityKind,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    mentions: Vec<Pred>,
}

/// Strict parse (INV-AI-03): the schema's shape, no extra fields, confidence in 0..=1, no
/// empty name or evidence.
pub fn parse(content: &str) -> Result<Vec<Pred>, String> {
    let r: Response = serde_json::from_str(content.trim()).map_err(|e| e.to_string())?;
    for p in &r.mentions {
        if !(0.0..=1.0).contains(&p.confidence) {
            return Err(format!("confidence {} out of range", p.confidence));
        }
        if p.name.trim().is_empty() || p.evidence.trim().is_empty() {
            return Err("empty name or evidence".into());
        }
    }
    Ok(r.mentions)
}

/// Whitespace runs become one space; ends trimmed (INV-AI-01 normalization).
pub fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// INV-AI-01: evidence is kept only if found in the segment text after whitespace normalization.
pub fn span_ok(segment: &str, evidence: &str) -> bool {
    let e = normalize_ws(evidence);
    !e.is_empty() && normalize_ws(segment).contains(&e)
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Attempt {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Usage,
    pub ms: u64,
    /// HTTP or parse error; `None` when the response parsed.
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Record {
    pub segment: String,
    pub attempts: Vec<Attempt>,
}

impl Record {
    /// The parsed predictions of the last attempt, if any attempt parsed.
    pub fn preds(&self) -> Option<Vec<Pred>> {
        self.attempts.last().and_then(|a| parse(&a.content).ok())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Host {
    pub cpu_threads: usize,
    pub ram_bytes: u64,
    pub gpu: String,
    pub vram_total_mib: u64,
}

pub fn host() -> Host {
    let ram = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024;
    let gpu = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let (name, total) = gpu.split_once(',').unwrap_or((&gpu, "0"));
    Host {
        cpu_threads: std::thread::available_parallelism().map_or(0, |n| n.get()),
        ram_bytes: ram,
        gpu: name.trim().to_string(),
        vram_total_mib: total.trim().parse().unwrap_or(0),
    }
}

pub fn utc_now() -> String {
    std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RunMeta {
    pub label: String,
    pub task: String,
    pub model_file: String,
    pub model_bytes: u64,
    pub build: Build,
    pub server_args: Vec<String>,
    pub np: usize,
    pub prompt_version: u32,
    pub items: usize,
    pub load_s: f64,
    pub wall_s: f64,
    pub peaks: Peaks,
    pub started_utc: String,
    pub host: Host,
}

/// Model files of a split GGUF (`-00001-of-00003`) count together.
pub fn model_bytes(model: &Path) -> u64 {
    let name = model
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.find("-00001-of-") {
        Some(i) => {
            let prefix = &name[..i];
            std::fs::read_dir(model.parent().unwrap_or(Path::new(".")))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with(prefix) && n.ends_with(".gguf") && n.contains("-of-")
                })
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        }
        None => std::fs::metadata(model).map_or(0, |m| m.len()),
    }
}

pub struct Launch {
    pub server: runtime::Server,
    pub build: Build,
    pub args: Vec<String>,
    pub sampler: Sampler,
    pub load_s: f64,
}

/// Find, version-check and start `llama-server` for `model`, with the memory sampler running.
pub fn launch(
    settings: Option<&Path>,
    model: &Path,
    args: Vec<String>,
    log: &Path,
) -> Result<Launch, String> {
    let found =
        runtime::find(settings, std::env::var_os("PATH").as_deref()).map_err(|e| e.to_string())?;
    let build = runtime::version(found.path()).map_err(|e| e.to_string())?;
    runtime::check_min(build.build, runtime::MIN_BUILD).map_err(|e| e.to_string())?;
    let mut full = vec!["-m".to_string(), model.to_string_lossy().into_owned()];
    full.extend(args);
    let baseline = crate::probe::vram_device_mib();
    let t = Instant::now();
    let server = runtime::Server::start(found.path(), &full, log, Duration::from_secs(1800))
        .map_err(|e| e.to_string())?;
    let load_s = t.elapsed().as_secs_f64();
    let sampler = Sampler::start(server.pid(), baseline);
    eprintln!(
        "server ready on port {} after {load_s:.1} s ({})",
        server.port,
        log.display()
    );
    Ok(Launch {
        server,
        build,
        args: full,
        sampler,
        load_s,
    })
}

/// One segment: request, strict parse, one retry with the parse error (INV-AI-03).
pub fn one(
    client: &Client,
    system: &str,
    schema: &Value,
    schema_name: &str,
    text: &str,
    parse_fn: fn(&str) -> Result<(), String>,
) -> Vec<Attempt> {
    let mut messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": user_message(text)}),
    ];
    let mut attempts = Vec::new();
    for _ in 0..2 {
        let a = match client.chat(&request(&messages, schema, schema_name)) {
            Ok(c) => {
                let error = parse_fn(&c.content).err();
                Attempt {
                    content: c.content,
                    finish_reason: c.finish_reason,
                    usage: c.usage,
                    ms: c.ms,
                    error,
                }
            }
            Err(e) => Attempt {
                content: String::new(),
                finish_reason: None,
                usage: Usage::default(),
                ms: 0,
                error: Some(e),
            },
        };
        let failed = a.error.clone();
        let bad = a.content.clone();
        attempts.push(a);
        match failed {
            None => break,
            Some(e) => {
                messages.push(json!({"role": "assistant", "content": bad}));
                messages.push(json!({"role": "user", "content": format!("That reply did not match the JSON schema ({e}). Reply again with JSON only.")}));
            }
        }
    }
    attempts
}

pub struct Job<'a> {
    pub label: String,
    pub task: &'a str,
    pub model: PathBuf,
    pub settings: Option<PathBuf>,
    pub np: usize,
    pub extra_args: Vec<String>,
    pub out: PathBuf,
}

/// Runs `items` (id, text) through `work` on `np` threads against a fresh server; writes
/// `<task>-<label>.jsonl` and `<task>-<label>.json`.
pub fn run_job<R: Serialize + Send + 'static>(
    job: &Job,
    items: Vec<(String, String)>,
    work: impl Fn(&Client, &str, &str) -> R + Sync + Send + 'static,
) -> Result<RunMeta, String> {
    let ctx = 8192 * job.np;
    let mut args: Vec<String> = [
        "-c",
        &ctx.to_string(),
        "-np",
        &job.np.to_string(),
        "--cache-ram",
        "0",
        "--reasoning",
        "off",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(job.extra_args.iter().cloned());
    let log = job.out.join(format!("{}-{}.log", job.task, job.label));
    let started_utc = utc_now();
    let Launch {
        mut server,
        build,
        args,
        sampler,
        load_s,
    } = launch(job.settings.as_deref(), &job.model, args, &log)?;
    let client = Arc::new(Client::new(server.base(), server.key.clone()));
    // Warm-up, not timed.
    let _ = client.chat(&json!({"messages": [{"role": "user", "content": "Say OK."}], "max_tokens": 4, "chat_template_kwargs": {"enable_thinking": false}}));

    let n = items.len();
    let items = Arc::new(items);
    let next = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<(usize, String)>>> = Arc::new(Mutex::new(Vec::with_capacity(n)));
    let work = Arc::new(work);
    let t = Instant::now();
    let threads: Vec<_> = (0..job.np)
        .map(|_| {
            let (items, next, results, client, work) = (
                items.clone(),
                next.clone(),
                results.clone(),
                client.clone(),
                work.clone(),
            );
            std::thread::spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() {
                        break;
                    }
                    let (id, text) = &items[i];
                    let r = work(&client, id, text);
                    let line = serde_json::to_string(&r).expect("serializes");
                    let mut g = results.lock().expect("lock");
                    g.push((i, line));
                    if g.len() % 25 == 0 {
                        eprintln!(
                            "  {}/{} in {:.0} s",
                            g.len(),
                            items.len(),
                            t.elapsed().as_secs_f64()
                        );
                    }
                }
            })
        })
        .collect();
    for th in threads {
        th.join().map_err(|_| "worker thread panicked")?;
    }
    let wall_s = t.elapsed().as_secs_f64();
    let peaks = sampler.stop();
    server.stop();

    let mut lines = Arc::try_unwrap(results)
        .map_err(|_| "results still shared")?
        .into_inner()
        .expect("lock");
    lines.sort_by_key(|r| r.0);
    let mut jsonl = String::new();
    for (_, line) in &lines {
        jsonl.push_str(line);
        jsonl.push('\n');
    }
    // Labels hold dots (`Qwen3.5`), so no `with_extension`.
    let path = |ext: &str| job.out.join(format!("{}-{}.{ext}", job.task, job.label));
    std::fs::write(path("jsonl"), jsonl).map_err(|e| e.to_string())?;
    let meta = RunMeta {
        label: job.label.clone(),
        task: job.task.to_string(),
        model_file: job
            .model
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        model_bytes: model_bytes(&job.model),
        build,
        server_args: args,
        np: job.np,
        prompt_version: PROMPT_VERSION,
        items: n,
        load_s,
        wall_s,
        peaks,
        started_utc,
        host: host(),
    };
    std::fs::write(
        path("json"),
        serde_json::to_string_pretty(&meta).expect("serializes") + "\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(meta)
}

pub fn parse_unit(content: &str) -> Result<(), String> {
    parse(content).map(|_| ())
}

/// `model-check extract`: every segment through the extraction prompt.
pub fn run(job: &Job, segments: &[Segment]) -> Result<RunMeta, String> {
    let items = segments
        .iter()
        .map(|s| (s.id.clone(), s.text.clone()))
        .collect();
    let schema = Arc::new(schema());
    run_job(job, items, move |client, id, text| Record {
        segment: id.to_string(),
        attempts: one(client, SYSTEM_PROMPT, &schema, "mentions", text, parse_unit),
    })
}

pub fn load_records(path: &Path) -> Result<Vec<Record>, String> {
    std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inv_ai_01_span_check_whitespace() {
        let seg = "Bill to\nVelmora Port   Authority\nAccounts Payable";
        assert!(span_ok(seg, "Velmora Port Authority"));
        assert!(span_ok(seg, "  to Velmora\tPort Authority "));
        assert!(span_ok(seg, "Bill to Velmora"));
        // Case, punctuation and letters are not normalized.
        assert!(!span_ok(seg, "velmora port authority"));
        assert!(!span_ok(seg, "Velmora Port Authority Ltd"));
        assert!(!span_ok(seg, "   "));
    }

    #[test]
    fn inv_ai_03_strict_parse() {
        let ok = r#"{"mentions":[{"name":"Corvin Hale","type":"person","evidence":"Mr Corvin Hale","confidence":0.9}]}"#;
        let p = parse(ok).unwrap();
        assert_eq!(p[0].kind, EntityKind::Person);
        assert_eq!(parse(r#"{"mentions":[]}"#).unwrap(), vec![]);
        for bad in [
            "",
            "not json",
            r#"{"mentions":[{"name":"A","type":"place","evidence":"A","confidence":0.5}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A"}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":1.5}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":0.5,"x":1}]}"#,
            r#"{"mentions":[{"name":" ","type":"org","evidence":"A","confidence":0.5}]}"#,
            r#"{"mentions":[],"extra":true}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":0.5}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn schema_matches_parser() {
        let s = schema();
        let item = &s["properties"]["mentions"]["items"];
        assert_eq!(item["required"].as_array().unwrap().len(), 4);
        assert_eq!(item["properties"]["type"]["enum"], json!(["person", "org"]));
    }
}
