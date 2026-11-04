//! The hardware check (US-09.1, NFR-COMP-03/-04).
//!
//! - Pure: [`HardwareProfile`] and [`verdict`], which applies the minimum of US-09.1 AC2 with
//!   the catalog's first tier, and picks the highest tier the machine fits (US-09.3 AC1).
//! - I/O: [`SystemProbe`] reads the machine (OS, CPU, RAM, GPUs, free disk) behind the
//!   [`HardwareProbe`] port, so the app's tests use a fake profile. A probe that fails gives
//!   "GPU not detected" or an unknown value, never an error.
//!
//! "GB" is 10^9 bytes (decided 2026-11-04): an 8 GB card reports about 8.5e9 bytes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Catalog, GB, Model, Tier};

/// Free disk recommended for models and project data (brief §8, US-09.1 AC2). Shown as a
/// note, not part of the verdict: each download checks its own size (decided 2026-11-04).
pub const RECOMMENDED_FREE_DISK: u64 = 30 * GB;
/// Apple Silicon with this much unified memory, but less than the minimum, "may work"
/// (US-09.1 AC4).
pub const APPLE_MAY_WORK: u64 = 24 * GB;
/// How long one probe command may run.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// NFR-COMP-04: the first NVIDIA drivers that run CUDA 12 (ASSUMPTION: NVIDIA's CUDA 12.0
/// release notes, 525.60.13 on Linux and 527.41 on Windows).
pub const CUDA12_DRIVER_LINUX: (u32, u32) = (525, 60);
pub const CUDA12_DRIVER_WINDOWS: (u32, u32) = (527, 41);

pub const MEETS_MINIMUM: &str = "Meets minimum";
pub const BELOW_MINIMUM: &str = "Below minimum";
/// US-09.1 AC3.
pub const VULKAN_BEST_EFFORT: &str = "Supported on a best-effort basis (Vulkan)";
/// US-09.1 AC4.
pub const MAY_WORK: &str = "May work, but not supported";
/// US-09.2 AC1.
pub const TOO_WEAK: &str =
    "This machine is too weak for local AI. Connect an external model endpoint.";
/// US-09.2 AC3 (Q7).
pub const OVERRIDE_WARNING: &str = "Not supported. It may be very slow.";

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Other,
}

impl GpuVendor {
    /// From a PCI vendor id.
    pub fn from_pci(id: u16) -> GpuVendor {
        match id {
            0x10de => GpuVendor::Nvidia,
            0x1002 | 0x1022 => GpuVendor::Amd,
            0x8086 => GpuVendor::Intel,
            0x106b => GpuVendor::Apple,
            _ => GpuVendor::Other,
        }
    }

    /// From a name as the OS reports it.
    pub fn from_name(name: &str) -> GpuVendor {
        let n = name.to_ascii_lowercase();
        if n.contains("nvidia") || n.contains("geforce") || n.contains("quadro") {
            GpuVendor::Nvidia
        } else if n.contains("amd") || n.contains("radeon") || n.contains("ati ") {
            GpuVendor::Amd
        } else if n.contains("intel") || n.contains(" arc") {
            GpuVendor::Intel
        } else if n.contains("apple") {
            GpuVendor::Apple
        } else {
            GpuVendor::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Apple => "Apple",
            GpuVendor::Other => "Other",
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub vendor: GpuVendor,
    pub name: String,
    /// Dedicated memory; `None` when unknown or shared with the system (integrated GPUs).
    pub vram_bytes: Option<u64>,
    pub driver: Option<String>,
}

/// What the check found (US-09.1 AC1).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct HardwareProfile {
    /// `linux`, `macos` or `windows`.
    pub os_family: String,
    /// For display, for example `Linux 24.04 Pop!_OS`.
    pub os: String,
    pub cpu: String,
    pub ram_bytes: u64,
    pub gpus: Vec<Gpu>,
    /// Apple Silicon: the memory CPU and GPU share (the RAM).
    pub unified_bytes: Option<u64>,
    pub apple_silicon: bool,
    /// Free space on the disk holding the models folder; `None` if it could not be read.
    pub free_disk_bytes: Option<u64>,
    /// The folder the free space was read for.
    pub disk_path: String,
}

/// The result of the check (US-09.1 AC2–AC4, NFR-COMP-04).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub meets: bool,
    /// [`MEETS_MINIMUM`] or [`BELOW_MINIMUM`].
    pub label: &'static str,
    /// Why the machine is below the minimum, one plain line per failed part.
    pub reasons: Vec<String>,
    /// Further notes: Vulkan best effort, "may work", an old driver.
    pub notes: Vec<String>,
    /// The highest catalog tier the machine's memory fits, if any (US-09.3 AC1).
    pub tier: Option<String>,
    /// Index in `gpus` of the GPU that decides (the one with the most VRAM).
    pub best_gpu: Option<usize>,
}

