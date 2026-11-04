//! `mangkon-cli hardware`, `models` and `model download|delete` (M5b, US-09.1, US-09.3,
//! US-09.4): the hardware check, the shipped catalog with each model's state, and downloads
//! from the hosts the catalog lists. Ctrl-C stops a download; it is Interrupted and resumes
//! from its partial file on the next `model download`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use mk_core::{
    DownloadState, EgressConfig, HardwareView, ModelView, ModelsConfig, Workspace, gb_text,
};
use serde_json::{Value, json};

pub struct Options {
    pub command: Command,
    /// The app's data folder (default: the OS one).
    pub app_data: Option<PathBuf>,
    /// The models folder (default: `models/` in the app's data folder).
    pub dir: Option<PathBuf>,
    pub json: bool,
}

pub enum Command {
    Hardware,
    Models,
    Download(String),
    Delete(String),
}

fn open(opts: &Options) -> Result<Workspace, String> {
    let dir = match &opts.app_data {
        Some(d) => d.clone(),
        None => mk_core::app_data_dir().ok_or("The system gives no folder for app data")?,
    };
    Workspace::open_with_models(
        &dir,
        EgressConfig::default(),
        ModelsConfig {
            models_dir: opts.dir.clone(),
            ..ModelsConfig::default()
        },
    )
    .map_err(|e| e.to_string())
}

pub fn run(opts: &Options) -> ExitCode {
    match run_inner(opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).expect("JSON values"));
}

fn hardware_json(h: &HardwareView) -> Value {
    json!({
        "profile": h.profile,
        "verdict": h.verdict,
        "local_override": h.local_override,
        "local_allowed": h.local_allowed,
    })
}

fn print_hardware(h: &HardwareView) {
    let p = &h.profile;
    println!("OS          {}", p.os);
    println!("CPU         {}", p.cpu);
    match p.unified_bytes {
        Some(u) => println!("Memory      {} GB unified", gb_text(u)),
        None => println!("RAM         {} GB", gb_text(p.ram_bytes)),
    }
    if p.gpus.is_empty() {
        println!("GPU         not detected");
    }
    for (i, g) in p.gpus.iter().enumerate() {
        let vram = g.vram_bytes.map_or("VRAM unknown".to_string(), |v| {
            format!("VRAM {} GB", gb_text(v))
        });
        let driver = g
            .driver
            .as_deref()
            .map_or(String::new(), |d| format!(", driver {d}"));
        let best = if h.verdict.best_gpu == Some(i) {
            "  (used)"
        } else {
            ""
        };
        println!("GPU         {}: {vram}{driver}{best}", g.name);
    }
    match p.free_disk_bytes {
        Some(f) => println!("Free disk   {} GB at {}", gb_text(f), p.disk_path),
        None => println!("Free disk   unknown at {}", p.disk_path),
    }
    println!("Verdict     {}", h.verdict.label);
    for r in h.verdict.reasons.iter().chain(&h.verdict.notes) {
        println!("            {r}");
    }
    if let Some(t) = &h.verdict.tier {
        println!("Tier        {t}");
    }
    if let Some(msg) = h.too_weak {
        println!("Local AI    {msg}");
    } else if h.local_override && !h.hardware_meets {
        println!(
            "Local AI    allowed by the override: {}",
            h.override_warning
        );
    }
}

fn model_json(m: &ModelView) -> Value {
    json!({
        "id": m.id,
        "name": m.name,
        "role": m.role,
        "quant": m.quant,
        "bytes": m.bytes,
        "vram_bytes": m.vram_bytes,
        "ram_bytes": m.ram_bytes,
        "license": m.license_spdx,
        "license_url": m.license_url,
        "recommended": m.recommended,
        "fits": m.fits,
        "disk_needed": m.disk_needed,
        "state": m.state.as_str(),
        "bytes_done": m.bytes_done,
        "error": m.error.as_ref().map(|e| json!({"code": e.code, "message": e.message})),
    })
}

