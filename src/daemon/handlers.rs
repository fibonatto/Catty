//! Request handlers: turn an event or a chat message into the cat's reply.
//! The LLM call never happens while the state lock is held.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::policy::should_speak;
use super::state::{Entry, Session, Shared, State};
use crate::command::{danger_kind, parse_segments, tool_key};
use crate::moment::{classify, Moment};
use crate::redact::redact;
use crate::memory::Obs;
use crate::util::{basename, now_secs, pick_line, sample};
use crate::voice::lines::{bank, fact_lines};
use crate::wire::{wire_fields, Event};
use crate::{chat_fallback, eyes, eyes_face, llm, note, random_face, sanitize, Config};

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
    facts: Vec<String>,
}

fn event_prompt(p: &Plan) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Right now: {}\nCommand: {}\nFolder: {}\n",
        note(&p.moment),
        p.cmd,
        p.dir
    ));
    if !p.facts.is_empty() {
        s.push_str(&format!(
            "What you remember about this user (use it if it fits): {}\n",
            p.facts.join("; ")
        ));
    }
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

pub(crate) fn on_event(sh: &Shared, ev: Event) -> Option<String> {
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

    let offset = if sh.cfg.memory { sh.tz_offset() } else { 0 };
    let plan: Option<Plan> = 'plan: {
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

        let facts = if sh.cfg.memory {
            st.mem.observe(&Obs {
                now: now_secs(),
                offset,
                exit: ev.exit,
                secs: ev.secs,
                streak,
                tool,
                danger,
            })
        } else {
            Vec::new()
        };
        let notable = !facts.is_empty();

        st.history.push_back(Entry {
            cmd: shown.clone(),
            exit: ev.exit,
        });
        while st.history.len() > 10 {
            st.history.pop_front();
        }

        if !should_speak(&moment, gap, sh.cfg.chattiness, notable, &mut st.rng) {
            break 'plan None;
        }
        if let Some(s) = st.sessions.get_mut(&ev.session) {
            s.gap = 0;
        }

        let lines = match facts.first() {
            Some(f) => fact_lines(f),
            None => bank(&moment),
        };
        let fallback = pick_line(&mut st.rng, &lines, &st.last_line);
        st.last_line = fallback.clone();
        let examples = sample(&mut st.rng, &lines, 3);
        Some(Plan {
            moment,
            fallback,
            examples,
            ctx: history_text(&st.history),
            cmd: shown,
            dir,
            facts: facts.iter().take(2).map(|f| f.note()).collect(),
        })
    };

    // Saved on every path, spoken or not: the counters moved either way.
    if sh.cfg.memory {
        sh.save_memory();
    }
    let plan = plan?;


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

pub(crate) fn on_chat(sh: &Shared, req: &str) -> String {
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
