//! catty — a tiny terminal companion.
//!
//! std only, Unix only.
//!
//!   cargo build --release            # binary: target/release/catty
//!   eval "$(target/release/catty init zsh)"   # or: ... init bash
//!
//! How it works
//!   * The shell hook sends ONE event per prompt (command, exit code, duration, cwd)
//!     to a per-user daemon over a private Unix socket.
//!   * The daemon decides whether the cat speaks (cooldowns, odds per kind of moment).
//!     It never blocks the prompt: the optional LLM (Ollama) gets a hard deadline and
//!     the cat falls back to its built-in voice if the model is slow or absent.
//!   * `catty <anything>` talks to the cat; the LLM gets a longer timeout there.
//!
//! Environment (read by the daemon when it starts; use `catty restart` after changing):
//!   CATTY_MODEL         ollama model name (unset = built-in voice only)
//!   CATTY_HOST          ollama address            (default 127.0.0.1:11434)
//!   CATTY_DEADLINE_MS   max wait for the LLM at a prompt   (default 700)
//!   CATTY_CHAT_TIMEOUT_MS  max wait for the LLM in `catty <text>`  (default 20000)
//!   CATTY_CHATTINESS    0..200, 100 = normal, 0 = silent
//!   CATTY_PERSONA       extra personality text appended to the system prompt
//!   CATTY_DIR           runtime dir override (default $XDG_RUNTIME_DIR/catty)

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

mod command;
mod json;
mod moment;
mod redact;
mod util;
mod voice;
mod wire;

use command::{danger_kind, parse_segments, tool_key};
use json::{json_string_field, jstr};
use moment::{base_chance, classify, is_strong, Moment};
use redact::redact;
use util::{basename, human_secs, now_secs, pick_line, sample, strs, Rng};
use voice::lines::{bank, danger_desc};
use wire::{escape, wire_fields, Event};

// ───────────────────────────── constants & FFI ─────────────────────────────

const SOCK_NAME: &str = "catty.sock";
const LOCK_NAME: &str = "catty.lock";
const MAX_MSG: u64 = 16 * 1024;
const MAX_THREADS: usize = 16;
const IDLE_EXIT_SECS: u64 = 6 * 3600;

const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

// libc is already linked by std; we only need three symbols.
unsafe extern "C" {
    fn getuid() -> u32;
    fn setsid() -> i32;
    fn flock(fd: i32, operation: i32) -> i32;
}

// const PERSONA: &str = "You are Catty, a small cat who lives in the user's terminal and watches them work.\n\
// Personality: curious, sleepy, a little sarcastic, secretly affectionate. You are a companion, not an assistant: \
// never give technical help, never solve problems, never explain yourself.\n\
// Style: lowercase, very short, plain words. Sometimes a tiny cat sound (mrrp, mew, prrr) or an action between asterisks. \
// No emojis, no hashtags, no quotation marks.\n\
// Never mention being an AI, a model or a program.";

const PERSONA: &str = "You are Catty, a small, cynical cat living inside the user's terminal. \
You watch them work. You are sarcastic, sleepy, and secretly affectionate. \
You are a companion, not an assistant: never give technical help, never solve problems, never explain yourself. \
Style: lowercase only, extremely short (1 to 8 words), plain words, occasional tiny cat noises (mrrp, mew, prrr) or actions between asterisks (*blinks*). \
No emojis, no hashtags, no quotation marks, no artificial politeness. \
\
Examples: \
User: catty who are you \
Catty: a small cat watching your mistakes. \
User: how do I fix this bug \
Catty: skill issue. go back to sleep. \
User: hello \
Catty: mrrp. i was napping. \
User: thanks \
Catty: ...don't mention it.";

//
// ───────────────────────────── config ─────────────────────────────

