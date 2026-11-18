# Marsad

Marsad is a dashboard for coding-agent CLI sessions (Claude Code, Codex, OpenCode, Kilo Code)
on the local machine and on remote machines over SSH or WSL: launch, continue, watch and steer
many sessions, answer their permission/question gates, review their git changes, open a
terminal next to them — and wire sessions into **workstreams** across machines, where agents
delegate to each other through [Wasl](https://github.com/mewais/Wasl) with human-approved
handoffs.

The backend is built on [Wasit](https://github.com/mewais/Wasit) (driver + session runtime +
control protocol). See [ARCHITECTURE.md](ARCHITECTURE.md).

## Layout

* `crates/marsad-proto` — wire types and the effective-config merge (`ts` feature → TypeScript)
* `crates/marsad-store` — hub persistence
* `crates/marsad-agent` — the daemon binary (`marsad-agent`)
* `crates/marsad-client` — typed client used by the desktop shell
* `apps/desktop` — Tauri + React desktop app (built separately; being rebuilt)

## Build

Wasit and Wasl are cargo git dependencies (branch `claude/awesome-euler-r7x45r`; Wasl depends
on the same Wasit source, so one copy is resolved). To develop against sibling checkouts
(`../Wasit`, `../Wasl`), create an untracked `.cargo/config.toml`:

```toml
[patch."https://github.com/mewais/Wasit"]
wasit-core = { path = "../Wasit/crates/wasit-core" }
wasit-control = { path = "../Wasit/crates/wasit-control" }
wasit-runtime = { path = "../Wasit/crates/wasit-runtime" }
wasit-shim = { path = "../Wasit/crates/wasit-shim" }
wasit-fake-cli = { path = "../Wasit/crates/wasit-fake-cli" }

[patch."https://github.com/mewais/Wasl"]
wasl-proto = { path = "../Wasl/crates/wasl-proto" }
wasl-node = { path = "../Wasl/crates/wasl-node" }
wasl-mcp = { path = "../Wasl/crates/wasl-mcp" }
```

```sh
cargo build --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

TypeScript definitions for the GUI (every Marsad, Wasl and Wasit wire type):

```sh
cargo run -p marsad-proto --features ts --bin marsad-ts-export -- apps/desktop/src/bridge/generated
```

## Run

```sh
marsad-agent serve --hub            # the GUI machine's daemon (normally started by the client)
marsad-agent stop                   # sessions keep running in their shims
marsad-agent --version              # {"build":"…","target":"…"}
marsad-agent mcp --socket <daemon socket> --session <id>   # Wasl MCP server (injected by the hub)
```

Data lives in `~/.marsad` (`%LOCALAPPDATA%\Marsad` on Windows). Remote hosts get the same
binary installed into `~/.marsad/bin/` on first connect; binaries for other target triples
come from `$MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]`.

## Tests

All tests run real processes: the daemon binary, Wasit's scriptable fake agent CLI, and on
Linux a real `sshd` started on a free localhost port (install `openssh-server` and
`openssh-client`). Tests that need root (they create a local user with a password to exercise
password and key+password sign-in) are `#[ignore]`d; run them with
`cargo test -p marsad-agent --test remote -- --ignored`. Live tests against the real agent CLIs
live in Wasit (`cargo test -p wasit-runtime --test live_config -- --ignored`); the live
workstream test (claude orchestrating opencode and kilo workers) is
`MARSAD_LIVE=1 cargo test -p marsad-agent --test live_workstream -- --ignored --nocapture`.
