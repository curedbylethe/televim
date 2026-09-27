# AGENTS.md

Guidance for AI agents and contributors working on `televim`. It is the source
of truth for versions, editions, lints, and the dependency rules. Where it
describes a file layout, that layout is the current one.

`Cargo.lock` and this file are both tracked. The lockfile is what makes a build
of a binary reproducible, and one deliberate pin in it is load-bearing — see the
`grammers` note under **Dependency Notes**.

## Project Overview

`televim` is a cross-platform Telegram client written in Rust. It displays
**only private user-to-user chats** — no channels, no bots, no groups. The UI is
driven by Vim keybindings (Normal and Insert today; Visual mode is a stub). The
primary constraint is **memory**: the client must stay under 50 MB RSS at idle
after loading 50+ chats.

The project is a portmanteau of "Telegram" and "Vim," signalling its
terminal-first, keyboard-driven identity.

## Project Goals

1. **Performance First:** Operate within a 50 MB RAM ceiling (stretch goal of 20–30 MB) by using a compile-time TUI, an arena allocator, and avoiding dynamic dispatch in hot paths.
2. **Vim-Centric UX:** Provide a modal editing experience (Normal, Insert, Visual) for chat navigation, message composition, and search. Every action must be reachable from the home row.
3. **Focus & Distraction-Free:** Display only user-to-user private chats. No bots, groups, or channels. This scope drastically reduces the state surface and memory footprint.
4. **Architectural Integrity:** Enforce a clean architecture where the Telegram protocol, domain logic, and UI are independent crates. The domain layer must have zero knowledge of `tokio` or `ratatui`.

## Toolchain

- **Rust edition:** `2024`
- **MSRV / pinned toolchain:** `1.98.1`
- **Resolver:** `2` (workspace-wide)

Do not bump `edition` or `rust-version` without an explicit request; they are tied to the toolchain pin in `rust-toolchain.toml`.

## Workspace Layout

```
televim/
├── Cargo.toml                  # Workspace root, [workspace.dependencies]
├── Cargo.lock                  # Tracked: a binary's lockfile is its reproducibility
├── rust-toolchain.toml         # Pin the toolchain (1.98.1)
├── rustfmt.toml                # Formatting rules
├── clippy.toml                 # Linting rules
├── Makefile                    # fmt / lint / boundary / test / ci / build-release
├── AGENTS.md                   # This file
├── .cargo/
│   └── config.toml             # Aliases (`cargo t`, `l`, `f`); no build flags set
├── .github/
│   └── workflows/
│       ├── ci.yml              # Hosted runner: fmt, clippy, boundary, test, release
│       └── desktop.yml         # Self-hosted: keyring round trip, real datacenter
│
└── crates/
    ├── telegram-framework/     # OUR wrapper over grammers
    │   ├── src/                # lib, error, session, client, auth, raw, dialogs,
    │   │                       #   history, messages, search, updates, testing
    │   └── tests/auth_integration.rs
    ├── proto/                  # Thin adapter: telegram-framework -> domain types
    │   ├── src/                # lib, error, client, auth, types, stream, history,
    │   │                       #   messages, search
    │   └── Cargo.toml
    ├── domain/                 # Pure business logic (no async, no UI)
    │   └── src/                # lib, chat, message, history, search, session,
    │                           #   updates, vim
    ├── tui/                    # ratatui widgets & input handling
    │   └── src/                # lib, app, event, theme, rows, wrap, widgets/
    └── app/                    # Composition root & CLI binary
        ├── src/                # main, config, net, runtime
        └── tests/              # proto_integration.rs, tui_e2e.rs
```

There is no workspace-level `tests/` or `benches/`. Integration tests live in the
crate that owns the seam they exercise: `app`'s because it is the only crate that
can hold a framework `Client` and a `ProtoClient` at once, `telegram-framework`'s
for the login flow. `app/tests/tui_e2e.rs` is a set of `#[ignore]`d placeholders —
`termlens` is not a dependency yet, so nothing in it runs. There are no
benchmarks.

**Dependency rule (strict):** `domain` knows nothing of `telegram-framework` or
`tui`; `app` orchestrates them. `tui` depends on `domain` but **not** on `proto`
or `telegram-framework`. Only `telegram-framework` may reference `grammers`
types, and `make boundary` is what proves it.

Per-crate dependencies, which are narrower than the crate diagrams below suggest
in one direction and wider in another:

| Crate | Depends on |
| :---- | :--------- |
| `domain` | `thiserror` |
| `tui` | `domain`, `ratatui`, `crossterm` |
| `proto` | `domain`, `telegram-framework`, `thiserror`, `tracing` |
| `app` | `proto`, `telegram-framework`, `domain`, `tui`, `tokio`, `anyhow`, `clap`, `config`, `serde`, `tracing`, `tracing-subscriber`, `crossterm`, `ratatui`, `base64` |

## Workspace Members

```toml
[workspace]
resolver = "2"
members = [
    "crates/telegram-framework",
    "crates/proto",
    "crates/domain",
    "crates/tui",
    "crates/app",
]
```

## `[workspace.package]`

```toml
[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.98.1"
```

## `[workspace.dependencies]`

Use these versions when adding or updating dependencies. **Do not** specify versions inside individual crate manifests; always inherit via `workspace = true`.