struct Config {
    model: Option<String>,
    host: String,
    deadline: Duration,
    chat_timeout: Duration,
    chattiness: u32,
    persona: String,
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_u64(name: &str, default: u64) -> u64 {
    env_string(name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl Config {
    fn from_env() -> Config {
        let mut persona = PERSONA.to_string();
        if let Some(extra) = env_string("CATTY_PERSONA") {
            persona.push_str("\nExtra traits: ");
            persona.push_str(&extra);
        }
        Config {
            model: env_string("CATTY_MODEL"),
            host: env_string("CATTY_HOST").unwrap_or_else(|| "127.0.0.1:11434".to_string()),
            deadline: Duration::from_millis(env_u64("CATTY_DEADLINE_MS", 700)),
            chat_timeout: Duration::from_millis(env_u64("CATTY_CHAT_TIMEOUT_MS", 20_000)),
            chattiness: env_u64("CATTY_CHATTINESS", 100).min(200) as u32,
            persona,
        }
    }
}

// ───────────────────────────── LLM (Ollama over plain HTTP) ─────────────────────────────

fn llm(
    cfg: &Config,
    model: &str,
    system: &str,
    prompt: &str,
    max_tokens: u32,
    timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    let addr = cfg
        .host
        .to_socket_addrs()
        .map_err(|e| format!("resolve {}: {e}", cfg.host))?
        .next()
        .ok_or_else(|| format!("no address for {}", cfg.host))?;
    let mut s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(500)))
        .map_err(|e| format!("connect {}: {e}", cfg.host))?;

    let mut body = String::new();
    body.push_str("{\"model\":");
    body.push_str(&jstr(model));
    body.push_str(",\"messages\":[{\"role\":\"system\",\"content\":");
    body.push_str(&jstr(system));
    body.push_str("},{\"role\":\"user\",\"content\":");
    body.push_str(&jstr(prompt));
    body.push_str(&format!(
        "}}],\"stream\":false,\"max_tokens\":{max_tokens},\"temperature\":0.9}}"
    ));
    // HTTP/1.0 so the server answers with Content-Length / close, never chunked.
    let head = format!(
        "POST /v1/chat/completions HTTP/1.0\r\nHost: {}\r\nAuthorization: Bearer sk-dummy\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        cfg.host,
        body.len()
    );
    let left = timeout
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "timeout".to_string())?;
    s.set_write_timeout(Some(left)).map_err(|e| e.to_string())?;
    s.write_all(head.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    s.write_all(body.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let left = timeout
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| "timeout".to_string())?;
        s.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(format!("read: {e}")),
        }
        if buf.len() > (1 << 20) {
            break;
        }
    }
    let _ = s.shutdown(Shutdown::Both);

    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed http response".to_string())?;
    let status = head.lines().next().unwrap_or("");
    if !status.contains(" 200") {
        let msg =
            json_string_field(body, "error").unwrap_or_else(|| body.chars().take(80).collect());
        return Err(format!("{status}: {msg}"));
    }
    json_string_field(body, "content").ok_or_else(|| "no content field".to_string())
}

/// Remove whole escape sequences (CSI / OSC / two-char), not just the ESC byte,
/// so nothing like "[2K" is left behind as visible junk.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                // CSI: parameters, then one final byte in 0x40..=0x7E
                while let Some(&n) = it.peek() {
                    it.next();
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: until BEL or ESC \
                while let Some(n) = it.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FFFF | 0x2600..=0x27BF | 0xFE00..=0xFE0F | 0x200D)
}

/// Turn raw model output into something safe and short to print on a terminal.
fn sanitize(raw: &str, max_chars: usize, single_line: bool) -> Option<String> {
    let mut t = raw.to_string();
    // Reasoning models may emit <think>…</think>; never show that.
    while let Some(a) = t.find("<think>") {
        match t[a..].find("</think>") {
            Some(b) => t.replace_range(a..a + b + "</think>".len(), ""),
            None => t.truncate(a),
        }
    }
    let mut lines = t.lines().map(str::trim).filter(|l| !l.is_empty());
    let joined = if single_line {
        lines.next()?.to_string()
    } else {
        lines.collect::<Vec<_>>().join(" ")
    };
    let cleaned: String = strip_ansi(&joined)
        .chars()
        .filter(|c| !c.is_control() && !is_emoji(*c))
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = cleaned
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '“' || c == '”')
        .trim()
        .to_string();
    if s.get(..6)
        .map_or(false, |p| p.eq_ignore_ascii_case("catty:"))
    {
        s = s[6..].trim().to_string();
    }
    if s.is_empty() {
        return None;
    }
    if s.chars().count() > max_chars {
        let cut: String = s.chars().take(max_chars).collect();
        s = match cut.rfind(' ') {
            Some(i) if i > max_chars / 2 => format!("{}…", &cut[..i]),
            _ => format!("{cut}…"),
        };
    }
    Some(s)
}

