//! Shell command analysis: split a line into segments, spot dangerous commands,
//! and map a command to a coarse "tool" kind. Pure, no I/O.

use crate::util::basename;

/// A command worth a raised eyebrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Danger {
    Rm,
    ForcePush,
    ResetHard,
    Dd,
    Mkfs,
}

/// Coarse kind of command the cat has opinions about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    GitCommit,
    GitPush,
    GitPull,
    GitStash,
    GitBranch,
    GitMerge,
    Git,
    Editor,
    Ssh,
    Cat,
    Sleep,
    Clear,
    Man,
    Top,
    Build,
    Container,
    Ping,
    Net,
    Ollama,
    Search,
    Rm,
    Sudo,
}

impl Tool {
    /// Stable name; it is sent to the LLM in the event prompt.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tool::GitCommit => "git-commit",
            Tool::GitPush => "git-push",
            Tool::GitPull => "git-pull",
            Tool::GitStash => "git-stash",
            Tool::GitBranch => "git-branch",
            Tool::GitMerge => "git-merge",
            Tool::Git => "git",
            Tool::Editor => "editor",
            Tool::Ssh => "ssh",
            Tool::Cat => "cat",
            Tool::Sleep => "sleep",
            Tool::Clear => "clear",
            Tool::Man => "man",
            Tool::Top => "top",
            Tool::Build => "build",
            Tool::Container => "container",
            Tool::Ping => "ping",
            Tool::Net => "net",
            Tool::Ollama => "ollama",
            Tool::Search => "search",
            Tool::Rm => "rm",
            Tool::Sudo => "sudo",
        }
    }

    /// Commands that run for a long time because the user is busy in them, not waiting on them.
    pub(crate) fn is_interactive(self) -> bool {
        matches!(self, Tool::Editor | Tool::Ssh | Tool::Top | Tool::Man)
    }
}

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

pub(crate) fn danger_kind(s: &Seg) -> Option<Danger> {
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
                Some(Danger::Rm)
            } else {
                None
            }
        }
        "git" => match first_sub(s) {
            "push" if has("--force") || has("-f") || has("--force-with-lease") => {
                Some(Danger::ForcePush)
            }
            "reset" if has("--hard") => Some(Danger::ResetHard),
            _ => None,
        },
        "dd" => {
            if s.args.iter().any(|a| a.starts_with("of=/dev/")) {
                Some(Danger::Dd)
            } else {
                None
            }
        }
        p if p.starts_with("mkfs") => Some(Danger::Mkfs),
        _ => None,
    }
}

pub(crate) fn tool_key(segs: &[Seg]) -> Option<Tool> {
    let s = segs.first()?;
    let k = match s.prog.as_str() {
        "git" => match first_sub(s) {
            "commit" => Tool::GitCommit,
            "push" => Tool::GitPush,
            "pull" | "fetch" => Tool::GitPull,
            "stash" => Tool::GitStash,
            "checkout" | "switch" => Tool::GitBranch,
            "merge" | "rebase" => Tool::GitMerge,
            _ => Tool::Git,
        },
        "vim" | "nvim" | "vi" | "nano" | "emacs" | "hx" => Tool::Editor,
        "ssh" => Tool::Ssh,
        "cat" => Tool::Cat,
        "sleep" => Tool::Sleep,
        "clear" => Tool::Clear,
        "man" | "tldr" => Tool::Man,
        "top" | "htop" | "btop" => Tool::Top,
        "make" | "cmake" | "cargo" | "gcc" | "clang" | "rustc" | "go" | "zig" | "npm" | "pnpm"
        | "yarn" | "gradle" | "mvn" => Tool::Build,
        "docker" | "podman" | "kubectl" => Tool::Container,
        "ping" => Tool::Ping,
        "curl" | "wget" => Tool::Net,
        "ollama" => Tool::Ollama,
        "grep" | "rg" | "find" | "fd" => Tool::Search,
        "rm" => Tool::Rm,
        _ => {
            if s.sudo {
                Tool::Sudo
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
        assert_eq!(d("sudo rm -rf /tmp/x"), Some(Danger::Rm));
        assert_eq!(d("cd x && rm -fr build"), Some(Danger::Rm));
        assert_eq!(d("git push --force origin main"), Some(Danger::ForcePush));
        assert_eq!(d("git reset --hard HEAD~1"), Some(Danger::ResetHard));
        assert_eq!(d("rm file.txt"), None);
        assert_eq!(d("git push origin main"), None);
    }

    fn key(c: &str) -> Option<&'static str> {
        tool_key(&parse_segments(c)).map(Tool::name)
    }

    // Characterization: pins the command -> tool mapping AND the tool names (they go to the LLM).
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