```toml
[workspace.dependencies]
# Core
tokio = { version = "1.53.1", features = ["full"] }
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Telegram (published on crates.io, pinned to 0.10.0).
grammers-client = { version = "0.10.0", default-features = false }
grammers-tl-types = { version = "0.10.0", default-features = false, features = ["tl-mtproto"] }
grammers-mtproto = { version = "0.10.0" }
grammers-mtsender = { version = "0.10.0" }

# TUI
ratatui = "0.30"
crossterm = "0.28"
# Declared, not yet used by any crate. The line editor is meant to adopt it —
# see "Known Gaps" — so treat this as spoken for rather than free to pick.
vim-line = "7.7"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# CLI / config
clap = { version = "4", features = ["derive"] }
config = "0.15"
# `keyring` 3 ships no default backend, so every supported platform store is
# named explicitly. They are target-gated, so enabling all of them is safe
# everywhere; on Linux the Secret Service backend links against `libdbus`.
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service", "crypto-rust"] }
# Declared, not yet used. The global allocator is not installed; the binary runs
# on the system allocator today. See "Known Gaps".
tikv-jemallocator = "0.6"
# One use: base64 inside the OSC 52 clipboard sequence. `app/src/runtime.rs` writes
# that sequence because it is the only place holding the terminal; `tui` records
# the text and never writes it.
base64 = "0.22"

# Dev-only
tempfile = "3"
# Compile-time assertions. Used to lock the "a login token cannot be cloned or
# reused" guarantee with the compiler rather than with a convention.
static_assertions = "1.1"

# Internal crates
domain = { path = "crates/domain" }
proto = { path = "crates/proto" }
tui = { path = "crates/tui" }
telegram-framework = { path = "crates/telegram-framework" }
```

`criterion`, `bumpalo`, `termlens` and `ratatui-testlib` are **not** dependencies.
Widget tests use `ratatui`'s own `TestBackend`; there are no benchmarks; and
`app/tests/tui_e2e.rs` is waiting on `termlens` before any of it can run.

### Dependency Notes

- The `grammers` crates are taken from crates.io, **not** from git. This reverses an earlier decision, and the reason is upstream's: `Lonami/grammers` stopped tagging releases after `v0.8.0`. 0.8.1, 0.9.0 and 0.10.0 are all published to the registry with no corresponding tag, so no `tag =` can name the version that is actually current, and a `rev` pin would mean tracking `master` by hand to receive patches. The published artefact is checksummed and is the one upstream supports.

  ```console
  $ git ls-remote --tags --refs https://codeberg.org/Lonami/grammers.git
  2d51ad1e…  refs/tags/mvp
  5f153640…  refs/tags/v0.2.0
  4c2e7fc3…  refs/tags/v0.3.0
  fcb9fe38…  refs/tags/v0.4.0
  a69198d4…  refs/tags/v0.8.0        ← the newest tag that exists
  ```

  There is no release automation upstream — `.forgejo/` holds only a PR template and the `Makefile` is `fmt` and `dev` — so the version is hand-bumped per manifest and published by hand. **If a `grammers` upgrade ever fails to build, check whether a published release is internally inconsistent before assuming the fault is ours.** That is not hypothetical: 0.10.0 requires `glass_pumpkin = "2.0.0-rc0"` while depending on `num-bigint 0.4`, and `glass_pumpkin 2.0.0-rc1` moved to `num-bigint 0.5`, so the published 0.10.0 does not compile. The fix is on `master` and is unpublished; until 0.10.1 ships, `Cargo.lock` holds `glass_pumpkin` at `2.0.0-rc0` **on purpose**, which is why the lockfile is tracked. A `cargo update` that drops the pin breaks the build inside `grammers-crypto`, with a `BigUint` type mismatch, and not inside this workspace. To restore it:

  ```console
  $ cargo update -p glass_pumpkin --precise 2.0.0-rc0
  ```

  `grammers-tl-types` generates from **TL layer 227**. That is the datum the next
  schema bump is diagnosed against.
- `grammers-mtsender` is what exposes `SenderPool`, which is the only way to construct a `grammers_client::Client`. `grammers-client` re-exports its `InvocationError` but not the pool itself, so the direct dependency is required.
- `grammers-client` is built with `default-features = false`, which drops only its `fs` feature. `grammers-tl-types` is pulled in by `grammers-client` with its default features, so `tl-api` is enabled through feature unification even though this workspace asks only for `tl-mtproto`.
- `tokio` enables the `full` feature set for flexibility across crates, but the application runtime is chosen explicitly at startup (see **Memory Strategy**).
- `vim-line` is pinned to `7.7` and is **not yet used by any crate**. Its API differs substantially from older major versions, so consult the current docs when writing motion logic. The input line is hand-rolled today; adopting it is the planned fix for the gaps in **Known Gaps**.
- `keyring` needs `libdbus-1-dev` and `pkg-config` on Linux to build the Secret Service backend; CI installs them before the first `cargo` step.

## `[profile.release]`