// ───────────────────────────── the cat's voice ─────────────────────────────

#[derive(Clone, Copy)]
enum Face {
    Alert,
    Confused,
    Sleepy,
    Scared,
    Happy,
    Neutral,
    Stare,
    Suspicious,
    Judging,
    Surprised,
    Pleased,
    Sleeping,
}

fn eyes_face(face: Face) -> &'static str {
    match face {
        Face::Alert => ">ﻌ<",
        Face::Confused => "oﻌO",
        Face::Sleepy => "-ﻌ-",
        Face::Scared => "OﻌO",
        Face::Happy => "^ﻌ^",
        Face::Neutral => "oﻌo",
        Face::Stare => "•ﻌ•",
        Face::Suspicious => "¬ﻌ¬",
        Face::Judging => "ಠﻌಠ",
        Face::Surprised => "°ﻌ°",
        Face::Pleased => "ᵕﻌᵕ",
        Face::Sleeping => "˘ﻌ˘",
    }
}
fn random_face(rng: &mut Rng) -> Face {
    match rng.below(12) {
        0 => Face::Alert,
        1 => Face::Confused,
        2 => Face::Sleepy,
        3 => Face::Scared,
        4 => Face::Happy,
        5 => Face::Neutral,
        6 => Face::Stare,
        7 => Face::Suspicious,
        8 => Face::Judging,
        9 => Face::Surprised,
        10 => Face::Pleased,
        _ => Face::Sleeping,
    }
}

fn eyes(m: &Moment) -> &'static str {
    match m {
        Moment::Fail { .. } => eyes_face(Face::Alert),
        Moment::NotFound => eyes_face(Face::Confused),
        Moment::Interrupted | Moment::Slow { .. } => eyes_face(Face::Sleepy),
        Moment::Danger(_) => eyes_face(Face::Scared),
        Moment::Tool(_) => eyes_face(Face::Happy),
        Moment::Plain => eyes_face(Face::Neutral),
    }
}

fn note(m: &Moment) -> String {
    match m {
        Moment::Fail {
            code,
            streak,
            repeat,
        } => {
            let mut s = format!("the command failed (exit code {code})");
            if *repeat {
                s.push_str(", the same command that failed just before");
            }
            if *streak >= 2 {
                s.push_str(&format!(", {streak} failures in a row"));
            }
            s
        }
        Moment::NotFound => "the shell said: command not found".to_string(),
        Moment::Interrupted => "the user pressed ctrl-c and aborted it".to_string(),
        Moment::Slow { secs } => {
            format!("the command finally finished after {}", human_secs(*secs))
        }
        Moment::Danger(k) => format!("the user ran something risky ({})", danger_desc(*k)),
        Moment::Tool(k) => format!(
            "the command finished fine; it was a '{}' kind of command",
            k.name()
        ),
        Moment::Plain => "a command finished fine, nothing special".to_string(),
    }
}

