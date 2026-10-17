//! Timing statistics, memory readings and the Markdown report.

use std::fmt::Write as _;
use std::time::Instant;

/// How many timed iterations a measurement gets, after one warm-up run.
#[derive(Clone, Copy, Debug)]
pub struct Iters {
    /// Iterations for a measurement whose warm-up run took under `slow_after_ms`.
    pub normal: usize,
    /// Iterations once the warm-up run took longer than `slow_after_ms`.
    pub slow: usize,
    pub slow_after_ms: f64,
    /// Iterations once the warm-up run took longer than `very_slow_after_ms`.
    pub very_slow: usize,
    pub very_slow_after_ms: f64,
}

impl Iters {
    pub fn quick() -> Self {
        Self { normal: 2, slow: 1, slow_after_ms: 1500.0, very_slow: 1, very_slow_after_ms: 8000.0 }
    }

    pub fn full() -> Self {
        Self { normal: 5, slow: 2, slow_after_ms: 2000.0, very_slow: 1, very_slow_after_ms: 15_000.0 }
    }

    fn count_for(&self, warm_ms: f64) -> usize {
        if warm_ms > self.very_slow_after_ms {
            self.very_slow
        } else if warm_ms > self.slow_after_ms {
            self.slow
        } else {
            self.normal
        }
    }
}

/// Milliseconds of several runs of one thing.
#[derive(Clone, Debug, Default)]
pub struct Timing {
    pub ms: Vec<f64>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

impl Timing {
    pub fn from_ms(ms: Vec<f64>) -> Self {
        Self { ms }
    }

