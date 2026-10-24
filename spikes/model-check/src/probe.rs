//! Memory peaks of the server process: RSS from `/proc/<pid>/status` (`VmRSS`, `VmHWM`) and
//! VRAM from `nvidia-smi`, sampled every 500 ms on a thread.

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Peaks {
    /// Highest sampled `VmRSS`.
    pub rss_max_bytes: u64,
    /// `VmHWM` (kernel's RSS high-water mark) read at stop. Includes mapped model file pages.
    pub vm_hwm_bytes: u64,
    /// Highest per-process VRAM `nvidia-smi` reported for the PID.
    pub vram_proc_max_mib: u64,
    /// Highest device-wide VRAM in use, and the value before start.
    pub vram_device_max_mib: u64,
    pub vram_device_baseline_mib: u64,
    pub samples: u64,
}

fn status_kib(pid: u32, field: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

fn nvidia(args: &[&str]) -> Option<String> {
    let out = Command::new("nvidia-smi").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Device memory in use, MiB (first GPU).
pub fn vram_device_mib() -> Option<u64> {
    nvidia(&["--query-gpu=memory.used", "--format=csv,noheader,nounits"])?
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn vram_proc_mib(pid: u32) -> Option<u64> {
    nvidia(&[
        "--query-compute-apps=pid,used_memory",
        "--format=csv,noheader,nounits",
    ])?
    .lines()
    .filter_map(|l| {
        let (p, m) = l.split_once(',')?;
        (p.trim().parse::<u32>().ok()? == pid).then(|| m.trim().parse().ok())?
    })
    .next()
}

pub struct Sampler {
    pid: u32,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Peaks>,
}

impl Sampler {
    /// `baseline` is the device VRAM in use before the server started.
    pub fn start(pid: u32, baseline: Option<u64>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut p = Peaks {
                vram_device_baseline_mib: baseline.unwrap_or(0),
                ..Peaks::default()
            };
            while !flag.load(Ordering::Relaxed) {
                if let Some(rss) = status_kib(pid, "VmRSS:") {
                    p.rss_max_bytes = p.rss_max_bytes.max(rss * 1024);
                }
                if let Some(v) = vram_proc_mib(pid) {
                    p.vram_proc_max_mib = p.vram_proc_max_mib.max(v);
                }
                if let Some(v) = vram_device_mib() {
                    p.vram_device_max_mib = p.vram_device_max_mib.max(v);
                }
                p.samples += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
            p
        });
        Sampler { pid, stop, handle }
    }

    /// Call while the process is still alive, so `VmHWM` can be read.
    pub fn stop(self) -> Peaks {
        let hwm = status_kib(self.pid, "VmHWM:");
        self.stop.store(true, Ordering::Relaxed);
        let mut p = self.handle.join().expect("sampler thread");
        p.vm_hwm_bytes = hwm.map(|k| k * 1024).unwrap_or(0);
        p
    }
}
