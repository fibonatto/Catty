//! Mask obvious secrets in a command line before it goes anywhere near a prompt.

/// Mask the obvious secrets before a command goes anywhere near a prompt.
fn is_secretish(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["token", "secret", "pass", "key", "auth", "cred"]
        .iter()
        .any(|w| k.contains(w))
}

pub(crate) fn redact(cmd: &str) -> String {
    let mut hide_next = false;
    let mut out: Vec<String> = Vec::new();
    for w in cmd.split_whitespace().take(40) {
        if hide_next {
            out.push("***".to_string());
            hide_next = false;
            continue;
        }
        let lw = w.to_ascii_lowercase();
        if matches!(
            lw.as_str(),
            "--password" | "--passwd" | "--token" | "--secret" | "--api-key" | "-password"
        ) {
            hide_next = true;
            out.push(w.to_string());
            continue;
        }
        match w.split_once('=') {
            Some((k, _)) if is_secretish(k) => out.push(format!("{k}=***")),
            _ => out.push(w.to_string()),
        }
    }
    out.join(" ").chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction() {
        assert_eq!(
            redact("curl --token abc API_KEY=zzz ls"),
            "curl --token *** API_KEY=*** ls"
        );
    }
}