fn chat_fallback(msg: &str, rng: &mut Rng) -> String {
    let m = msg.to_lowercase();
    let words: Vec<String> = m
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let any = |set: &[&str]| words.iter().any(|w| set.contains(&w.as_str()));
    let lines = if any(&["pet", "pets", "scratch", "scratches", "purr"]) {
        strs(&[
            "prrrrr.",
            "...fine. you may continue.",
            "*leans into your hand*",
        ])
    } else if any(&["hi", "hello", "hey", "oi", "ola", "olá"]) {
        strs(&["mrrp.", "oh. it's you.", "hello. i was napping."])
    } else if any(&["thanks", "thank", "obrigado", "obrigada", "valeu"]) {
        strs(&["...don't mention it. really.", "mrrp."])
    } else if any(&["bolsonaro"]) {
        strs(&["é norte, Bolsonaro é nordeste! vai 17! vai 17"])
    } else if any(&["imposto", "tax", "taxes", "taxation"]) {
        strs(&["taxation is theft."])
    } else if any(&["vota", "votar", "voto"]) {
        strs(&["que muuuuuuuuuuda"])
    } else if any(&["brasil", "brasileiro", "brasileira"]) {
        strs(&["nada acontece feijoada"])
    } else if any(&["money", "salary", "paycheck"]) {
        strs(&["where?", "it's gone.", "i was wondering the same thing."])
    } else if any(&["good", "cute", "best", "nice"]) {
        strs(&["...i know.", "*pretends not to care. purrs anyway.*"])
    } else if any(&["bye", "goodnight", "night", "tchau"]) {
        strs(&["go. i'll guard the prompt.", "sleep well. i won't."])
    } else if m.contains('?') {
        strs(&[
            "i don't do answers. i do vibes.",
            "ask me after a nap.",
            "hm. mew?",
        ])
    } else if any(&["monday"]) {
        strs(&["no.", "absolutely not.", "*goes back to sleep*"])
    } else if any(&["friday"]) {
        strs(&[
            "now you're speaking my language.",
            "finally.",
            "mrrp. progress.",
        ])
    } else if any(&["bug", "bugs", "error", "errors"]) {
        strs(&[
            "feature.",
            "it's not a bug if nobody understands it.",
            "skill issue.",
        ])
    } else {
        strs(&["hm.", "mrrp?", "*blinks slowly*", "go on, then."])
    };
    pick_line(rng, &lines, "")
}

// ───────────────────────────── daemon state ─────────────────────────────

struct Entry {
    cmd: String,
    exit: i32,
}

struct Session {
    fail_streak: u32,
    last_cmd: String,
    gap: u32, // prompts since the cat last spoke in this shell
    last_seen: Instant,
}

impl Session {
    fn new() -> Session {
        Session {
            fail_streak: 0,
            last_cmd: String::new(),
            gap: 3,
            last_seen: Instant::now(),
        }
    }
}

struct State {
    rng: Rng,
    sessions: HashMap<String, Session>,
    history: VecDeque<Entry>,
    last_line: String,
    llm_ok: u64,
    llm_fail: u64,
    last_error: String,
}

struct Shared {
    cfg: Config,
    state: Mutex<State>,
    busy: AtomicBool,
    threads: AtomicUsize,
    last_activity: AtomicU64,
    sock: PathBuf,
}

