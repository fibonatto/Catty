//! Wire format between the shell hook / CLI and the daemon: line-based `key=value`
//! with escaping, plus the `Event` message.

use std::collections::HashMap;

// Wire format escaping: values never contain a raw newline.
pub(crate) fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(x) => {
                out.push('\\');
                out.push(x);
            }
            None => out.push('\\'),
        }
    }
    out
}

pub(crate) fn wire_fields(body: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in body.lines().skip(1) {
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.to_string(), unescape(v));
        }
    }
    m
}

pub(crate) struct Event {
    pub(crate) session: String,
    pub(crate) cwd: String,
    pub(crate) cmd: String,
    pub(crate) exit: i32,
    pub(crate) secs: u64,
}

impl Event {
    pub(crate) fn to_wire(&self) -> String {
        format!(
            "event\nsession={}\ncwd={}\ncmd={}\nexit={}\nsecs={}\n",
            escape(&self.session),
            escape(&self.cwd),
            escape(&self.cmd),
            self.exit,
            self.secs
        )
    }

    pub(crate) fn from_wire(body: &str) -> Option<Event> {
        let f = wire_fields(body);
        Some(Event {
            session: f.get("session")?.clone(),
            cwd: f.get("cwd").cloned().unwrap_or_default(),
            cmd: f.get("cmd")?.clone(),
            exit: f.get("exit").and_then(|v| v.parse().ok()).unwrap_or(0),
            secs: f.get("secs").and_then(|v| v.parse().ok()).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_roundtrip() {
        let s = "a\\b\nc\rd";
        assert_eq!(unescape(&escape(s)), s);
        assert!(!escape(s).contains('\n'));
    }
}