    fn sorted(&self) -> Vec<f64> {
        let mut v = self.ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    pub fn median(&self) -> f64 {
        percentile(&self.sorted(), 0.5)
    }

    pub fn p95(&self) -> f64 {
        percentile(&self.sorted(), 0.95)
    }

    pub fn min(&self) -> f64 {
        self.sorted().first().copied().unwrap_or(f64::NAN)
    }

    pub fn max(&self) -> f64 {
        self.sorted().last().copied().unwrap_or(f64::NAN)
    }

    pub fn n(&self) -> usize {
        self.ms.len()
    }
}

pub fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Run `f` once to warm up, then `iters` times, timing each run.
pub fn bench<R>(iters: Iters, mut f: impl FnMut() -> R) -> Timing {
    let t = Instant::now();
    let _ = f();
    let warm = ms_since(t);
    let n = iters.count_for(warm);
    let mut ms = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let _ = f();
        ms.push(ms_since(t));
    }
    Timing::from_ms(ms)
}

/// Like [`bench`], but `setup` runs before every iteration and is not timed.
pub fn bench_with<S, R>(iters: Iters, mut setup: impl FnMut() -> S, mut f: impl FnMut(S) -> R) -> Timing {
    let s = setup();
    let t = Instant::now();
    let _ = f(s);
    let warm = ms_since(t);
    let n = iters.count_for(warm);
    let mut ms = Vec::with_capacity(n);
    for _ in 0..n {
        let s = setup();
        let t = Instant::now();
        let _ = f(s);
        ms.push(ms_since(t));
    }
    Timing::from_ms(ms)
}

// ----- memory -----

/// Resident set size and peak RSS, bytes, from `/proc/self/status` (0 where unavailable).
pub fn rss() -> (u64, u64) {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else {
        return (0, 0);
    };
    let mut rss = 0;
    let mut peak = 0;
    for line in s.lines() {
        let kb = |l: &str| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024;
        if line.starts_with("VmRSS:") {
            rss = kb(line);
        } else if line.starts_with("VmHWM:") {
            peak = kb(line);
        }
    }
    (rss, peak)
}

/// Reset the kernel's peak-RSS counter so the next [`rss`] peak covers only what follows.
/// Freed heap memory is handed back to the OS first (`malloc_trim`), so the peak measures the
/// scenario and not what earlier scenarios left in the allocator.
pub fn reset_peak() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` takes a pad size and touches only the allocator's own free lists.
    unsafe {
        libc::malloc_trim(0);
    }
    let _ = std::fs::write("/proc/self/clear_refs", "5\n");
}

/// `MemAvailable` from `/proc/meminfo`, bytes (0 where unavailable), capped by the headroom of
/// the memory cgroup this process runs in (a container limit is what the OOM killer enforces,
/// whatever `/proc/meminfo` says).
pub fn mem_available() -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    let avail = s
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024);
    match cgroup_headroom() {
        Some(h) => avail.min(h),
        None => avail,
    }
}

/// `limit - usage` of the memory cgroup, when there is one with a limit.
pub fn cgroup_headroom() -> Option<u64> {
    let read_u64 = |p: &str| std::fs::read_to_string(p).ok()?.trim().parse::<u64>().ok();
    let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    // cgroup v2: a single "0::/path" line.
    if let Some(path) = cg.lines().find_map(|l| l.strip_prefix("0::")) {
        let base = format!("/sys/fs/cgroup{path}");
        if let Some(max) = read_u64(&format!("{base}/memory.max")) {
            let cur = read_u64(&format!("{base}/memory.current")).unwrap_or(0);
            return Some(max.saturating_sub(cur));
        }
    }
    // cgroup v1: the "memory" controller line.
    let path = cg.lines().find_map(|l| {
        let mut it = l.splitn(3, ':');
        let (_, ctrl, path) = (it.next()?, it.next()?, it.next()?);
        ctrl.split(',').any(|c| c == "memory").then(|| path.to_owned())
    })?;
    let base = format!("/sys/fs/cgroup/memory{path}");
    let limit = read_u64(&format!("{base}/memory.limit_in_bytes"))?;
    if limit > (1u64 << 60) {
        return None;
    }
    let usage = read_u64(&format!("{base}/memory.usage_in_bytes")).unwrap_or(0);
    Some(limit.saturating_sub(usage))
}

pub fn mem_total() -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    s.lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024)
}

pub fn gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / (1u64 << 30) as f64)
}

pub fn mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / (1u64 << 20) as f64)
}

// ----- the report -----

pub struct Table {
    pub title: String,
    pub intro: Vec<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub struct Report {
    pub preamble: Vec<String>,
    pub tables: Vec<Table>,
    pub notes: Vec<String>,
    path: std::path::PathBuf,
    echo: bool,
}

impl Report {
    pub fn new(path: impl Into<std::path::PathBuf>, echo: bool) -> Self {
        Self { preamble: Vec::new(), tables: Vec::new(), notes: Vec::new(), path: path.into(), echo }
    }

    pub fn table(&mut self, title: &str, headers: &[&str]) -> usize {
        self.tables.push(Table {
            title: title.to_owned(),
            intro: Vec::new(),
            headers: headers.iter().map(|s| (*s).to_owned()).collect(),
            rows: Vec::new(),
        });
        if self.echo {
            println!("\n### {title}\n");
            println!("| {} |", headers.join(" | "));
            println!("|{}|", headers.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
        }
        self.tables.len() - 1
    }

    pub fn intro(&mut self, table: usize, text: &str) {
        self.tables[table].intro.push(text.to_owned());
    }

    pub fn row(&mut self, table: usize, cells: &[String]) {
        if self.echo {
            println!("| {} |", cells.join(" | "));
        }
        self.tables[table].rows.push(cells.to_vec());
        // Rewrite after every row, so a crash or an out-of-memory kill still leaves a report.
        self.write();
    }

    pub fn note(&mut self, text: &str) {
        if self.echo {
            println!("- {text}");
        }
        self.notes.push(text.to_owned());
    }

    pub fn markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# Eskiz benchmark report\n\n");
        for p in &self.preamble {
            let _ = writeln!(s, "{p}");
        }
        for t in &self.tables {
            let _ = write!(s, "\n### {}\n\n", t.title);
            for i in &t.intro {
                let _ = writeln!(s, "{i}\n");
            }
            let _ = writeln!(s, "| {} |", t.headers.join(" | "));
            let _ = writeln!(s, "|{}|", t.headers.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
            for r in &t.rows {
                let _ = writeln!(s, "| {} |", r.join(" | "));
            }
        }
        if !self.notes.is_empty() {
            s.push_str("\n### Notes\n\n");
            for n in &self.notes {
                let _ = writeln!(s, "- {n}");
            }
        }
        s
    }

    pub fn write(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, self.markdown());
    }
}

/// `"12.3"` for most numbers, `"0.045"` for small ones, `"1234"` for big ones.
pub fn fmt_ms(ms: f64) -> String {
    if ms.is_nan() {
        "-".to_owned()
    } else if ms >= 1000.0 {
        format!("{ms:.0}")
    } else if ms >= 10.0 {
        format!("{ms:.1}")
    } else if ms >= 1.0 {
        format!("{ms:.2}")
    } else {
        format!("{ms:.3}")
    }
}

/// `median / p95 / min` cells of a timing.
pub fn cells(t: &Timing) -> [String; 4] {
    [t.n().to_string(), fmt_ms(t.median()), fmt_ms(t.p95()), fmt_ms(t.min())]
}