```toml
[profile.release]
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

`panic = "abort"` means no unwinding — error recovery must be explicit. This is consistent with the `clippy.toml` ban on `unwrap()` in production code.

## `[workspace.lints.clippy]`

```toml
[workspace.lints.clippy]
all = { level = "deny", priority = 10 }
pedantic = { level = "warn", priority = 3 }
```

Individual crates opt in with:

```toml
[lints]
workspace = true
```

## Rust Edition Notes

The workspace uses `edition = "2024"`. When writing code, assume:

- Latest edition idioms are available (e.g., `let`-chains, edition-2024+ scoping rules).
- Do **not** introduce `extern crate` or other pre-2018 idioms.
- `rustfmt.toml` declares `edition = "2024"` so formatting matches the compiler edition.

## Architecture

### `telegram-framework` (First-Party `grammers` Wrapper)

A **first-party crate** that wraps `grammers-client` and provides:

- `ClientBuilder` with ergonomic user-account login (phone → code → 2FA).
- `SessionStore` trait with pluggable backends (keyring, file, memory).
- Typed event stream: `Updates` filtered to messages in private conversations with people — groups, channels and bots never reach the caller.
- Chat listing with automatic `InputPeer` resolution and caching.
- Message send/edit/delete builders.
- An **escape hatch**: `Client::invoke()` that forwards directly to `grammers`.

The modules beyond the three above are the ones with a rule in them: `dialogs`,
`history`, `messages`, `search` and `updates`. Each keeps every decision that can
be wrong in a free function over primitives, so it is testable on CI without a
datacenter, and keeps the `grammers`-reading code as thin as it can be made. That
is the same rule `app/src/net.rs` follows.

This is the only crate permitted to depend on `grammers-*`.

The `grammers` dependency is optional, behind the crate's `live` feature, and is
**off by default**. That makes the boundary above mechanically checkable —
`cargo tree -p proto` and `cargo tree -p domain` show no `grammers` crate — so
build and test with `--all-features` whenever you touch this crate. Session
storage needs no `grammers` at all and is always compiled; the client half is
what `live` gates.

### `proto`

A **thin translation layer**. Consumes `telegram-framework` and maps its types into `domain` types. It must not expose any `grammers` or `telegram-framework` types to `domain` or `tui`.

```
crates/proto/
├── src/
│   ├── lib.rs          # The `tl` re-export rule, and why app is exempt
│   ├── error.rs        # ProtoError, and the framework-error mapping
│   ├── client.rs       # Wraps telegram-framework::Client
│   ├── auth.rs         # Login, 2FA, session management
│   ├── types.rs        # Internal DTOs (never expose grammers types)
│   ├── stream.rs       # Update subscription & event mapping
│   ├── history.rs      # History page -> domain window
│   ├── messages.rs     # Send/edit/delete, and identifier widening
│   └── search.rs       # Server-side search
└── Cargo.toml          # deps: domain, telegram-framework, thiserror, tracing
```

### `domain`

Pure business logic. **No `tokio`, no `ratatui`, no `grammers`.** Only `thiserror`.

The entities are `Chat`, `Message` and `Session`; the parts with real rules in
them are the conversation window, search, and the update vocabulary.

```
crates/domain/
├── src/
│   ├── lib.rs
│   ├── chat.rs         # Chat entity, filtering rules
│   ├── message.rs      # Message entity
│   ├── history.rs      # ConversationWindow, and the page/anchor rules
│   ├── search.rs       # Query parsing, match scoring, local scanning
│   ├── selection.rs    # Mark, Selection: the two ends of a selection
│   ├── session.rs      # Session state
│   ├── updates.rs      # UpdateEvent vocabulary
│   └── vim.rs          # Pure Vim motion calculator (no UI): VimState between
│                       #   items, char_motion within one item's text
└── Cargo.toml          # deps: thiserror
```

`domain::history::ConversationWindow` is a flat, bounded `VecDeque` capped at
`CONVERSATION_WINDOW` (200). It is a window over the messages the client has
seen, not one list per conversation, because a single flat cap is what stops an
edit or a deletion from needing to find the conversation it belongs to, and what
stops that from becoming the largest allocation in the process under a live feed.

### `tui`

`ratatui` widgets and `crossterm` event handling. Depends on `domain` but **not**
on `proto` or `telegram-framework`.

```
crates/tui/
├── src/
│   ├── lib.rs
│   ├── app.rs          # The App struct, key dispatch, layout, prompt state
│   ├── event.rs        # crossterm KeyEvent -> AppAction (partly unwired)
│   ├── rows.rs         # One owner for the panel's geometry
│   ├── wrap.rs         # Text + width -> the rows it occupies
│   ├── widgets/
│   │   ├── chat_list.rs
│   │   ├── conversation.rs
│   │   ├── input_bar.rs
│   │   └── status_bar.rs
│   └── theme.rs        # Color schemes
└── Cargo.toml          # deps: domain, ratatui, crossterm
```

`app.rs` is the largest file in the workspace and holds the key dispatch, the
scroll arithmetic, the paging decision, and the prompt state. Key dispatch is
`Focus` first and `Mode` second: the conversation owns a mode (Normal, Visual,
Confirm) and the input line owns none — having the line at all is its insert mode
— because one enum cannot describe a conversation being selected in *and* a
half-written line waiting. `Tab` and `BackTab` walk the panes in the order they
are drawn, `h`/`l` step between the two panes, `Ctrl+w` leaves the line without
throwing it away, and the focused pane's block is drawn in
`Theme::border_focused`. `event.rs` exists but `key_to_action` is not yet called:
`App::handle_key` matches on `KeyEvent` directly. That is pre-existing dead code —
do not delete it without asking.

`rows.rs` and `wrap.rs` are one answer to "how tall is this message", and the
panel asks them rather than working it out again: a message is as many rows as
its text needs at the width the panel gave it, and the viewport, the scrollbar,
`Ctrl+d`/`Ctrl+u` and the fetch triggers all count those rows. `App` records the
panel's height and its width in `Cell`s on the way past, because a frame is
drawn from a shared reference and only the panel knows them. Nothing is cached:
the layout is rebuilt per frame, because a cache is a second thing to keep in
step with the window, and that is the failure this arrangement exists to
prevent.

### `app`

Composition root. Wires the async runtime, the protocol client, the domain state,
and the TUI event loop together. Returns `anyhow::Result`. Binary only — there is
no library target.

```
crates/app/
├── src/
│   ├── main.rs         # Entry point, CLI parsing (clap)
│   ├── config.rs       # Load TOML + env
│   ├── net.rs          # Client bring-up, history fetches, update pump
│   └── runtime.rs      # Tokio runtime setup, channel wiring, event loop
├── tests/
│   ├── proto_integration.rs  # Opt-in, against a real datacenter
│   └── tui_e2e.rs            # #[ignore]d placeholders awaiting termlens
└── Cargo.toml
```

`app` asks for `proto/live` and `telegram-framework/live` unconditionally, and
has no feature of its own. It is a Telegram client: the two crates that talk to
Telegram are no use to it without the client they gate, and a build of `app`
that cannot reach the network would be a build of something else. The feature
stays off inside those crates so that `make boundary` can prove *they* pull in
no `grammers` crate on their own; asking for it here is the composition root
saying what it wants.

`app` is the one crate that may name `telegram_framework::tl` (see
`crates/proto/src/lib.rs`), and it is deliberately not covered by `make boundary`
— it is the composition root, and it is allowed to see everything.

`net.rs` is the only module that knows both halves exist. It brings the client
up (build, sign in when the stored session is not enough, fetch the chat list,
take the feed), asks for a page whenever the conversation on show is near one of
its ends, and folds in whatever arrives. Everything with a rule in it is a
function over the state — `wanted`, `open_first_chat`, `backoff` — so the part
that can be wrong is tested without a client or a datacenter; the rest is the
calls. A `HistoryCursor` lives beside the loop rather than in `tui`, because
`tui` may not name `proto`.

Nothing about the account is required. `Config` reads `TELEVIM_API_ID`,
`TELEVIM_API_HASH`, `TELEVIM_PHONE`, `TELEVIM_CODE`, `TELEVIM_PASSWORD` and
`TELEVIM_SESSION_PATH`; with no credentials the client is never built and the
screen says why, and with no `session_path` the session goes to the OS
credential store. Signing in needs the phone number and the code in the
configuration, because the screen has nowhere to ask for them yet — that is a
gap, not a decision.

The log goes to a file beside the configuration (`televim.toml` → `televim.log`)
and **never the terminal**: this program draws on the terminal, and `grammers`
logs at `info` as a matter of course, so a logger that shares the screen with
the interface takes it apart on the first connection. A directory that cannot be
written leaves the run with no log rather than an unreadable screen.

`runtime.rs` is the other half of that rule, in the opposite direction: it is the
only module holding the terminal, so it is where the OSC 52 clipboard sequence is
written after `net::drive` on every pass. `tui` records the text a yank asked to
copy and never writes it — a widget that writes to the terminal behind the
renderer's back is a race.

## Core Library Rationale

| Concern               | Crate                                                                           | Rationale                                                                                              |
| :-------------------- | :------------------------------------------------------------------------------ | :----------------------------------------------------------------------------------------------------- |
| **Telegram Protocol** | `grammers-client` / `grammers-tl-types` / `grammers-mtproto` / `grammers-mtsender` `0.10.0` | Wrapped in a first-party `telegram-framework` crate. Taken from crates.io, because upstream no longer tags releases. |
| **TUI Framework**     | `ratatui` `0.30`                                                                | Immediate-mode TUI, low overhead, ideal for redraw-only-what-changed.                                  |
| **Vim Motions**       | `vim-line` `7.7` (declared, unused)                                             | Trait-based line editor with Normal/Insert modes and motions. Not adopted yet; see **Known Gaps**.      |
| **Async Runtime**     | `tokio` `1.53.1`                                                                | One event loop, so the runtime is current-thread. See **Memory Strategy**.                             |
| **Error Handling**    | `thiserror` `2` + `anyhow` `1`                                                  | Typed errors in protocol/domain; `anyhow` at the `app` boundary.                                       |
| **Logging**           | `tracing` `0.1` + `tracing-subscriber` `0.3` (`env-filter`)                     | Structured logging, written to a file. Never the terminal — see `app` below.                           |
| **Configuration**     | `config` `0.15`                                                                 | Layered TOML + env configuration.                                                                      |
| **Secret Storage**    | `keyring` `3`                                                                   | Cross-platform OS keyring integration. Never store session strings in plaintext.                       |
| **Memory Allocator**  | `tikv-jemallocator` `0.6` (declared, unused)                                   | Declared for the memory budget; not installed. See **Known Gaps**.                                    |
| **Terminal Backend**  | `crossterm` `0.28`                                                              | Backend for `ratatui`.                                                                                 |
| **TUI Testing**       | `ratatui`'s `TestBackend`                                                        | Widget assertions. `termlens` for PTY tests is wanted but not a dependency yet.                        |

## Memory Strategy

What is in place:

- `tokio` **current-thread** runtime: `Builder::new_current_thread()` in
  `app/src/runtime.rs`, explicitly, even though the `full` feature set is enabled.
- A bounded conversation window: `domain::history::ConversationWindow` caps at
  `CONVERSATION_WINDOW` messages, so full history is never held.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`. The stripped binary is currently **4.7 MB**, well inside the
  15 MB target.

