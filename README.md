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
* `apps/desktop` — the desktop app: React UI (`src/`), a thin Tauri v2 shell (`src-tauri/`,
  built separately) and the real-app end-to-end suite (`e2e-real/`)

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

## Desktop app

`apps/desktop/src-tauri` is a thin shell: it starts/connects to the local hub daemon through
`marsad-client` and gives the webview exactly four commands — `rpc(method, params, host?)`,
`rpc_subscribe(topic, host?, from?, channel)` (events with cursors plus daemon link state;
durable resubscription is native), `rpc_unsubscribe(id)`, `connection_state()` (also the
`connection` event). `rpc` methods under `desktop.` are native: `desktop.open {target}`,
`desktop.reveal {target}`, `desktop.pick_folder {title?, default_path?}`. Natively it raises OS
notifications (needs-you / failed / finished / host lost, per the stored preferences; bringing
the app forward afterwards sends the webview `navigate {host, session}`), shows a tray item
with the attention count, keeps one instance and the window state.

```sh
pnpm --dir apps/desktop install && pnpm --dir apps/desktop build
cd apps/desktop/src-tauri && cargo build --features custom-protocol   # serves ../dist
MARSAD_AGENT_BIN=<path to marsad-agent> target/debug/marsad-desktop
```

`MARSAD_DATA` (data dir) and `MARSAD_DAEMON_ARGS` (extra `serve` options) are development
overrides. Debug builds run an end-to-end smoke through the real bridge with
`MARSAD_SMOKE=<report.json> MARSAD_SMOKE_SCENARIO=<fake CLI scenario> MARSAD_SMOKE_CWD=<dir>`
(the daemon pointed at the fake CLI with `MARSAD_DAEMON_ARGS="--driver-bin claude=<wasit-fake-cli>"`):
it subscribes to `fleet`, launches a session, prompts, answers its gate and writes the report.
Packaging: `pnpm --dir apps/desktop bundle` (stages `marsad-agent` as the sidecar).

### End-to-end suite on the real app

`apps/desktop/e2e-real/` drives the REAL desktop app (UI embedded) on the real daemon, shims
and agent CLIs through the DOM over WebDriver (`tauri-driver` + `WebKitWebDriver`), with a temp
data dir. One command:

```sh
cargo install tauri-driver            # once; plus: apt-get install webkit2gtk-driver xvfb dbus
pnpm --dir apps/desktop test:real -- --build      # --build: agent, fake CLI, UI and app first
pnpm --dir apps/desktop test:real -- --real       # + live claude / opencode / kilo (cheap models)
pnpm --dir apps/desktop test:real -- --shots /tmp/shots --only inbox-ready,workstream
```

The default run uses Wasit's fake CLI for all four personas (one scenario, turns picked by the
prompt) and covers: launches, permission / question / plan gates with each driver's own
choices, interrupt, model / mode, slash commands, rename, Changes (diff, discard, commit),
Terminal, resume by id and continue-latest, stop + delete, agent config, a three-node workstream
with handoff gates, a `kill -9` of the daemon mid-turn, and, as root with `sshd` installed, an
ssh host by key (provisioning, remote session, link killed and reconnecting) and one that signs
in with a password (the sign-in dialog, `auth_needed` after a drop, Connect again). Failures
leave a screenshot and the data dir behind.

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
