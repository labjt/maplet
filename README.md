# maplet

An omarchy-native terminal client for [Maple AI](https://trymaple.ai) — private,
end-to-end-encrypted AI chat and **agent mode**, in your terminal, themed by your
active omarchy theme. Use it full-screen (`maplet`) or from the command line to
work in a directory (`maplet run "…"`).

![maplet in chat mode](docs/chat.png)

Chat streams markdown with syntax-highlighted code, and takes its colours from
whichever omarchy theme is active (Tokyo Night above).

![maplet in agent mode](docs/agent.png)

Agent mode runs [goose](https://github.com/aaif-goose/goose) against the same
encrypted enclave, showing its thinking, collapsible tool calls, and permission
prompts inline.

```
maplet (one binary)
 ├─ embedded maple-proxy ── attestation + E2EE ──▶ enclave.trymaple.ai (AWS Nitro TEE)
 ├─ chat mode ── OpenAI-compat SSE ──▶ embedded proxy
 └─ agent mode ── ACP ──▶ goose subprocess ── OpenAI-compat ──▶ embedded proxy
```

All inference stays end-to-end encrypted to Maple's enclave; the embedded
[maple-proxy](https://github.com/opensecretcloud/maple-proxy) handles attestation and
crypto in-process (loopback only, ephemeral port). Agent mode drives
[goose](https://github.com/aaif-goose/goose) — the same engine behind Maple's Agent
Mode — over the [Agent Client Protocol](https://agentclientprotocol.com), so the agent's
model calls are enclave-encrypted too.

## Setup

1. **Build**: `cargo build --release` (binary: `target/release/maplet`)
2. **API key** (Maple Pro/Team/Max, created at trymaple.ai):
   ```sh
   mkdir -p ~/.config/maple-tui
   (umask 077; $EDITOR ~/.config/maple-tui/api_key)   # paste key, save
   ```
   or `export MAPLE_API_KEY=...` (env wins over the file).
3. **goose** (agent mode only):
   ```sh
   curl -fsSL https://github.com/aaif-goose/goose/releases/download/stable/download_cli.sh | CONFIGURE=false bash
   ```

## Working in a directory: `maplet run`

The command-line half. Point it at a directory and it reads, edits and runs
things there, with the same encrypted enclave behind it.

```console
$ cd ~/code/project
$ maplet run "add a --verbose flag and mention it in the README"
· working in /home/you/code/project
· model glm-5-3
~ thinking…
› shell · cat src/main.rs
? edit · src/main.rs
  a allow always  ·  y allow once  ·  n reject
allow? [Y/a/n] y
› edit · src/main.rs
Added a `--verbose` flag and documented it under Usage.
```

Run it with no prompt for an interactive session in the current directory,
`Ctrl+D` to leave:

```console
$ maplet run
· ready — type a request, Ctrl+D to leave

you › how many lines in list.txt?
› shell · wc -l < list.txt
2 lines.

you › add a third line: gamma
```

**It composes.** Answers go to stdout; tool calls, thinking and prompts go to
stderr, so redirecting captures just the answer:

```sh
maplet run -q "summarise src/" > notes.md      # answer only
cat build.log | maplet run "why did this fail?" # piped stdin as context
maplet run --json --yes "run the tests" | jq -r 'select(.type=="tool").title'
```

**It asks first.** maplet runs goose in approve mode, so every tool use needs a
`y` (once), `a` (always, rest of session) or `n`. `--yes` approves everything
and is *required* when there is no terminal to ask on — without a terminal and
without `--yes`, tool use is refused rather than silently allowed. (goose's own
default, `smart_approve`, will happily approve a shell `rm`; maplet overrides
it.)

| Flag | Meaning |
|---|---|
| `-C, --cwd <DIR>` | work somewhere other than the current directory |
| `-m, --model <ID>` | override the model (`maplet models` lists them) |
| `-y, --yes` | approve tool use without asking |
| `-q, --quiet` | print only the answer |
| `--think` | stream the model's reasoning |
| `--json` | JSON Lines of every event, for scripting |

Exit status is `0` on success, `1` on error, `2` if the turn was cancelled.

## The TUI

`maplet` with no arguments. Also `maplet repl` (plain chat REPL) and
`maplet models`.

### Commands

Type `/` for a filtered completion list; `Tab` completes, `Enter` runs the
highlighted command, `Esc` dismisses.

![slash commands](docs/commands.png)

| Command | Action |
|---|---|
| `/help` | show keys and commands |
| `/model [id]` | pick a model, or set one by id |
| `/new` (`/clear`) | start a new session |
| `/sessions` (`/resume`) | reopen a saved session |
| `/chat` · `/agent` | switch mode |
| `/quit` (`/exit`, `/q`) | leave maplet |

### Keys

| Key | Action |
|---|---|
| `Enter` / `Alt+Enter` | send / newline |
| `Esc` | cancel turn · close modal |
| `Ctrl+T` | toggle chat ↔ agent mode |
| `Ctrl+O` / `Ctrl+P` / `Ctrl+N` | model picker / session picker / new session |
| `Tab` | focus tool cards; again to expand raw input/output |
| `y a n N` | permission prompt: allow once / always / reject once / always |
| `PgUp/PgDn` · `End` | scroll · follow live output |
| `?` | help |

## Config (`~/.config/maple-tui/config.toml`, all optional)

```toml
[chat]
model = "glm-5-3"

[agent]
model = "glm-5-3"        # needs tool calling; `maplet models` lists what you have
cwd = "~"                # agent session working directory
builtins = ["developer"] # goose builtin extensions (shell + file tools)

[[agent.mcp_servers]]    # injected into the agent session via ACP
name = "time"
transport = "stdio"      # or "http" (streamable HTTP; SSE unsupported)
command = "uvx"
args = ["mcp-server-time"]

[proxy]
port = 0                 # 0 = ephemeral; avoids Maple Desktop's 8080
pcr0_environment = "production"
```

Goose's own `~/.config/goose/config.yaml` extensions also load (goose wins on
name collisions).

## Notes

- Theming reads `~/.local/state/omarchy/current/theme/colors.toml` live (switch
  themes and maplet restyles); falls back to ANSI-16 off-omarchy.
- Maple retires model ids from time to time; if a request starts failing, run
  `maplet models` and update the config. `maplet run` checks up front and names
  the alternatives.
- Chat sessions persist locally under `~/.local/share/maple-tui/sessions/`.
  Agent sessions are persisted by goose itself (resume is on the roadmap).
- Logs: `~/.local/state/maple-tui/maplet.log.*` (`RUST_LOG=debug` for goose ACP
  traffic).
- The Maple API key is sent only to the embedded loopback proxy, which talks
  only to the attested enclave.

MIT, like Maple itself.
