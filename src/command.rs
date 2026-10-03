//! Shell command analysis: split a line into segments, spot dangerous commands,
//! and map a command to a coarse "tool" kind. Pure, no I/O.

use crate::util::basename;

pub(crate) struct Seg {
    pub(crate) prog: String,
    args: Vec<String>,
    sudo: bool,
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((k, _)) => {
            !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !k.chars().next().map_or(false, |c| c.is_ascii_digit())
        }
        None => false,
    }
}

fn parse_seg(part: &str) -> Option<Seg> {
    let mut sudo = false;
    let mut words = part.split_whitespace();
    let mut prog: Option<String> = None;
    while let Some(w) = words.next() {
        if is_assignment(w) {
            continue;
        }
        match w {
            "sudo" | "doas" => {
                sudo = true;
                continue;
            }
            "time" | "nohup" | "command" | "env" | "exec" | "nice" | "builtin" => continue,
            _ => {}
        }
        if sudo && w.starts_with('-') {
            continue;
        }
        prog = Some(basename(w));
        break;
    }
    let prog = prog?;
    let args: Vec<String> = words.map(|s| s.to_string()).collect();
    Some(Seg { prog, args, sudo })
}

pub(crate) fn parse_segments(cmd: &str) -> Vec<Seg> {
    cmd.split(|c: char| c == ';' || c == '|' || c == '&')
        .filter_map(parse_seg)
        .collect()
}

fn first_sub(s: &Seg) -> &str {
    s.args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("")
}

pub(crate) fn danger_kind(s: &Seg) -> Option<&'static str> {
    let flags: String = s
        .args
        .iter()
        .filter(|a| a.starts_with('-') && !a.starts_with("--"))
        .map(|a| &a[1..])
        .collect::<Vec<_>>()
        .join("");
    let has = |name: &str| s.args.iter().any(|a| a == name);
    match s.prog.as_str() {
        "rm" => {
            let rec = flags.contains('r') || flags.contains('R') || has("--recursive");
            let force = flags.contains('f') || has("--force");
            if (rec && force) || has("--no-preserve-root") {
                Some("rm")
            } else {
                None
            }
        }
        "git" => match first_sub(s) {
            "push" if has("--force") || has("-f") || has("--force-with-lease") => {
                Some("force-push")
            }
            "reset" if has("--hard") => Some("reset-hard"),
            _ => None,
        },
        "dd" => {
            if s.args.iter().any(|a| a.starts_with("of=/dev/")) {
                Some("dd")
            } else {
                None
            }
        }
        p if p.starts_with("mkfs") => Some("mkfs"),
        _ => None,
    }
}

pub(crate) fn tool_key(segs: &[Seg]) -> Option<&'static str> {
    let s = segs.first()?;
    let k = match s.prog.as_str() {
        "git" => match first_sub(s) {
            "commit" => "git-commit",
            "push" => "git-push",
            "pull" | "fetch" => "git-pull",
            "stash" => "git-stash",
            "checkout" | "switch" => "git-branch",
            "merge" | "rebase" => "git-merge",
            _ => "git",
        },
        "vim" | "nvim" | "vi" | "nano" | "emacs" | "hx" => "editor",
        "ssh" => "ssh",
        "cat" => "cat",
        "sleep" => "sleep",
        "clear" => "clear",
        "man" | "tldr" => "man",
        "top" | "htop" | "btop" => "top",
        "make" | "cmake" | "cargo" | "gcc" | "clang" | "rustc" | "go" | "zig" | "npm" | "pnpm"
        | "yarn" | "gradle" | "mvn" => "build",
        "docker" | "podman" | "kubectl" => "container",
        "ping" => "ping",
        "curl" | "wget" => "net",
        "ollama" => "ollama",
        "grep" | "rg" | "find" | "fd" => "search",
        "rm" => "rm",
        _ => {
            if s.sudo {
                "sudo"
            } else {
                return None;
            }
        }
    };
    Some(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_detection() {
        let d = |c: &str| parse_segments(c).iter().find_map(danger_kind);
        assert_eq!(d("sudo rm -rf /tmp/x"), Some("rm"));
        assert_eq!(d("cd x && rm -fr build"), Some("rm"));
        assert_eq!(d("git push --force origin main"), Some("force-push"));
        assert_eq!(d("git reset --hard HEAD~1"), Some("reset-hard"));
        assert_eq!(d("rm file.txt"), None);
        assert_eq!(d("git push origin main"), None);
    }

    fn key(c: &str) -> Option<&'static str> {
        tool_key(&parse_segments(c))
    }

    // Characterization: pins what `tool_key` does today, so the move to an enum is checkable.
    #[test]
    fn tool_key_table() {
        let cases: &[(&str, Option<&str>)] = &[
            ("git commit -m x", Some("git-commit")),
            ("git push origin main", Some("git-push")),
            ("git pull", Some("git-pull")),
            ("git fetch --all", Some("git-pull")),
            ("git stash pop", Some("git-stash")),
            ("git checkout main", Some("git-branch")),
            ("git switch -c x", Some("git-branch")),
            ("git merge dev", Some("git-merge")),
            ("git rebase main", Some("git-merge")),
            ("git status", Some("git")),
            ("git", Some("git")),
            ("vim a.rs", Some("editor")),
            ("nvim a.rs", Some("editor")),
            ("vi a.rs", Some("editor")),
            ("nano a.rs", Some("editor")),
            ("emacs a.rs", Some("editor")),
            ("hx a.rs", Some("editor")),
            ("ssh host", Some("ssh")),
            ("cat f", Some("cat")),
            ("sleep 1", Some("sleep")),
            ("clear", Some("clear")),
            ("man ls", Some("man")),
            ("tldr ls", Some("man")),
            ("top", Some("top")),
            ("htop", Some("top")),
            ("btop", Some("top")),
            ("make", Some("build")),
            ("cargo build", Some("build")),
            ("npm test", Some("build")),
            ("docker ps", Some("container")),
            ("kubectl get pods", Some("container")),
            ("ping x", Some("ping")),
            ("curl x", Some("net")),
            ("wget x", Some("net")),
            ("ollama list", Some("ollama")),
            ("grep x f", Some("search")),
            ("rg x", Some("search")),
            ("find .", Some("search")),
            ("fd x", Some("search")),
            ("rm f", Some("rm")),
            ("sudo ls", Some("sudo")),
            ("sudo rm -rf /tmp/x", Some("rm")),
            ("ls", None),
            ("", None),
            // prefixes that parse_seg skips, and path stripping
            ("FOO=1 cargo test", Some("build")),
            ("time cargo build", Some("build")),
            ("/usr/bin/git commit", Some("git-commit")),
            // only the FIRST segment counts
            ("cd x && cargo build", None),
            ("cargo build | tee log", Some("build")),
        ];
        for (cmd, want) in cases {
            assert_eq!(key(cmd), *want, "command: {cmd:?}");
        }
    }
}