What is declared but **not** in place, and so cannot be relied on:

- `tikv-jemallocator` is not installed as the global allocator. The binary runs on
  the system allocator.
- No arena allocator is used for MTProto deserialization. `bumpalo` is not a
  dependency.
- No `heaptrack`/`valgrind` step measures RSS in CI, so the 50 MB ceiling is a
  target nobody has measured yet.

### Detailed Rationale

1. **Current-thread runtime:** a TUI has one event loop, so a work-stealing
   runtime is overhead with nothing to schedule. Offload to `spawn_blocking` only
   where work genuinely blocks.
2. **Bounded window:** the cap is what keeps the process's largest allocation
   bounded under a live feed. See the `domain` section for why it is one flat
   window rather than one per conversation.
3. **Release profile:** `lto = "fat"` and `codegen-units = 1` buy size and speed
   at the cost of build time, which is the right trade for a distributed binary.
4. **Zero-copy where possible:** translating framework types into domain types
   should use `Cow<'_, str>` and byte slices rather than cloning strings, so the
   domain types stay lightweight DTOs.
5. **Allocator work is still ahead.** `jemalloc` and an arena for the parse path
   are the intended answer to long-tail fragmentation; neither is built, so treat
   any memory claim as unverified.

## Quality Harness

- `rustfmt.toml` and `clippy.toml` — both reproduced below.
- `Makefile` — `make ci` is the gate, and is what CI runs.
- CI runs `fmt-check`, `lint` (`-D warnings`), `boundary`, `test`, `build-release`.