/// `bytes` in GB (10^9) for a message: one decimal below 10 GB, none above.
pub fn gb(bytes: u64) -> String {
    let v = bytes as f64 / GB as f64;
    if v < 10.0 {
        let s = format!("{v:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    } else {
        format!("{}", v.round() as u64)
    }
}

impl HardwareProfile {
    /// The GPU with the most VRAM.
    pub fn best_gpu(&self) -> Option<usize> {
        self.gpus
            .iter()
            .enumerate()
            .filter_map(|(i, g)| g.vram_bytes.map(|v| (i, v)))
            .max_by_key(|&(i, v)| (v, std::cmp::Reverse(i)))
            .map(|(i, _)| i)
    }

    fn best_vram(&self) -> u64 {
        self.best_gpu()
            .and_then(|i| self.gpus[i].vram_bytes)
            .unwrap_or(0)
    }

    fn unified(&self) -> u64 {
        self.unified_bytes.unwrap_or(self.ram_bytes)
    }

    /// Whether the machine's memory fits a tier (disk aside).
    pub fn fits_tier(&self, t: &Tier) -> bool {
        if self.apple_silicon {
            t.min_unified_bytes.is_some_and(|u| self.unified() >= u)
        } else {
            self.best_vram() >= t.min_vram_bytes && self.ram_bytes >= t.min_ram_bytes
        }
    }

    /// Whether a model's measured VRAM and RAM fit this machine. On unified memory both come
    /// from the same pool (ASSUMPTION: their sum must fit).
    pub fn fits_model(&self, m: &Model) -> bool {
        if self.apple_silicon {
            m.vram_bytes + m.ram_bytes <= self.unified()
        } else {
            m.vram_bytes <= self.best_vram() && m.ram_bytes <= self.ram_bytes
        }
    }
}

/// Parses a driver version `525.60.13` into `(525, 60)`.
fn driver_version(v: &str) -> Option<(u32, u32)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

/// The hardware verdict for `p` against the catalog's minimum tier (its first).
pub fn verdict(p: &HardwareProfile, catalog: &Catalog) -> Verdict {
    let mut reasons = Vec::new();
    let mut notes = Vec::new();
    let best = p.best_gpu();
    let min = catalog.tiers.first();
    if p.apple_silicon {
        let need = min.and_then(|t| t.min_unified_bytes).unwrap_or(32 * GB);
        let have = p.unified();
        if have < need {
            reasons.push(format!(
                "Unified memory {} GB, needs {} GB",
                gb(have),
                gb(need)
            ));
            if have >= APPLE_MAY_WORK {
                notes.push(MAY_WORK.to_string());
            }
        }
    } else {
        let need_vram = min.map_or(8 * GB, |t| t.min_vram_bytes);
        let need_ram = min.map_or(16 * GB, |t| t.min_ram_bytes);
        match best {
            None if p.gpus.is_empty() => reasons.push(format!(
                "GPU not detected, needs a GPU with {} GB VRAM",
                gb(need_vram)
            )),
            None => reasons.push(format!(
                "GPU memory not detected, needs a GPU with {} GB VRAM",
                gb(need_vram)
            )),
            Some(i) => {
                let g = &p.gpus[i];
                let vram = g.vram_bytes.unwrap_or(0);
                if vram < need_vram {
                    reasons.push(format!(
                        "GPU {}: VRAM {} GB, needs {} GB",
                        g.name,
                        gb(vram),
                        gb(need_vram)
                    ));
                } else if matches!(g.vendor, GpuVendor::Amd | GpuVendor::Intel) {
                    notes.push(VULKAN_BEST_EFFORT.to_string());
                }
                if g.vendor == GpuVendor::Nvidia
                    && let Some(have) = g.driver.as_deref().and_then(driver_version)
                {
                    let want = if p.os_family == "windows" {
                        CUDA12_DRIVER_WINDOWS
                    } else {
                        CUDA12_DRIVER_LINUX
                    };
                    if have < want {
                        notes.push(format!(
                            "The NVIDIA driver {} is older than {}.{}, the first that runs \
                             CUDA 12. Update the driver.",
                            g.driver.as_deref().unwrap_or(""),
                            want.0,
                            want.1
                        ));
                    }
                }
            }
        }
        if p.ram_bytes < need_ram {
            reasons.push(format!(
                "RAM {} GB, needs {} GB",
                gb(p.ram_bytes),
                gb(need_ram)
            ));
        }
    }
    match p.free_disk_bytes {
        Some(free) if free < RECOMMENDED_FREE_DISK => notes.push(format!(
            "Free disk {} GB; about {} GB is recommended for models and project data. \
             Larger models may not fit.",
            gb(free),
            gb(RECOMMENDED_FREE_DISK)
        )),
        None => notes.push("Free disk space could not be read.".to_string()),
        Some(_) => {}
    }
    let tier = catalog
        .tiers
        .iter()
        .rev()
        .find(|t| p.fits_tier(t))
        .map(|t| t.id.clone());
    let meets = reasons.is_empty();
    Verdict {
        meets,
        label: if meets { MEETS_MINIMUM } else { BELOW_MINIMUM },
        reasons,
        notes,
        tier,
        best_gpu: best,
    }
}

/// Reads the machine (ADR-0002 rule 2: I/O behind a port).
pub trait HardwareProbe: Send + Sync {
    /// The profile, with the free space of the disk that holds `models_dir`.
    fn probe(&self, models_dir: &Path) -> HardwareProfile;

    /// Only the free space of the disk that holds `models_dir` (read again before each
    /// download start and after a delete, US-09.4 AC1/AC5).
    fn free_disk(&self, models_dir: &Path) -> Option<u64> {
        self.probe(models_dir).free_disk_bytes
    }
}

/// The real machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProbe;

/// Runs `cmd args` with a time limit; its stdout if it exits 0.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.join().ok()?;
    status.filter(|s| s.success()).map(|_| text)
}

