# Marsad — مرصد

**A single control surface for managing many concurrent Claude Code sessions at once** — across the
local machine and remote Linux hosts (over SSH). Launch, monitor, answer permission/plan/edit gates,
prompt, review diffs & commit, configure, and revisit sessions — all without opening a terminal.

Marsad (Arabic *مرصد*, "observatory") removes terminal-tab sprawl. There are two clients:

- **Desktop** (primary): native Linux (also Windows/macOS), built with **Tauri** (Rust core + web
  UI). Runs **local and remote** sessions.
- **Mobile companion** (iOS + Android, **fully native**): SSH-only — monitor, answer gates, and
  prompt on the go. Cannot run local sessions.

## Status

Greenfield. This repository is currently **scaffolding**: an architecture decomposed into isolated
blocks (Rust crates + native app packages), each with a separate, pre-written test suite. See
[`ARCHITECTURE.md`](./ARCHITECTURE.md) for the full block map, interface contracts, and the
test-first build regime.

## How it works (architecture in one paragraph)

Almost all logic lives in a **shared Rust core** (`crates/marsad-*`): the Claude Code stream-json
protocol, the session state machine, gates, diffing, persistence, host management, transport,
lifecycle (reconnect-or-resume), notifications, and the dashboard view-models. The desktop wraps the
core via **Tauri**; the phones bind it via **UniFFI** (generated Swift + Kotlin), keeping native code
to UI + thin platform glue. A detached supervisor, **`marsad-agent`**, owns every `claude` process so
**sessions outlive the GUI** — quitting or crashing the app never kills a running session. Every
session is wrapped in **tmux** (Linux/macOS) or **ConPTY** (Windows) for resilience across SSH drops
and sleep, and is re-adopted or `--resume`d on relaunch.

Marsad never parses rendered terminal output. It drives Claude Code only through official, structured
surfaces: `claude --print --input-format stream-json --output-format stream-json --verbose`, plus
hooks and the permission/approval control protocol.

## Repository layout

```
crates/            # the shared Rust core, one crate per block (B01–B13)
apps/desktop/      # Tauri app: src-tauri (Rust bridge) + src (React/TS UI)
apps/ios/          # native SwiftUI app over the UniFFI core
apps/android/      # native Jetpack Compose app over the UniFFI core
tests/integration/ # cross-block integration scenarios (i01–i08)
docs/specs/        # full long-form spec per block
fixtures/          # recorded real claude stream-json + hook payloads
```

## Building

The Rust workspace builds with a stable toolchain:

```sh
cargo build --workspace
cargo test  --workspace        # block + integration suites
cargo llvm-cov --workspace --fail-under-lines 100 --fail-under-regions 100
```

Desktop UI:

```sh
pnpm --dir apps/desktop install
pnpm --dir apps/desktop test   # Vitest
cargo tauri dev                # run the desktop app
```

Phones are built on their native toolchains (Xcode / Gradle). See each app's `AGENTS.md`.

## License

MIT © 2026 Mohammad Ewais
