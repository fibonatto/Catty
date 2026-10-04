//! The per-user daemon: socket server, idle shutdown, and single-instance lock.
//! State lives in `state`, the decision to speak in `policy`, replies in `handlers`.

mod handlers;
mod policy;
mod state;

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::util::now_secs;
use crate::wire::Event;
use crate::{
    flock, prepare_dir, prepare_state_dir, Config, IDLE_EXIT_SECS, LOCK_EX, LOCK_NAME, LOCK_NB,
    MAX_MSG, MAX_THREADS, SOCK_NAME,
};
use handlers::{on_chat, on_event};
use state::Shared;

struct ThreadSlot(Arc<Shared>);

impl Drop for ThreadSlot {
    fn drop(&mut self) {
        self.0.threads.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle(mut s: UnixStream, sh: &Shared) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let mut req = String::new();
    if (&mut s).take(MAX_MSG).read_to_string(&mut req).is_err() {
        return;
    }
    sh.touch();
    let head = req.lines().next().unwrap_or("").trim().to_string();
    let reply = match head.as_str() {
        "ping" => "pong\n".to_string(),
        "status" => sh.status(),
        "quit" => {
            let _ = s.write_all(b"bye\n");
            let _ = fs::remove_file(&sh.sock);
            std::process::exit(0);
        }
        "event" => Event::from_wire(&req)
            .and_then(|ev| on_event(sh, ev))
            .unwrap_or_default(),
        "chat" => on_chat(sh, &req),
        "forget" => {
            sh.forget();
            "ok\n".to_string()
        }
        _ => String::new(),
    };
    let _ = s.write_all(reply.as_bytes());
}

fn idle_watch(sh: Arc<Shared>) {
    loop {
        thread::sleep(Duration::from_secs(60));
        if now_secs().saturating_sub(sh.last_activity.load(Ordering::SeqCst)) > IDLE_EXIT_SECS {
            let _ = fs::remove_file(&sh.sock);
            std::process::exit(0);
        }
    }
}

pub(crate) fn run_daemon() -> io::Result<()> {
    let dir = prepare_dir()?;
    // flock is the single-instance guard: no check-then-bind race between shells,
    // and the kernel releases it if we die, so there is never a stale lock.
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .mode(0o600)
        .open(dir.join(LOCK_NAME))?;
    if unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        return Ok(()); // another daemon already owns it
    }
    let sock = dir.join(SOCK_NAME);
    let _ = fs::remove_file(&sock); // safe: we hold the lock, so this one is stale
    let listener = UnixListener::bind(&sock)?;
    fs::set_permissions(&sock, fs::Permissions::from_mode(0o600))?;

    let cfg = Config::from_env();
    let mem_path = if cfg.memory {
        prepare_state_dir().ok().map(|d| d.join("memory"))
    } else {
        None
    };
    let shared = Arc::new(Shared::new(cfg, sock, mem_path));
    let _ = shared.tz_offset(); // warm the cache so the first prompt doesn't pay for `date`
    {
        let sh = Arc::clone(&shared);
        thread::spawn(move || idle_watch(sh));
    }
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        if shared.threads.load(Ordering::SeqCst) >= MAX_THREADS {
            continue; // drop the connection; the shell just gets no comment
        }
        shared.threads.fetch_add(1, Ordering::SeqCst);
        let slot = ThreadSlot(Arc::clone(&shared));
        thread::spawn(move || handle(stream, &slot.0));
    }
    drop(lock);
    Ok(())
}