/// The free space of the disk holding `dir`: the mount point that is the longest prefix of the
/// folder (or of its nearest existing parent).
fn free_disk(dir: &Path) -> Option<u64> {
    let mut at: PathBuf = dir.to_path_buf();
    while !at.exists() {
        at = at.parent()?.to_path_buf();
    }
    let at = std::fs::canonicalize(&at).unwrap_or(at);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| at.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// `nvidia-smi` CSV: `name, memory.total (MiB), driver_version`.
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split(',').map(str::trim);
            let name = f.next().filter(|n| !n.is_empty())?.to_string();
            let mib: Option<u64> = f.next().and_then(|v| v.parse().ok());
            let driver = f.next().filter(|d| !d.is_empty()).map(str::to_string);
            Some(Gpu {
                vendor: GpuVendor::Nvidia,
                name,
                vram_bytes: mib.map(|m| m * 1024 * 1024),
                driver,
            })
        })
        .collect()
}

/// The device name in `lspci -mm` output: the fourth quoted field.
pub fn parse_lspci_name(text: &str) -> Option<String> {
    let line = text.lines().next()?;
    let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
    let vendor = quoted.get(1)?;
    let device = quoted.get(2)?;
    Some(format!("{vendor} {device}"))
}

fn read_hex(path: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Linux: NVIDIA from `nvidia-smi`, others from `/sys/class/drm` (AMD `mem_info_vram_total`,
/// Intel discrete `lmem_total_bytes`, ASSUMPTION), names from `lspci` when present.
fn linux_gpus() -> Vec<Gpu> {
    let mut gpus = run(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ],
    )
    .map(|t| parse_nvidia_smi(&t))
    .unwrap_or_default();
    let have_nvidia = !gpus.is_empty();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return gpus;
    };
    let mut cards: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("card"))
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    cards.sort();
    for card in cards {
        let dev = card.join("device");
        let Some(vendor_id) = read_hex(&dev.join("vendor")) else {
            continue;
        };
        let vendor = GpuVendor::from_pci(vendor_id);
        if vendor == GpuVendor::Nvidia && have_nvidia {
            continue;
        }
        let device_id = read_hex(&dev.join("device")).unwrap_or(0);
        let vram = match vendor {
            GpuVendor::Amd => read_u64(&dev.join("mem_info_vram_total")),
            GpuVendor::Intel => read_u64(&dev.join("lmem_total_bytes")),
            _ => None,
        };
        let name = run(
            "lspci",
            &["-mm", "-d", &format!("{vendor_id:04x}:{device_id:04x}")],
        )
        .and_then(|t| parse_lspci_name(&t))
        .unwrap_or_else(|| format!("{} GPU {vendor_id:04x}:{device_id:04x}", vendor.label()));
        let driver = std::fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        gpus.push(Gpu {
            vendor,
            name,
            vram_bytes: vram.filter(|v| *v > 0),
            driver,
        });
    }
    gpus
}

