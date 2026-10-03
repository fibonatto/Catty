//! Small, dependency-free helpers: RNG, clock, string formatting, line picking.

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new() -> Rng {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Rng(nanos ^ ((std::process::id() as u64) << 32) ^ 0xA5A5_5A5A_1234_5678)
    }

    // splitmix64: good enough, and unlike `subsec_nanos() % n` it does not
    // degenerate on clocks with coarse resolution.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % (n as u64)) as usize
        }
    }

    pub(crate) fn chance(&mut self, pct: u32) -> bool {
        self.next() % 100 < pct as u64
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn strs(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub(crate) fn basename(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}

pub(crate) fn human_secs(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

pub(crate) fn sample(rng: &mut Rng, lines: &[String], n: usize) -> Vec<String> {
    let mut pool: Vec<String> = lines.to_vec();
    let mut out = Vec::new();
    while out.len() < n && !pool.is_empty() {
        let i = rng.below(pool.len());
        out.push(pool.swap_remove(i));
    }
    out
}

pub(crate) fn pick_line(rng: &mut Rng, lines: &[String], last: &str) -> String {
    if lines.is_empty() {
        return "mrrp.".to_string();
    }
    let mut line = &lines[rng.below(lines.len())];
    if line.as_str() == last && lines.len() > 1 {
        line = &lines[rng.below(lines.len())];
    }
    line.clone()
}