```toml
# rustfmt.toml
edition = "2024"
max_width = 100
hard_tabs = false
tab_spaces = 4
newline_style = "Unix"
use_small_heuristics = "Default"
```

```toml
# clippy.toml
msrv = "1.98.1" # Minimum Supported Rust Version
# Disallow panics in production code
disallowed-methods = [
    { path = "std::option::Option::unwrap", reason = "Use expect() or handle the None case" },
    { path = "std::result::Result::unwrap", reason = "Use expect() or propagate the error" },
]
```

### `Makefile`

| Target | Does |
| :----- | :--- |
| `make ci` | `fmt-check lint boundary test build-release` — the whole gate |
| `make fmt` / `fmt-check` | format, or check formatting |
| `make lint` | `clippy --all-targets --all-features -- -D warnings` |
| `make boundary` | asserts no `grammers` crate outside `telegram-framework` |
| `make test` | `test --all --all-features` |
| `make check` | `check --all-targets`, faster than a build |
| `make build-release` | the optimized binary |
| `make run` / `watch` | run it, or under `cargo-watch` |
| `make audit` | `cargo audit` (needs `cargo-audit` installed) |
| `make update` | `cargo update` — **drops the `glass_pumpkin` pin**; see Dependency Notes |

`lint` and `test` both pass `--all-features` for the reason given above.

### CI Pipeline

`.github/workflows/ci.yml`, on every push and pull request, on `ubuntu-latest`:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `make boundary`
4. `cargo test --all --all-features`
5. `cargo build --release`

`libdbus-1-dev` and `pkg-config` are installed first, because `keyring`'s Linux
backend links against DBus at build time.

`.github/workflows/desktop.yml` is `workflow_dispatch` only, and covers the two
things a hosted runner cannot do:

- **keyring round trip** — `cargo test -p telegram-framework --all-features -- --ignored`,
  on a self-hosted runner with a real OS credential store. That test is
  `#[ignore]`d everywhere else for exactly that reason.
- **live datacenter** — `telegram-framework`'s `auth_integration` with
  `TELEVIM_TEST_DC=1` and the `TELEVIM_*` secrets. It takes a `login_code`
  input; without one, the login tests skip and only the `pong` check runs.

`app`'s `proto_integration.rs` is in **neither** workflow. Run it by hand — see
below.

## Testing & Verification

- **Unit Tests:** `#[cfg(test)]` modules live beside the code they cover, in
  `domain`, `tui`, and `telegram-framework`. Test the Vim motion logic
  exhaustively (e.g., `gg` on an empty buffer, `G` at the last message). Widget
  tests use `ratatui`'s `TestBackend`.
