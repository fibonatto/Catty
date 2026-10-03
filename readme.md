# Catty

Catty is a terminal companion that operates as a shell hook and background daemon, evaluating command events and providing contextual responses via an optional local LLM integration (Ollama).

![catty](print.png)

## Architecture

The system consists of two main components:

* **Shell Hook**: Injected into `zsh` or `bash` initialization. On each prompt, it captures the exit code, execution duration, current working directory, and command text, then sends this data via Unix domain socket to the daemon.

* **Background Daemon**: Runs as a persistent process listening on a private Unix socket (`catty.sock`). It receives events, applies decision logic based on cooldowns and probability thresholds, and optionally queries an LLM for generated responses. The daemon is non-blocking to the terminal prompt.

### Command Classification

Commands are analyzed and mapped to tool categories via string matching on the binary name and arguments. Supported tools include:
- **Git operations**: `git-commit`, `git-push`, `git-pull`, `git-stash`, `git-branch`, `git-merge`
- **Build systems**: `make`, `cargo`, `npm`, `pnpm`, `yarn`, `gradle`, `mvn`, `gcc`, `clang`, `rustc`, `go`, `zig`
- **Editors**: `vim`, `nvim`, `vi`, `nano`, `emacs`, `hx`
- **Containers**: `docker`, `podman`, `kubectl`
- **Search**: `grep`, `rg`, `find`, `fd`
- **Monitoring**: `top`, `htop`, `btop`
- **Other**: `ssh`, `cat`, `sleep`, `clear`, `man`, `tldr`, `ping`, `curl`, `wget`, `ollama`, `rm`, `sudo`

### Danger Detection

The daemon identifies high-risk commands:
- `rm -rf` (recursive + force)
- `rm --no-preserve-root`
- `git push --force` / `git push -f` / `git push --force-with-lease`
- `git reset --hard`
- `dd of=/dev/...` (writing to block device)
- Commands starting with `mkfs*`

### Response Heuristics

Response probability is determined by moment classification:

| Moment | Base Chance | Notes |
|--------|-------------|-------|
| Danger (detected) | 85% | Can bypass cooldown if strong |
| Failed command (3+ streak) | 85% | Strong enough to bypass cooldown |
| Failed command (repeated) | 70% | Same command failed before |
| Failed command (first failure) | 45% | Generic failure |
| Command not found | 60% | Exit code 127 |
| Slow execution (≥20s) | 60-100% | Scales with duration; 100% at ≥300s |
| Slow execution (≥300s) | 100% | Strong (bypasses cooldown) |
| Tool-specific reaction | 25% | Generic for recognized tools |
| Interrupted (Ctrl-C) | 20% | Exit code 130 |
| Plain command (exit 0) | 4% | No special context |

The daemon maintains a cooldown timer to avoid excessive responses. Strong moments (detected dangers, 3+ failure streaks, ≥120s execution) can override the cooldown.

### Redaction

Before sending a command to the LLM, the daemon masks environment variables and arguments that contain:
- `token`, `secret`, `pass`, `key`, `auth`, `cred` (case-insensitive)
- Explicit flags: `--password`, `--passwd`, `--token`, `--secret`, `--api-key`, `-password`

Values are replaced with `***`. The redacted command is limited to 200 characters.

### Voice

Built-in static responses are stored as template strings keyed by moment type. Tool-specific commentary is provided for 22 command categories. Failure messages vary by exit code (e.g., code 139 = segfault, code 137/143 = killed).

## Project Structure

```
src/
├── main.rs          # Daemon setup, event loop, socket management, LLM integration
├── command.rs       # Command parsing, tool classification, danger detection
├── moment.rs        # Moment classification and response probability logic
├── voice/
│   ├── lines.rs     # Static response text, indexed by moment
│   └── mod.rs       # Voice module export
├── redact.rs        # Secret masking for commands
├── wire.rs          # Unix socket message format (key=value pairs)
├── json.rs          # Minimal JSON string extraction for Ollama responses
└── util.rs          # RNG (splitmix64), time, string helpers
```

## Build

**Rust Edition 2021**, no external dependencies. Compile with optimization:

```bash
cargo build --release
```

Or via `rustc`:

```bash
rustc --edition 2021 -O src/main.rs -o catty
```

## Installation

Initialize the shell hook in your shell configuration file:

```bash
eval "$(./catty init zsh)"   # ~/.zshrc
# or
eval "$(./catty init bash)"  # ~/.bashrc
```

## CLI Commands

* `catty <text>` — Send a direct message to the daemon for LLM processing.
* `catty pet` — Trigger a fixed response.
* `catty start` — Explicitly start the background daemon.
* `catty stop` — Terminate the running daemon.
* `catty restart` — Restart the daemon (applies configuration changes).
* `catty status` — Print daemon PID, configuration state, and LLM success/failure counters.

## Configuration

Environment variables (read on daemon startup; use `catty restart` to apply changes):

* `CATTY_MODEL` — Ollama model name. If unset, daemon uses only built-in voice.
* `CATTY_HOST` — Ollama server address (default: `127.0.0.1:11434`).
* `CATTY_DEADLINE_MS` — Max latency for prompt reactions (default: `700`).
* `CATTY_CHAT_TIMEOUT_MS` — Max latency for `catty <text>` interactions (default: `20000`).
* `CATTY_CHATTINESS` — Response frequency multiplier, 0–200 (default: `100`).
* `CATTY_PERSONA` — Extra personality traits appended to LLM system prompt.
* `CATTY_DIR` — Runtime directory override for socket and lock file (default: `$XDG_RUNTIME_DIR/catty` or temp dir).

## Implementation Notes

* **No external crates**: All I/O, JSON parsing, and utility logic are implemented in the standard library.
* **Synchronous I/O**: Ollama requests use blocking TCP with strict timeouts instead of async runtime.
* **Monolithic daemon**: All socket management, HTTP/1.0 construction, and message handling run in a single binary.
* **Unix-only**: Targets Unix systems; shell hook support is limited to `zsh` and `bash`.
* **Custom RNG**: Uses splitmix64 for probability sampling (more stable than simple modulo on coarse clocks).
* **Interactive tool detection**: Editors, SSH, `top`, and `man` are flagged as interactive; slow execution tracking ignores them.