fn state_text(m: &ModelView) -> String {
    match m.state {
        DownloadState::Available => "available".into(),
        DownloadState::Installed => "installed".into(),
        s => format!(
            "{} {}/{} GB",
            s.as_str(),
            gb_text(m.bytes_done),
            gb_text(m.bytes)
        ),
    }
}

fn print_models(models: &[ModelView]) {
    for m in models {
        let role = match m.role {
            mk_core::ModelRole::Extraction => "extraction",
            mk_core::ModelRole::Embeddings => "embeddings",
        };
        let rec = if m.recommended { "  Recommended" } else { "" };
        println!("{}  ({}){rec}", m.id, m.name);
        println!(
            "    {role}, {} GB on disk, needs {} GB VRAM + {} GB RAM, license {} ({}), {}",
            gb_text(m.bytes),
            gb_text(m.vram_bytes),
            gb_text(m.ram_bytes),
            m.license_spdx,
            m.license_url,
            state_text(m)
        );
        if let Some(need) = m.disk_needed {
            println!("    Does not fit: needs {} GB free disk", gb_text(need));
        }
        if let Some(e) = &m.error {
            println!("    {}", e.message);
        }
    }
}

fn view(ws: &Workspace, id: &str) -> Result<ModelView, String> {
    ws.model_catalog()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("No model {id} in the catalog"))
}

fn run_inner(opts: &Options) -> Result<ExitCode, String> {
    let ws = open(opts)?;
    match &opts.command {
        Command::Hardware => {
            let h = ws.hardware().map_err(|e| e.to_string())?;
            if opts.json {
                print_json(&hardware_json(&h));
            } else {
                print_hardware(&h);
            }
        }
        Command::Models => {
            let h = ws.hardware().map_err(|e| e.to_string())?;
            let models = ws.model_catalog().map_err(|e| e.to_string())?;
            if opts.json {
                print_json(&json!({
                    "local_allowed": h.local_allowed,
                    "models": models.iter().map(model_json).collect::<Vec<_>>(),
                }));
            } else {
                if let Some(msg) = h.too_weak {
                    println!("{msg}\n");
                }
                print_models(&models);
            }
        }
        Command::Download(id) => {
            let before = view(&ws, id)?;
            let start = if before.state == DownloadState::Available {
                ws.start_download(id)
            } else {
                ws.resume_download(id)
            };
            start.map_err(|e| e.to_string())?;
            eprintln!(
                "Downloading {} ({} GB, license {})",
                before.name,
                gb_text(before.bytes),
                before.license_spdx
            );
            while ws.downloading().is_some() {
                std::thread::sleep(Duration::from_secs(1));
                if let Ok(m) = view(&ws, id)
                    && m.state == DownloadState::Downloading
                {
                    let eta = m.eta_s.map_or(String::new(), |s| format!(", {s} s left"));
                    eprintln!(
                        "  {:.0}/{:.0} MB, {:.1} MB/s{eta}",
                        m.bytes_done as f64 / 1e6,
                        m.bytes as f64 / 1e6,
                        m.speed_bps as f64 / 1e6
                    );
                }
            }
            ws.wait_download();
            let m = view(&ws, id)?;
            if opts.json {
                print_json(&model_json(&m));
            } else if let Some(e) = &m.error {
                println!("{}: {}", m.state.as_str(), e.message);
            } else {
                println!("{}", state_text(&m));
            }
            if m.state != DownloadState::Installed {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Delete(id) => {
            let free = ws.delete_model(id).map_err(|e| e.to_string())?;
            if opts.json {
                print_json(&json!({ "deleted": id, "free_disk_bytes": free }));
            } else {
                match free {
                    Some(f) => println!("Deleted {id}. Free disk: {} GB", gb_text(f)),
                    None => println!("Deleted {id}."),
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