- **Integration Tests:** `crates/app/tests/proto_integration.rs` exercises the whole stack — the framework's login, the session store, the session cache, and the `proto` wrapper over both — against a real datacenter: it proves a stored session rebuilds an authorised client, that the client produces a private chat list in newest-first order, that the update feed and the chat list name conversations by the same identifier, and that history pages come back oldest first, without gaps or repeats, from an anchor that is exclusive. It lives in `app` because that is the only crate that may hold a framework `Client` and a `ProtoClient` at once. Three cases cannot be provoked with one account and are documented as deferred in the test's module docs: an update arriving for real, the offline gap `catch_up` closes, and a conversation with a known amount of history. A run prints how many updates it examined, how many the framework discarded, and how far it walked the history, so a run that proved little says so.
- **Opt-in Tests:** Anything that needs a real account — `telegram-framework`'s
  `auth_integration` and `app`'s `proto_integration` — checks `TELEVIM_TEST_DC`
  and reports a skip when it is unset, so CI stays green without credentials.
  They self-skip rather than being `#[ignore]`d: an `#[ignore]`d test is
  invisible in a normal run, so an opted-in run could not tell "ran and passed"
  from "never ran at all". The one exception is the keyring round trip in
  `session.rs`, which is `#[ignore]`d because what it needs is a credential
  store rather than a datacenter.

  To run the `app` suite, which no workflow runs for you:

  ```console
  $ TELEVIM_TEST_DC=1 TELEVIM_API_ID=… TELEVIM_API_HASH=… \
    TELEVIM_TEST_PHONE=+… TELEVIM_TEST_CODE=… \
    cargo test --all --all-features --test proto_integration
  ```

  Each test requests its own login code and Telegram throttles that hard, so run
  them sparingly.
- **TUI E2E Tests:** `app/tests/tui_e2e.rs` holds eight `#[ignore]`d
  placeholders describing what a PTY harness should assert — the screen after
  launch, typing, `:q`, scrolling, and an arrival moving a pinned view. **None of
  them run**: `termlens` is not a dev-dependency, and the test bodies are `TODO`
  comments. Keystroke and rendering coverage today comes from the unit tests and
  the `TestBackend` assertions in `tui`'s conversation panel. Wiring this up means
  adding `termlens` to `app` and filling the bodies in.
- **Memory Verification:** not implemented. The 50 MB ceiling is a target with no
  measurement behind it; adding a `heaptrack` or `valgrind --tool=massif` step
  that fails past it is the way to make it real.

## Acceptance Criteria (v1)

| Metric                   | Target                                                                                                             | Measurement Method                                          |
| :----------------------- | :----------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------- |
| **Memory Usage**         | < 50 MB RSS at idle after loading 50+ chats.                                                                       | `heaptrack` or `valgrind --tool=massif` on a release build. |
| **Startup Time**         | < 500 ms from launch to the chat list rendering on screen.                                                         | Hyperfine benchmark on a warm cache.                        |
| **Input Latency**        | < 16 ms (one frame) for a keypress to reflect in the UI.                                                           | Instrumented via `std::time::Instant` in the event loop.    |
| **Binary Size**          | < 15 MB (stripped, `lto = "fat"`).                                                                                 | `ls -lh target/release/televim`.                            |
| **Vim Fidelity**         | 95% of navigation commands from `vim-line` and common Normal-mode motions (hjkl, gg, G, /, n, N) work as expected. | Integration tests with `termlens`.                          |
| **Protocol Correctness** | Successfully authenticate, fetch the private chat list, and send/receive messages via MTProto.                     | End-to-end test against a Telegram test DC.                 |

## Feature Set

Working today:

- **Authentication:** MTProto login with 2FA, session stored in the OS keyring
  (or a file, per `session_path`).
- **Chat List:** private chats only, filtered to exclude bots, groups and
  channels, with unread counts and last-message previews. `j`/`k` move the
  highlight, `gg`/`G` reach both ends, and `Enter` opens the highlighted
  conversation. `:chat <id>` still works. Moving the highlight opens the
  conversation it lands on, once the reader has stopped moving — a held key would
  otherwise fetch every chat it scrolled past. The focused pane's border is
  drawn in `Theme::border_focused`; `h`/`l` and `Tab` move between the two panes,
  and `Ctrl+w` steps back out of the line.
- **Conversation View:** `j`/`k` by message, `g`/`G`, `Ctrl+d`/`Ctrl+u` by a
  screenful, `gg` to the first unread, `n` to cycle search matches. Messages
  soft-wrap to the panel's width — at whitespace where there is whitespace to
  break at, and at the panel's edge where there is not — so a message is as many
  rows as its text needs. Everything that measures the conversation counts those
  rows rather than messages: the slice, the scrollbar beside it, a page, and the
  `FETCH_MARGIN` (20) that asks for a page. `[you]`/`[them]` and a reply's quoted
  target are on the first row of a message, `[sending…]`/`[failed: …]` on the
  last.
- **Message Composition:** `i`/`a` to compose, `Enter` to send, `Esc` to leave.
  Reply with `r`, edit with `e`.
- **Send / edit / delete:** one message with `d`, or every message a selection
  covers in Visual, with a confirmation before deleting. The prompt counts, says
  which side the messages are from, and says how many were left out because they
  are placeholders. Delete removes for both sides. More than a hundred messages is
  several requests with a pause between them, and a failure part-way through is
  reported as how many went through rather than as a plain failure.
- **Search:** `/` searches the loaded window, then asks the server and prefers
  its answer. `n` repeats or cycles.
- **Yank / paste:** `y` in Visual puts the selection in a register — the selected
  characters for a text selection, one line per message oldest-first for a set of
  them — and `p` in Normal opens the line with it. A yank is also offered to the
  system clipboard with OSC 52, best-effort and truncated to 74 kB of sequence;
  the register is the half that always works. The register is cleared by opening
  another conversation. `p` is not bound in Visual.
