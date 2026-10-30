//! `mem`: samples the memory of the running desktop app and the processes it started
//! (`WebKitWebProcess`, `WebKitNetworkProcess`, workers) for NFR-RES-01 (W3d-1). Linux only:
//! the figures come from `/proc/<pid>/status` and `/proc/<pid>/smaps_rollup`.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

use crate::Result;

/// One process at one sample, in kB.
#[derive(Default, Clone)]
struct Sample {
    name: String,
    rss: u64,
    anon: u64,
    file: u64,
    pss: u64,
    pss_anon: u64,
}

pub fn mem(args: &[&str]) -> Result {
    if !cfg!(target_os = "linux") {
        return Err("mem reads /proc and runs on Linux only".into());
    }
    let mut pid = None;
    let mut seconds = 60u64;
    let mut every = 10u64;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| -> Result<u64> {
            it.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))
        };
        match *arg {
            "--pid" => pid = Some(value("--pid")? as u32),
            "--seconds" => seconds = value("--seconds")?,
            "--every" => every = value("--every")?.max(1),
            other => return Err(format!("mem: unknown argument {other}")),
        }
    }
    let root = match pid {
        Some(p) => p,
        None => find_app()?,
    };
    println!("==> mem: pid {root} and its children, every {every} s for {seconds} s (kB)");
    println!(
        "{:>5}  {:<22} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "t (s)", "process", "RSS", "anon", "file", "PSS", "PSS anon"
    );
    let start = Instant::now();
    let mut totals = Vec::new();
    for tick in 0..=seconds / every {
        let due = Duration::from_secs(tick * every);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
        let tree = tree(root);
        if tree.is_empty() {
            return Err(format!("process {root} has exited"));
        }
        let mut sum = Sample {
            name: "total".into(),
            ..Sample::default()
        };
        let t = start.elapsed().as_secs();
        for s in tree.iter().filter_map(|&p| sample(p)) {
            println!(
                "{t:>5}  {:<22} {:>9} {:>9} {:>9} {:>9} {:>9}",
                s.name, s.rss, s.anon, s.file, s.pss, s.pss_anon
            );
            sum.rss += s.rss;
            sum.anon += s.anon;
            sum.file += s.file;
            sum.pss += s.pss;
            sum.pss_anon += s.pss_anon;
        }
        println!(
            "{t:>5}  {:<22} {:>9} {:>9} {:>9} {:>9} {:>9}",
            sum.name, sum.rss, sum.anon, sum.file, sum.pss, sum.pss_anon
        );
        totals.push(sum.rss);
    }
    let (lo, hi) = (
        totals.iter().min().copied().unwrap_or(0),
        totals.iter().max().copied().unwrap_or(0),
    );
    println!(
        "\nRSS total: min {} MB, max {} MB over {} samples",
        lo / 1024,
        hi / 1024,
        totals.len()
    );
    Ok(())
}

/// The desktop app: a process named `mangkon` that is not a parser worker.
fn find_app() -> Result<u32> {
    let mut found: Vec<u32> = pids()
        .into_iter()
        .filter(|&p| comm(p).as_deref() == Some("mangkon") && !cmdline(p).contains("--worker"))
        .collect();
    found.sort_unstable();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err("no running `mangkon` app found; start it or pass --pid".into()),
        _ => Err(format!(
            "several `mangkon` apps run ({found:?}); pass --pid"
        )),
    }
}

/// `root` and every process below it.
fn tree(root: u32) -> Vec<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for p in pids() {
        if let Some(parent) = parent(p) {
            children.entry(parent).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        if fs::metadata(format!("/proc/{p}")).is_err() {
            continue;
        }
        out.push(p);
        stack.extend(children.get(&p).into_iter().flatten());
    }
    out.sort_unstable();
    out
}

fn pids() -> Vec<u32> {
    fs::read_dir("/proc")
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn comm(pid: u32) -> Option<String> {
    Some(
        fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()?
            .trim()
            .to_string(),
    )
}

fn cmdline(pid: u32) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

fn parent(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...`; comm may hold spaces and parentheses.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// `Key:   1234 kB` lines of a `/proc` file.
fn fields(text: &str) -> BTreeMap<&str, u64> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim(), v.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}

fn sample(pid: u32) -> Option<Sample> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let rollup = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
    let (status, rollup) = (fields(&status), fields(&rollup));
    let get = |m: &BTreeMap<&str, u64>, k: &str| m.get(k).copied().unwrap_or(0);
    let mut name = comm(pid)?;
    if cmdline(pid).contains("--worker") {
        name.push_str(" (worker)");
    }
    Some(Sample {
        name: format!("{name} [{pid}]"),
        rss: get(&status, "VmRSS"),
        anon: get(&status, "RssAnon"),
        file: get(&status, "RssFile"),
        pss: get(&rollup, "Pss"),
        pss_anon: get(&rollup, "Pss_Anon"),
    })
}