/// macOS (not verified in M5b): `system_profiler SPDisplaysDataType -json`. Apple Silicon has
/// no VRAM of its own; discrete GPUs on Intel Macs report `spdisplays_vram`.
fn macos_gpus() -> Vec<Gpu> {
    let Some(text) = run("system_profiler", &["SPDisplaysDataType", "-json"]) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v["SPDisplaysDataType"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|g| {
                    let name = g["sppci_model"].as_str()?.to_string();
                    let vram = g["spdisplays_vram"]
                        .as_str()
                        .or_else(|| g["spdisplays_vram_shared"].as_str())
                        .and_then(parse_size_words);
                    Some(Gpu {
                        vendor: GpuVendor::from_name(&name),
                        name,
                        vram_bytes: vram,
                        driver: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `8 GB` / `4096 MB` into bytes (binary units, as macOS writes them).
fn parse_size_words(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    match it.next()?.to_ascii_uppercase().as_str() {
        "GB" => Some(n << 30),
        "MB" => Some(n << 20),
        _ => None,
    }
}

/// Windows (not verified in M5b): `Win32_VideoController` for names and drivers, the
/// registry's `HardwareInformation.qwMemorySize` for VRAM (`AdapterRAM` stops at 4 GB).
fn windows_gpus() -> Vec<Gpu> {
    let script = "$ErrorActionPreference='SilentlyContinue'; \
        $k='HKLM:\\SYSTEM\\ControlSet001\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}\\0*'; \
        $mem=@{}; Get-ItemProperty -Path $k | ForEach-Object { \
          if ($_.'HardwareInformation.qwMemorySize') { $mem[$_.DriverDesc]=[uint64]$_.'HardwareInformation.qwMemorySize' } }; \
        Get-CimInstance Win32_VideoController | ForEach-Object { \
          [pscustomobject]@{name=$_.Name; driver=$_.DriverVersion; vram=$mem[$_.Name]} } | ConvertTo-Json -Compress";
    let Some(text) = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    ) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };
    let list = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => Vec::new(),
    };
    list.iter()
        .filter_map(|g| {
            let name = g["name"].as_str()?.to_string();
            let vendor = GpuVendor::from_name(&name);
            // The Windows driver version ends in the NVIDIA version: 32.0.15.7283 → 572.83.
            let driver = g["driver"].as_str().map(|d| {
                if vendor == GpuVendor::Nvidia {
                    nvidia_from_windows_driver(d).unwrap_or_else(|| d.to_string())
                } else {
                    d.to_string()
                }
            });
            Some(Gpu {
                vendor,
                name,
                vram_bytes: g["vram"].as_u64().filter(|v| *v > 0),
                driver,
            })
        })
        .collect()
}

/// `32.0.15.7283` → `572.83`: the last five digits of the Windows driver version.
fn nvidia_from_windows_driver(v: &str) -> Option<String> {
    let digits: String = v.split('.').skip(2).collect();
    if digits.len() < 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let tail = &digits[digits.len() - 5..];
    Some(format!("{}.{}", &tail[..3], &tail[3..]))
}

impl HardwareProbe for SystemProbe {
    fn probe(&self, models_dir: &Path) -> HardwareProfile {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        let ram = sys.total_memory();
        let cpu = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(System::cpu_arch);
        let cores = sys.cpus().len();
        let os_family = std::env::consts::OS.to_string();
        let apple_silicon = cfg!(target_os = "macos") && cfg!(target_arch = "aarch64");
        let gpus = match os_family.as_str() {
            "linux" => linux_gpus(),
            "macos" => macos_gpus(),
            "windows" => windows_gpus(),
            _ => Vec::new(),
        };
        HardwareProfile {
            os: System::long_os_version().unwrap_or_else(|| os_family.clone()),
            os_family,
            cpu: if cores > 0 {
                format!("{cpu} ({cores} threads)")
            } else {
                cpu
            },
            ram_bytes: ram,
            gpus,
            unified_bytes: apple_silicon.then_some(ram),
            apple_silicon,
            free_disk_bytes: free_disk(models_dir),
            disk_path: models_dir.display().to_string(),
        }
    }

    fn free_disk(&self, models_dir: &Path) -> Option<u64> {
        free_disk(models_dir)
    }
}

/// A fixed profile (tests, and the CLI's `--fake-profile`).
#[derive(Clone, Debug)]
pub struct FixedProbe(pub HardwareProfile);

impl HardwareProbe for FixedProbe {
    fn probe(&self, _models_dir: &Path) -> HardwareProfile {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia(vram_gb: f64, driver: &str) -> Gpu {
        Gpu {
            vendor: GpuVendor::Nvidia,
            name: "NVIDIA Test".into(),
            vram_bytes: Some((vram_gb * GB as f64) as u64),
            driver: Some(driver.into()),
        }
    }

    pub(crate) fn profile(gpus: Vec<Gpu>, ram_gb: u64, disk_gb: u64) -> HardwareProfile {
        HardwareProfile {
            os_family: "linux".into(),
            os: "Linux".into(),
            cpu: "Test CPU".into(),
            ram_bytes: ram_gb * GB,
            gpus,
            unified_bytes: None,
            apple_silicon: false,
            free_disk_bytes: Some(disk_gb * GB),
            disk_path: "/models".into(),
        }
    }

    fn apple(unified_gb: u64) -> HardwareProfile {
        HardwareProfile {
            os_family: "macos".into(),
            apple_silicon: true,
            unified_bytes: Some(unified_gb * GB),
            gpus: vec![Gpu {
                vendor: GpuVendor::Apple,
                name: "Apple M3".into(),
                vram_bytes: None,
                driver: None,
            }],
            ..profile(Vec::new(), unified_gb, 100)
        }
    }

    fn cat() -> Catalog {
        Catalog::shipped()
    }

    #[test]
    fn us_09_1_ac2_meets_minimum() {
        // RM-1: RTX 5070 Laptop, 8151 MiB, 64 GB RAM.
        let rm1 = profile(
            vec![Gpu {
                vram_bytes: Some(8151 << 20),
                ..nvidia(0.0, "595.104.02")
            }],
            64,
            500,
        );
        let v = verdict(&rm1, &cat());
        assert!(v.meets, "{v:?}");
        assert_eq!(v.label, MEETS_MINIMUM);
        assert!(v.notes.is_empty(), "{v:?}");
        assert_eq!(v.tier.as_deref(), Some("large-ram"));
        let small = profile(vec![nvidia(8.5, "595.1")], 16, 31);
        let v = verdict(&small, &cat());
        assert!(v.meets, "{v:?}");
        assert_eq!(v.tier.as_deref(), Some("minimum"));
    }

    #[test]
    fn us_09_1_ac2_each_failed_part_has_a_reason() {
        let p = profile(vec![nvidia(6.0, "595.1")], 8, 12);
        let v = verdict(&p, &cat());
        assert!(!v.meets);
        assert_eq!(v.label, BELOW_MINIMUM);
        assert_eq!(
            v.reasons,
            vec![
                "GPU NVIDIA Test: VRAM 6 GB, needs 8 GB",
                "RAM 8 GB, needs 16 GB",
            ]
        );
        // Free disk is a note, not a reason (decided 2026-11-04).
        assert_eq!(
            v.notes,
            vec![
                "Free disk 12 GB; about 30 GB is recommended for models and project data. \
                 Larger models may not fit."
            ]
        );
        assert_eq!(v.tier, None);
        let none = verdict(&profile(Vec::new(), 32, 100), &cat());
        assert_eq!(
            none.reasons,
            vec!["GPU not detected, needs a GPU with 8 GB VRAM"]
        );
        let mut unknown_disk = profile(vec![nvidia(12.0, "595.1")], 32, 0);
        unknown_disk.free_disk_bytes = None;
        let v = verdict(&unknown_disk, &cat());
        assert!(v.meets, "{v:?}");
        assert_eq!(v.notes, vec!["Free disk space could not be read."]);
    }

    #[test]
    fn us_09_1_ac2_apple_32gb_meets() {
        // A 32 GiB Mac reports 34.4e9 bytes.
        let mut p = apple(0);
        p.unified_bytes = Some(32 << 30);
        let v = verdict(&p, &cat());
        assert!(v.meets, "{v:?}");
        assert_eq!(v.tier.as_deref(), Some("minimum"));
        assert_eq!(
            verdict(&apple(64), &cat()).tier.as_deref(),
            Some("large-ram")
        );
    }

    #[test]
    fn us_09_1_ac3_vulkan_best_effort() {
        for vendor in [GpuVendor::Amd, GpuVendor::Intel] {
            let g = Gpu {
                vendor,
                name: "Radeon / Arc".into(),
                vram_bytes: Some(16 * GB),
                driver: None,
            };
            let v = verdict(&profile(vec![g.clone()], 32, 100), &cat());
            assert!(v.meets);
            assert_eq!(v.notes, vec![VULKAN_BEST_EFFORT]);
            // Below the VRAM minimum: no best-effort note, just the reason.
            let small = Gpu {
                vram_bytes: Some(4 * GB),
                ..g
            };
            let v = verdict(&profile(vec![small], 32, 100), &cat());
            assert!(!v.meets && v.notes.is_empty(), "{v:?}");
        }
    }

    #[test]
    fn us_09_1_ac3_best_gpu_decides() {
        let igpu = Gpu {
            vendor: GpuVendor::Intel,
            name: "Intel iGPU".into(),
            vram_bytes: None,
            driver: None,
        };
        let p = profile(vec![igpu, nvidia(8.5, "595.1")], 32, 100);
        let v = verdict(&p, &cat());
        assert!(v.meets);
        assert_eq!(v.best_gpu, Some(1));
        assert!(v.notes.is_empty());
    }

    #[test]
    fn us_09_1_ac4_apple_24gb() {
        let v = verdict(&apple(24), &cat());
        assert!(!v.meets);
        assert_eq!(v.label, BELOW_MINIMUM);
        assert_eq!(v.notes, vec![MAY_WORK]);
        assert_eq!(v.reasons, vec!["Unified memory 24 GB, needs 32 GB"]);
        // 16 GB: below, and no "may work".
        assert!(verdict(&apple(16), &cat()).notes.is_empty());
    }

    #[test]
    fn nfr_comp_04_old_driver() {
        let v = verdict(&profile(vec![nvidia(12.0, "470.82.01")], 32, 100), &cat());
        assert!(v.meets);
        assert_eq!(v.notes.len(), 1);
        assert!(v.notes[0].contains("older than 525.60"), "{v:?}");
        let ok = verdict(&profile(vec![nvidia(12.0, "525.60.13")], 32, 100), &cat());
        assert!(ok.notes.is_empty());
        let mut win = profile(vec![nvidia(12.0, "527.37")], 32, 100);
        win.os_family = "windows".into();
        assert!(verdict(&win, &cat()).notes[0].contains("527.41"));
    }

    #[test]
    fn gb_text() {
        assert_eq!(gb(8151 << 20), "8.5");
        assert_eq!(gb(30 * GB), "30");
        assert_eq!(gb(12 * GB + 400_000_000), "12");
        assert_eq!(gb(146_146_432), "0.1");
        assert_eq!(gb(2 * GB), "2");
    }

    #[test]
    fn probe_parsers() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 5070 Laptop GPU, 8151, 595.104.02\n");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].vram_bytes, Some(8151 << 20));
        assert_eq!(g[0].driver.as_deref(), Some("595.104.02"));
        assert_eq!(
            parse_lspci_name(
                "03:00.0 \"VGA compatible controller\" \"Advanced Micro Devices, Inc. [AMD/ATI]\" \"Navi 31 [Radeon RX 7900 XTX]\" -rc8 \"x\" \"y\"\n"
            )
            .as_deref(),
            Some("Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX]")
        );
        assert_eq!(parse_size_words("8 GB"), Some(8 << 30));
        assert_eq!(
            nvidia_from_windows_driver("32.0.15.7283").as_deref(),
            Some("572.83")
        );
        assert_eq!(driver_version("595.104.02"), Some((595, 104)));
    }

    /// RM-1 check: run with `--ignored` on a machine with an NVIDIA GPU.
    #[test]
    #[ignore = "reads this machine's hardware"]
    fn probe_this_machine() {
        let p = SystemProbe.probe(&std::env::temp_dir());
        let v = verdict(&p, &cat());
        eprintln!("{p:#?}\n{v:#?}");
        assert!(p.ram_bytes > 0);
    }
}