- **Commands:** `:q`/`:quit` and `:chat <id>`.

Not built, and named here so nobody reads the roadmap below as current:

- **Visual mode** — `v` and `V` start a charwise or linewise selection at the
  cursor's message, `Esc` drops it, `o`/`O` swap its ends, `j`/`k` move the focus to
  another message and `h` `l` `w` `b` `e` `0` `$` `f` `t` `F` `T` move it by
  character *within* one. A selection spanning two or more messages is a set of
  messages; one inside a single message is a text range. `y` yanks it and `d`
  deletes it, with a confirmation. `dd` is `d` with no second press to
  distinguish. `p` is unbound in Visual — replacing a selection with the reader's
  own text is a destructive reading of a key that looks additive.
- **`:w`** — not a command. The only commands are `q`, `quit` and `chat <id>`.
- **Multi-line input** — the line is one row, append-only, with a fake `█`
  caret pinned to the end. `Esc` clears it and `Ctrl+w` keeps it, but the bar
  shows the Normal-mode hint rather than a dimmed draft, so a kept line is only
  reachable with `Tab`.
- **Vim motions inside the input** — none. `w`, `b`, `f`, `0`, `$` do not exist
  there; `vim-line` is declared for this and unused.
- **The yank clipboard is one-way and says nothing.** Whether a terminal honours
  OSC 52 at all is not something this program can find out, so a refused or capped
  write is not a failure of the yank and is not reported as one. `y` reaching the
  register and `p` is the whole feature; the clipboard is a convenience on top.
- **`p` is not in the input bar's hint.** The bar is one row of eighty columns and
  the hint is already 71 of the 78 it has; its length is asserted by a test, so
  adding a key means removing one.
- **A column is a character.** The wrap counts characters, so a double-width
  character or a combining mark is laid out as one column whatever cells the
  terminal gives it. What it costs is a fact about the font, and the answer
  needs a display-width table rather than a guess.

The planned shape of the first four is worked out in `~/.opencode/plan/`.

## Key Decisions

- **Why a first-party `telegram-framework` instead of `ferogram`:** The `ferogram` crate has a small contributor base and pins specific `grammers` revisions, which couples `televim` to an external maintainer's release cadence. By writing our own thin wrapper over `grammers-client`, we own the abstraction, keep the dependency surface minimal, and can tailor the API exactly to `televim`'s needs. The wrapper lives in `crates/telegram-framework` and is the only crate that touches `grammers`; `proto`, `domain`, and `tui` never see a `grammers` type.
- **Why `grammers` from crates.io rather than git:** this used to be the other way round, and the reason it changed is that upstream stopped tagging. The newest tag is `v0.8.0`; 0.8.1, 0.9.0 and 0.10.0 exist only on the registry, so a `tag =` pin cannot name the current version at all. The registry artefact is checksummed, is what upstream publishes, and a `rev` pin in place of it would make every consumer track `master` by hand to get a patch.
- **Why a peer with no bare identifier is skipped rather than given a number:** `grammers` reports none only for the account's own sentinel peer, and the account's real user identifier is only ever disclosed by asking Telegram for the account's own user, which this crate does not do. Substituting a constant would put a number in the chat list that addresses no conversation, so the conversation is dropped instead. It is unreachable for anything Telegram named — a received peer is a user, a group or a channel, and only `InputPeerSelf` yields the sentinel — and `every_real_user_keeps_its_identifier` in `updates.rs` is the test that would catch it becoming reachable, because the skip would then swallow real conversations in silence.
- **Why the update position is recorded by an explicit call:** `grammers` 0.10.0
  stopped writing it when the stream is dropped, because asking for it is `async`
  and a destructor cannot await. The position therefore moves to
  `UpdateSubscription::finish`, which the update pump calls once it has read the
  feed to its end. A feed dropped without it persists a position behind the one it
  reached, and the next launch resolves the gap by replaying updates the reader
  has already seen.
- **Why `VimState` and `char_motion` are two things:** `VimState` moves a cursor
  between the items of a list and knows nothing about what an item is;
  `char_motion` moves a position *within* one item's text and knows nothing about
  the list. A selection needs both at once, and a type that did both would be doing
  two things. For the same reason `domain::selection::Selection` answers only what
  is purely about a position — the character range, and which end is which — and
  `App::covered` answers "which messages" from the window's own order. The
  identifiers alone cannot: a placeholder for a send in flight is numbered below
  zero and sits at the *end* of the window, so a selection reaching one spans a
  different set of messages by number than by position, and acting on the wrong one
  deletes messages the reader did not select.
- **Why a status line ranks a confirmation above a selection above a search:** a
  confirmation is a question waiting for an answer and is over as soon as one is
  given; a selection is state the reader must not lose and is the one thing on
  screen whose extent is not otherwise visible; a search's label is state too, and
  a `flash` is not — so a refusal written while any of the three is up is a line
  the reader never sees. That is why an operation that finishes in Visual leaves
  Visual, and why a prompt carries the count of what it left out instead of
  flashing it.