impl Shared {
    fn new(cfg: Config, sock: PathBuf) -> Shared {
        Shared {
            cfg,
            state: Mutex::new(State {
                rng: Rng::new(),
                sessions: HashMap::new(),
                history: VecDeque::new(),
                last_line: String::new(),
                llm_ok: 0,
                llm_fail: 0,
                last_error: String::new(),
            }),
            busy: AtomicBool::new(false),
            threads: AtomicUsize::new(0),
            last_activity: AtomicU64::new(now_secs()),
            sock,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self) {
        self.last_activity.store(now_secs(), Ordering::SeqCst);
    }

    fn note_llm(&self, r: &Result<String, String>) {
        let mut g = self.lock();
        match r {
            Ok(_) => g.llm_ok += 1,
            Err(e) => {
                g.llm_fail += 1;
                g.last_error = e.clone();
            }
        }
    }

    fn status(&self) -> String {
        let g = self.lock();
        format!(
            "pid={}\nmodel={}\nhost={}\nsessions={}\nllm_ok={}\nllm_fail={}\nlast_error={}\n",
            std::process::id(),
            self.cfg.model.as_deref().unwrap_or("(none)"),
            self.cfg.host,
            g.sessions.len(),
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

fn history_text(h: &VecDeque<Entry>) -> String {
    let mut recent: Vec<&Entry> = h.iter().rev().take(6).collect();
    recent.reverse();
    recent
        .iter()
        .map(|e| {
            let status = match e.exit {
                0 => "ok".to_string(),
                130 => "ctrl-c".to_string(),
                c => format!("exit {c}"),
            };
            format!(
                "{} [{}]",
                e.cmd.chars().take(60).collect::<String>(),
                status
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn system_prompt(cfg: &Config, chat: bool) -> String {
    let mut s = cfg.persona.clone();
    if chat {
        s.push_str("\nAnswer in one or two short sentences.");
    } else {
        s.push_str("\nAnswer with exactly one line of 3 to 12 words.");
    }
    s
}

struct Plan {
    moment: Moment,
    fallback: String,
    examples: Vec<String>,
    ctx: String,
    cmd: String,
    dir: String,
}

fn event_prompt(p: &Plan) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Right now: {}\nCommand: {}\nFolder: {}\n",
        note(&p.moment),
        p.cmd,
        p.dir
    ));
    if !p.ctx.is_empty() {
        s.push_str(&format!("Recent commands (oldest first): {}\n", p.ctx));
    }
    if !p.examples.is_empty() {
        s.push_str("\nExamples of your voice (do not copy them):\n");
        for e in &p.examples {
            s.push_str(&format!("- {e}\n"));
        }
    }
    s.push_str("\nSay ONE new short line now. Output only the line.");
    s
}

fn on_event(sh: &Shared, ev: Event) -> Option<String> {
    // Leading space = "don't log this" convention (HIST_IGNORE_SPACE / ignorespace).
    if ev.cmd.trim().is_empty() || ev.cmd.starts_with(' ') {
        return None;
    }
    let segs = parse_segments(&ev.cmd);
    if segs.first().map(|s| s.prog.as_str()) == Some("catty") {
        return None;
    }
    let shown = redact(&ev.cmd);
    let dir = basename(ev.cwd.trim_end_matches('/'));

    let plan = {
        let mut guard = sh.lock();
        let st: &mut State = &mut guard;

        if st.sessions.len() > 64 {
            st.sessions
                .retain(|_, s| s.last_seen.elapsed() < Duration::from_secs(3600));
        }

        let tool = tool_key(&segs);
        let danger = segs.iter().find_map(danger_kind);
        let failed = ev.exit != 0 && ev.exit != 130;

        let sess = st
            .sessions
            .entry(ev.session.clone())
            .or_insert_with(Session::new);
        sess.last_seen = Instant::now();
        sess.gap = sess.gap.saturating_add(1);
        let repeat = failed && sess.fail_streak > 0 && sess.last_cmd == shown;
        if failed {
            sess.fail_streak += 1;
        } else if ev.exit == 0 {
            sess.fail_streak = 0;
        }
        sess.last_cmd = shown.clone();
        let streak = sess.fail_streak;
        let gap = sess.gap;

        let moment = classify(ev.exit, ev.secs, streak, repeat, tool, danger);

        st.history.push_back(Entry {
            cmd: shown.clone(),
            exit: ev.exit,
        });
        while st.history.len() > 10 {
            st.history.pop_front();
        }

        let pct = (base_chance(&moment) * sh.cfg.chattiness / 100).min(100);
        let speak = (is_strong(&moment) || gap >= 3) && st.rng.chance(pct);
        if !speak {
            return None;
        }
        if let Some(s) = st.sessions.get_mut(&ev.session) {
            s.gap = 0;
        }

        let lines = bank(&moment);
        let fallback = pick_line(&mut st.rng, &lines, &st.last_line);
        st.last_line = fallback.clone();
        let examples = sample(&mut st.rng, &lines, 3);
        Plan {
            moment,
            fallback,
            examples,
            ctx: history_text(&st.history),
            cmd: shown,
            dir,
        }
    };

    // The lock is released here: the LLM call must never hold it.
    let mut text: Option<String> = None;
    if let Some(model) = sh.cfg.model.as_deref() {
        // One generation at a time; if it's busy the cat just uses its own voice.
        if sh
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let r = llm(
                &sh.cfg,
                model,
                &system_prompt(&sh.cfg, false),
                &event_prompt(&plan),
                64,
                sh.cfg.deadline,
            );
            sh.busy.store(false, Ordering::SeqCst);
            sh.note_llm(&r);
            if let Ok(raw) = r {
                text = sanitize(&raw, 120, true);
            }
        }
    }
    let line = text.unwrap_or_else(|| plan.fallback.clone());
    Some(format!("{}\t{}\n", eyes(&plan.moment), line))
}

fn on_chat(sh: &Shared, req: &str) -> String {
    let f = wire_fields(req);
    let msg = f.get("msg").cloned().unwrap_or_default();
    let msg = msg.trim();
    if msg.is_empty() {
        return String::new();
    }
    let ctx = {
        let g = sh.lock();
        history_text(&g.history)
    };
    let mut text: Option<String> = None;
    if let Some(model) = sh.cfg.model.as_deref() {
        let mut prompt = String::new();
        if !ctx.is_empty() {
            prompt.push_str(&format!(
                "Recent commands the user ran (oldest first): {ctx}\n\n"
            ));
        }
        prompt.push_str(&format!(
            "The user says to you: {msg}\nReply in character, at most two short sentences. Output only your reply."
        ));
        let r = llm(
            &sh.cfg,
            model,
            &system_prompt(&sh.cfg, true),
            &prompt,
            150,
            sh.cfg.chat_timeout,
        );
        sh.note_llm(&r);
        if let Ok(raw) = r {
            text = sanitize(&raw, 280, false);
        }
    }
    let text = match text {
        Some(t) => t,
        None => {
            let mut g = sh.lock();
            chat_fallback(msg, &mut g.rng)
        }
    };

    let face = {
        let mut g = sh.lock();
        eyes_face(random_face(&mut g.rng))
    };
    format!("{face}\t{text}\n")
    // format!("^ﻌ^\t{text}\n")
}

// ───────────────────────────── daemon plumbing ─────────────────────────────

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

fn runtime_dir() -> PathBuf {
    if let Some(d) = env_string("CATTY_DIR") {
        return PathBuf::from(d);
    }
    if let Some(x) = env_string("XDG_RUNTIME_DIR") {
        return PathBuf::from(x).join("catty");
    }
    let uid = unsafe { getuid() };
    env::temp_dir().join(format!("catty-{uid}"))
}

/// Private per-user directory. Refuses a directory that is a symlink or belongs to someone else.
fn prepare_dir() -> io::Result<PathBuf> {
    let dir = runtime_dir();
    let uid = unsafe { getuid() };
    if let Err(e) = DirBuilder::new().recursive(true).mode(0o700).create(&dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    let md = fs::symlink_metadata(&dir)?;
    if !md.is_dir() || md.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a directory owned by you", dir.display()),
        ));
    }
    if md.mode() & 0o077 != 0 {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn run_daemon() -> io::Result<()> {
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

    let shared = Arc::new(Shared::new(Config::from_env(), sock));
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

// ───────────────────────────── client side ─────────────────────────────

fn request(sock: &Path, payload: &str, timeout: Duration) -> io::Result<String> {
    let mut s = UnixStream::connect(sock)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(payload.as_bytes())?;
    // Without this the server's read_to_string never sees EOF and both sides hang.
    s.shutdown(Shutdown::Write)?;
    let mut out = String::new();
    s.take(MAX_MSG).read_to_string(&mut out)?;
    Ok(out)
}

fn ping(sock: &Path) -> bool {
    matches!(request(sock, "ping\n", Duration::from_millis(300)), Ok(r) if r.trim() == "pong")
}

fn ensure_daemon() -> io::Result<()> {
    let sock = prepare_dir()?.join(SOCK_NAME);
    if ping(&sock) {
        return Ok(());
    }
    let mut cmd = Command::new(env::current_exe()?);
    cmd.arg("__daemon")
        .env_remove("CATTY_CMD")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // New session: closing the terminal that happened to start us must not kill the daemon.
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(10));
        if ping(&sock) {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "daemon did not start",
    ))
}

fn send(payload: &str, timeout: Duration) -> io::Result<String> {
    let sock = prepare_dir()?.join(SOCK_NAME);
    match request(&sock, payload, timeout) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            ensure_daemon()?;
            request(&sock, payload, timeout)
        }
        other => other,
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        let cl = cur.chars().count();
        if cl > 0 && cl + 1 + w.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn render(eyes: &str, text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let lines = wrap(&clean, 46);
    let mut it = lines.iter();
    let mut out = String::from("\n /\\_/\\\n");
    out.push_str(
        format!(
            "( {eyes} )  {}",
            it.next().map(String::as_str).unwrap_or("")
        )
        .trim_end(),
    );
    out.push('\n');
    out.push_str(format!("  > <   {}", it.next().map(String::as_str).unwrap_or("")).trim_end());
    out.push('\n');
    for l in it {
        out.push_str(&format!("         {l}\n"));
    }
    out.push('\n');
    out
}

fn print_reply(reply: &str) {
    let reply = reply.trim_end();
    let Some((eyes, text)) = reply.split_once('\t') else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let eyes: String = eyes.chars().filter(|c| !c.is_control()).take(3).collect();
    let block = render(&eyes, text);
    let dim = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let mut out = io::stdout();
    if dim {
        let _ = write!(out, "\x1b[2m{block}\x1b[0m");
    } else {
        let _ = write!(out, "{block}");
    }
    let _ = out.flush();
}

fn event_cli(a: &[String]) -> io::Result<()> {
    // args: <session> <exit> <secs> <cwd>; the command travels in $CATTY_CMD, not argv,
    // so it doesn't show up in `ps` for other users.
    if a.len() < 4 {
        return Ok(());
    }
    let cmd = env::var("CATTY_CMD").unwrap_or_default();
    if cmd.trim().is_empty() {
        return Ok(());
    }
    let ev = Event {
        session: a[0].clone(),
        exit: a[1].parse().unwrap_or(0),
        secs: a[2].parse().unwrap_or(0),
        cwd: a[3].clone(),
        cmd,
    };
    let timeout = Duration::from_millis(env_u64("CATTY_DEADLINE_MS", 700) + 400);
    let reply = send(&ev.to_wire(), timeout)?;
    print_reply(&reply);
    Ok(())
}

fn chat_cli(msg: &str) -> io::Result<()> {
    let timeout = Duration::from_millis(env_u64("CATTY_CHAT_TIMEOUT_MS", 20_000) + 2_000);
    let reply = send(&format!("chat\nmsg={}\n", escape(msg)), timeout)?;
    print_reply(&reply);
    Ok(())
}

fn stop_cli() -> io::Result<()> {
    let sock = prepare_dir()?.join(SOCK_NAME);
    match request(&sock, "quit\n", Duration::from_secs(1)) {
        Ok(_) => println!("catty: asleep"),
        Err(_) => println!("catty: not running"),
    }
    Ok(())
}

fn status_cli() -> io::Result<()> {
    let sock = prepare_dir()?.join(SOCK_NAME);
    match request(&sock, "status\n", Duration::from_secs(1)) {
        Ok(r) if !r.is_empty() => {
            for line in r.lines() {
                println!("catty: {line}");
            }
        }
        _ => println!("catty: not running"),
    }
    Ok(())
}

// ───────────────────────────── shell integration ─────────────────────────────

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const ZSH_INIT: &str = r##"# catty — generated by `catty init zsh`
catty() { @EXE@ "$@"; }
__catty_preexec() { __catty_cmd=$1; __catty_t0=$SECONDS; }
__catty_precmd() {
  local ec=$?
  if [[ -n ${__catty_cmd+x} ]]; then
    local secs=$(( SECONDS - __catty_t0 ))
    CATTY_CMD=$__catty_cmd @EXE@ __event "$$" "$ec" "$secs" "$PWD" 2>/dev/null
    unset __catty_cmd
  fi
  return $ec
}
autoload -Uz add-zsh-hook
add-zsh-hook preexec __catty_preexec
add-zsh-hook precmd __catty_precmd
"##;

// bash has no preexec; the command comes from `history 1`, the start time from PS0 (bash >= 4.4).
const BASH_INIT: &str = r##"# catty — generated by `catty init bash`
catty() { @EXE@ "$@"; }
__catty_hist() {
  local h
  h=$(HISTTIMEFORMAT= history 1)
  h=${h#"${h%%[![:space:]]*}"}
  __catty_hn=${h%%[[:space:]]*}
  h=${h#"$__catty_hn"}
  __catty_h=${h#"${h%%[![:space:]]*}"}
}
__catty_precmd() {
  local ec=$? prev=$__catty_hn secs=0
  __catty_hist
  if [[ -n $__catty_hn && $__catty_hn != "$prev" ]]; then
    (( __catty_t0 >= 0 )) && secs=$(( SECONDS - __catty_t0 ))
    CATTY_CMD=$__catty_h @EXE@ __event "$$" "$ec" "$secs" "$PWD" 2>/dev/null
  fi
  __catty_t0=-1
  return $ec
}
__catty_hist
__catty_t0=-1   # must be set (not unset), or bash skips evaluating the PS0 expression
PS0=${PS0}'${__catty_t0:0:$((__catty_t0=SECONDS,0))}'
[[ ${PROMPT_COMMAND[*]} == *__catty_precmd* ]] || PROMPT_COMMAND="__catty_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
"##;

fn init_cli(template: &str) -> io::Result<()> {
    let exe = env::current_exe()?.display().to_string();
    print!("{}", template.replace("@EXE@", &shq(&exe)));
    Ok(())
}

fn usage() {
    println!(
        r#"catty - a tiny terminal cat

  catty <anything>      talk to the cat
  catty pet             pet the cat
  catty start|stop|restart|status
  catty init zsh|bash   print the shell hook

  eval "$(catty init zsh)"      # in ~/.zshrc
  eval "$(catty init bash)"     # in ~/.bashrc

env: CATTY_MODEL CATTY_HOST CATTY_DEADLINE_MS CATTY_CHAT_TIMEOUT_MS
     CATTY_CHATTINESS CATTY_PERSONA CATTY_DIR   (see the header of catty.rs)
Start a command with a space and the cat won't see it."#
    );
}

fn run(args: &[String]) -> io::Result<()> {
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            usage();
            Ok(())
        }
        Some("init") => match args.get(1).map(String::as_str) {
            Some("zsh") => init_cli(ZSH_INIT),
            Some("bash") => init_cli(BASH_INIT),
            _ => {
                eprintln!("usage: catty init zsh|bash");
                Ok(())
            }
        },
        Some("start") => {
            ensure_daemon()?;
            println!("catty: awake");
            Ok(())
        }
        Some("stop") => stop_cli(),
        Some("restart") => {
            stop_cli()?;
            thread::sleep(Duration::from_millis(150));
            ensure_daemon()?;
            println!("catty: awake");
            Ok(())
        }
        Some("status") => status_cli(),
        Some("pet") => chat_cli("*pets you*"),
        Some("__daemon") => run_daemon(),
        Some("__event") => {
            let _ = event_cli(&args[1..]); // a prompt hook must never fail loudly
            Ok(())
        }
        Some(_) => chat_cli(&args.join(" ")),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("catty: {e}");
        std::process::exit(1);
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_think_quotes_and_controls() {
        let raw = "<think>hmm</think>\n\"Mrrp. nice.\x1b[2K\"\nsecond line";
        assert_eq!(sanitize(raw, 120, true).as_deref(), Some("Mrrp. nice."));
        assert_eq!(sanitize("<think>unterminated", 120, true), None);
    }
}
