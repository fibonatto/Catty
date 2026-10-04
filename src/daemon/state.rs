//! Daemon state: per-shell sessions, recent history, counters, and the shared handle.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use crate::memory::Memory;
use crate::util::{local_offset_secs, now_secs, Rng};
use crate::Config;

pub(crate) struct Entry {
    pub(crate) cmd: String,
    pub(crate) exit: i32,
}

pub(crate) struct Session {
    pub(crate) fail_streak: u32,
    pub(crate) last_cmd: String,
    pub(crate) gap: u32, // prompts since the cat last spoke in this shell
    pub(crate) last_seen: Instant,
}

impl Session {
    pub(crate) fn new() -> Session {
        Session {
            fail_streak: 0,
            last_cmd: String::new(),
            gap: 3,
            last_seen: Instant::now(),
        }
    }
}

pub(crate) struct State {
    pub(crate) rng: Rng,
    pub(crate) sessions: HashMap<String, Session>,
    pub(crate) history: VecDeque<Entry>,
    pub(crate) last_line: String,
    pub(crate) mem: Memory,
    llm_ok: u64,
    llm_fail: u64,
    last_error: String,
}

pub(crate) struct Shared {
    pub(crate) cfg: Config,
    state: Mutex<State>,
    pub(crate) busy: AtomicBool,
    pub(crate) threads: AtomicUsize,
    pub(crate) last_activity: AtomicU64,
    pub(crate) sock: PathBuf,
    mem_path: Option<PathBuf>,
    io: Mutex<()>, // serializes memory writes so a stale snapshot never lands last
    tz_secs: AtomicI64,
    tz_at: AtomicU64,
}

impl Shared {
    pub(crate) fn new(cfg: Config, sock: PathBuf, mem_path: Option<PathBuf>) -> Shared {
        let mem = match (&mem_path, cfg.memory) {
            (Some(p), true) => fs::read_to_string(p)
                .map(|s| Memory::from_text(&s))
                .unwrap_or_default(),
            _ => Memory::default(),
        };
        Shared {
            cfg,
            state: Mutex::new(State {
                rng: Rng::new(),
                sessions: HashMap::new(),
                history: VecDeque::new(),
                last_line: String::new(),
                mem,
                llm_ok: 0,
                llm_fail: 0,
                last_error: String::new(),
            }),
            busy: AtomicBool::new(false),
            threads: AtomicUsize::new(0),
            last_activity: AtomicU64::new(now_secs()),
            sock,
            mem_path,
            io: Mutex::new(()),
            tz_secs: AtomicI64::new(0),
            tz_at: AtomicU64::new(0),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn touch(&self) {
        self.last_activity.store(now_secs(), Ordering::SeqCst);
    }

    pub(crate) fn note_llm(&self, r: &Result<String, String>) {
        let mut g = self.lock();
        match r {
            Ok(_) => g.llm_ok += 1,
            Err(e) => {
                g.llm_fail += 1;
                g.last_error = e.clone();
            }
        }
    }

    /// Local UTC offset, refreshed at most hourly (it spawns `date`, so never under the lock).
    pub(crate) fn tz_offset(&self) -> i64 {
        let now = now_secs();
        if now.saturating_sub(self.tz_at.load(Ordering::SeqCst)) > 3600 {
            self.tz_secs.store(local_offset_secs(), Ordering::SeqCst);
            self.tz_at.store(now, Ordering::SeqCst);
        }
        self.tz_secs.load(Ordering::SeqCst)
    }

    /// Persist the memory. Never call this while holding the state lock.
    pub(crate) fn save_memory(&self) {
        let Some(path) = self.mem_path.as_ref() else {
            return;
        };
        let _io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let text = self.lock().mem.to_text();
        let _ = write_atomic(path, &text);
    }

    /// `catty forget`: wipe the memory in RAM and on disk.
    pub(crate) fn forget(&self) {
        let _io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        self.lock().mem = Memory::default();
        if let Some(path) = self.mem_path.as_ref() {
            let _ = fs::remove_file(path);
        }
    }

    pub(crate) fn status(&self) -> String {
        let g = self.lock();
        format!(
            "pid={}\nmodel={}\nhost={}\nsessions={}\nmemory={}\nmemory_days={}\nllm_ok={}\nllm_fail={}\nlast_error={}\n",
            std::process::id(),
            self.cfg.model.as_deref().unwrap_or("(none)"),
            self.cfg.host,
            g.sessions.len(),
            if !self.cfg.memory {
                "off"
            } else if self.mem_path.is_some() {
                "on"
            } else {
                "ram-only"
            },
            g.mem.days_seen(),
            g.llm_ok,
            g.llm_fail,
            if g.last_error.is_empty() {
                "-"
            } else {
                g.last_error.as_str()
            }
        )
    }
}

/// Write to a sibling temp file, then rename: readers never see a half-written memory.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let res = (|| {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_file_roundtrip_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("catty-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("memory");
        write_atomic(&path, "v=1\nmax_streak=9\n").unwrap();
        let m = Memory::from_text(&fs::read_to_string(&path).unwrap());
        assert!(m.to_text().contains("max_streak=9"));
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