- **Why a deletion of many messages is several requests:** Telegram caps
  `messages.deleteMessages` at 100 identifiers, and the pinned `grammers` passes
  `delete_messages` the whole vector rather than chunking it, so a longer
  selection is refused outright unless `telegram-framework` splits it. It does, in
  `delete_batches`, and waits `DELETE_BATCH_PAUSE` between the requests it makes:
  a burst of back-to-back bulk deletions is how a five-hundred-message selection
  becomes a `FLOOD_WAIT` and a half-deleted conversation. A batch that fails after
  an earlier one landed is `FrameworkError::PartialDelete`, carrying how much went
  through, because "deleted 200 of 250" and "failed" are different events and only
  one of them is actionable. Retrying the remainder is deliberately absent.
- **Why `anyhow` + `thiserror`:** `thiserror` for typed errors in `telegram-framework`, `proto`, and `domain`. `anyhow` at the `app` boundary.
- **Why `ratatui` + `crossterm`:** `ratatui` is the UI layer; `crossterm` is the terminal I/O backend. They are complementary.
- **Why `panic = "abort"`:** Reduces binary size and eliminates unwinding machinery. Requires explicit error handling throughout.
- **Why the message window is capped:** `domain::history::ConversationWindow` keeps a flat, bounded window of the messages the client has seen — a `VecDeque` capped at `CONVERSATION_WINDOW` — rather than one list per conversation or an unbounded buffer. The window exists so that an edit or a deletion can be matched to a message; capping it is what stops that from becoming the largest allocation in the process under a live feed. An event for a message that has scrolled out is not applied, which is the same answer the window already gives for a conversation it does not hold.
- **Why dropping a client stops the network:** the framework's `Client` owns the connection pool's runner task and the update relay, and aborts both when it is dropped. A detached task would leave the socket open until the process ended, and the relay would keep draining an unbounded channel nothing can read.

## Guidance for Agents

1. **Never hardcode dependency versions in a crate manifest.** Always use `workspace = true`.
2. **Respect the dependency rule.** `domain` must not pull in `tokio`, `ratatui`, `grammers`, or `tui`. If you need protocol data in `domain`, add a plain DTO.
3. **Do not use `unwrap()` in production code** — it's denied by Clippy. Use `expect()` with a message, or propagate with `?`.
4. **Prefer `Cow<'_, str>` and byte slices** when translating `telegram-framework` types into domain types.
5. **Keep `panic = "abort"` semantics in mind.** No catching panics across FFI boundaries or expecting unwinding cleanup.
6. **When adding dependencies**, edit only `[workspace.dependencies]` in the root `Cargo.toml`, then run `cargo update -p <crate>` if needed. If the crate is `glass_pumpkin`, use `--precise 2.0.0-rc0` — see item 7.
7. **The `grammers` crates come from crates.io**, because upstream stopped tagging after `v0.8.0` and no tag can name 0.8.1, 0.9.0 or 0.10.0. Do not move them back to a `tag =` or `rev =` pin without checking `git ls-remote --tags` first — a `rev` pin reintroduces hand-tracking of `master`. If an upgrade fails to build, suspect an inconsistent published release before suspecting this workspace; the `glass_pumpkin` pin in `Cargo.lock` is deliberate and `cargo update` will drop it.
8. **`Cargo.lock` is tracked.** It is a binary, so the lockfile is what makes a build reproducible, and one pin in it is load-bearing. Do not add it to `.gitignore`; if it is ever untracked, a fresh clone does not build.
9. **Runtime kind is a policy decision, not a feature flag.** Even with `tokio`'s `full` features enabled, `app/src/runtime.rs` must use `Builder::new_current_thread()` to honor the memory budget.
10. **Formatting edition must match compiler edition.** Both are `2024`.
11. **Consult `vim-line` `7.7` docs** when writing motion logic; its API differs substantially from earlier major versions.
12. **`make ci` before every commit.** It is the same gate CI runs, and it catches the `boundary` violation that nothing else does.
13. **This file describes what exists.** If a change makes a section here wrong — a new dependency, a moved module, a feature that now works — update it in the same commit. A file that is aspirational is worse than no file, because it is trusted.

## Known Gaps

Real, and named so they are not mistaken for oversights:

- **Visual mode has one operation left to bind.** `v`, `V`, `o`, `Esc`, `y` and
  `d` work and a selection is drawn; `r` in Visual does nothing. See the feature
  list for what is bound.
- **The input line has no vim controls, no multi-line, and no drafts.**
  `vim-line` is declared for exactly this and is not yet wired up.
- **A feed dropped without `finish` persists a stale update position.** See
  **Key Decisions**.
- **`tikv-jemallocator` is not installed.** No allocator work is done, and the
  50 MB ceiling is unmeasured.
- **No benchmarks and no working PTY tests.** `app/tests/tui_e2e.rs` is
  `#[ignore]`d placeholders awaiting `termlens`.
- **`tui/src/event.rs` is unwired.** `key_to_action` is not called.
- **A peer with no bare identifier is skipped**, and the skip is unreachable
  today. See **Key Decisions** for why, and which test guards it.

## v2 Hooks

The architecture leaves clear extension points for future features: a notification daemon (via `notify-rust`), file upload/download (using `tokio::fs` and `reqwest`), or a plugin system (using `wasmtime` for sandboxed extensions). Because the `domain` layer is pure, adding these features won't require touching the protocol or UI layers.
