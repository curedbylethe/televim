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
    │   │                       #   account, history, messages, search, updates,
    │   │                       #   testing
    │   └── tests/auth_integration.rs
    ├── proto/                  # Thin adapter: telegram-framework -> domain types
    │   ├── src/                # lib, error, client, account, auth, types, stream,
    │   │                       #   history, messages, search
    │   └── Cargo.toml
    ├── domain/                 # Pure business logic (no async, no UI)
    │   └── src/                # lib, account, chat, message, history, search,
    │                           #   session, updates, vim
    ├── tui/                    # ratatui widgets & input handling
    │   └── src/                # lib, app, card, event, grapheme, line, rows,
    │                           #   text_row, theme, wrap, widgets/
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
| `tui` | `domain`, `ratatui`, `crossterm`, `vim-line`, `unicode-width`, `unicode-segmentation`, `emojis` |
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
version = "0.1.5"
edition = "2024"
rust-version = "1.98.1"
license = "MIT OR Apache-2.0"
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
# The input line's editor, wrapped in `tui::line` — see "Key Decisions" for why
# a wrapper and not the crate direct. `7.7` is a caret range, so `tui::line`'s
# spike tests pin the behaviour the wrapper depends on against whatever resolves.
vim-line = "7.7"
# One use: how wide a string is on a terminal, in `tui::wrap::columns`. `ratatui`
# already resolves it, so the row layout and the caret column share one table.
unicode-width = "0.2"
# One use: grapheme cluster edges, in `tui::grapheme`. A delete removes one
# and a row is never cut inside one. Already in the lockfile via ratatui.
unicode-segmentation = "1"
# The GitHub gemoji catalog, for the input line's `:shortcode` completion. The
# whole catalog rather than a curated subset: 1914 emoji, of which 1870 carry a
# shortcode, at the cost of roughly 0.5 MB of binary and 2 MB of `phf` tables
# touched only while composing, and about 5 µs to scan — a table this workspace
# otherwise maintains by hand for bytes it does not need.
emojis = "0.9"

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
- `vim-line` is pinned to `7.7` and used by `tui` for the input line, behind the
  `tui::line` wrapper. Its API differs substantially from older major versions, so consult the current docs when writing motion logic — and consult the wrapper's module docs first, because four of its behaviours are defects this workspace routes around rather than depends on: `EditResult::edits` must be applied in reverse, the word motions index bytes rather than characters, a visual selection is `cursor + 1` — so a range can run past the end of a buffer, or inside a character, and every index an edit carries is snapped on its way into the string — and a delete removes one code point, so the wrapper widens it to the cluster it touched.
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
- The account's own profile, which is the only call that discloses the account's
  own user identifier: `grammers` reports none for a peer that is the account
  itself, so a client that has not made it cannot name its own user.
- Message send/edit/delete builders.
- An **escape hatch**: `Client::invoke()` that forwards directly to `grammers`.

The modules beyond the three above are the ones with a rule in them: `dialogs`,
`account`, `history`, `messages`, `search` and `updates`. Each keeps every decision that can
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
│   ├── account.rs      # The account's own profile -> a domain Account
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
│   ├── account.rs      # The signed-in account, and its two display questions
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
│   ├── card.rs         # One profile panel over two subjects: the row model, the
│                      #   drawing, and which rows exist
│   ├── emoji.rs        # The `:query` under the caret, and its candidates
│   ├── event.rs        # crossterm KeyEvent -> AppAction (partly unwired)
│   ├── grapheme.rs     # Cluster edges: what a delete removes, where a row may break
│   ├── line.rs         # The input line: owns the text, wraps vim-line
│   ├── rows.rs         # One owner for the panel's geometry
│   ├── text_row.rs     # A run of text the reader can put a cursor in: the
│                      #   text, a match on it, a selection split out of it, and
│                      #   a caret cut into it. Shared by the conversation, the
│                      #   input bar and a card row
│   ├── wrap.rs         # Text + width -> the rows it occupies. `wrap` for a
│                      #   message; `wrap_keeping_whitespace` for the input bar;
│                      #   `columns` for how wide any of it is
│   ├── widgets/
│   │   ├── chat_list.rs
│   │   ├── conversation.rs
│   │   ├── emoji_popup.rs  # The `:shortcode` completion above the bar
│   │   ├── input_bar.rs
│   │   ├── profile.rs   # The card, in the conversation's rectangle
│   │   └── status_bar.rs
│   └── theme.rs        # Color schemes
└── Cargo.toml          # deps: domain, ratatui, crossterm, vim-line,
                       #        unicode-width, unicode-segmentation, emojis
```

`app.rs` is the largest file in the workspace and holds the key dispatch, the
scroll arithmetic, the paging decision, and the prompt state. Key dispatch is
`Focus` first and `Mode` second: the conversation owns a mode (Normal, Visual,
Confirm) and the input line owns a mode of its own inside `tui::line` — having
the line at all used to be its insert mode, until the line grew a normal mode
and a visual one, and one enum could no longer describe a conversation being
selected in *and* a half-written line waiting. `Tab` and `BackTab` walk the
panes in the order they are drawn, `h`/`l` step between the two panes, `Ctrl+w`
leaves the line from any of its modes without throwing anything away, and the
focused pane's block is drawn in `Theme::border_focused`. `event.rs` exists but
`key_to_action` is not yet called: `App::handle_key` matches on `KeyEvent`
directly. That is pre-existing dead code — do not delete it without asking.

`line.rs` owns the composed text and wraps `vim-line`, which never stores a
buffer: the wrapper applies the edits the library calculates, decides `Enter`
(send) and `Esc` (two stages, nothing lost) itself without handing either over,
and refuses the keys the library cannot run — a newline in `:` or `/`, and a byte-
counted motion *behind an operator* on non-ASCII text. A delete is widened to
the grapheme cluster it touches, because the library removes one code point and
a family is several. A `:` line and a `/` line
are prompts rather than buffers and get insert only. The bar draws the draft the
wrapper lays out with `wrap_keeping_whitespace` — the conversation's `wrap`,
except that a run of
spaces stays on the row it ends with rather than being given to neither, so that
a space the reader typed has a cell to be seen in — grown to six rows, with a
**painted** caret. While the
reader is composing, the bar stands a dim `·` in for every space in the draft:
a space is a cell that paints nothing, and a caret on a blank cell is a bar on a
blank cell, so the key that typed one looked like the key that did nothing. The
conversation gets no dots — a message is read as prose.

`text_row.rs` is the one place a run of text is painted, and it is three
surfaces sharing it: a message, a draft, and a row of a profile card. All three
need the same three things in the same order — the text, a search match patched
on, a selection split out of it — and the order is the rule rather than an
accident, because a match and a selection each set a foreground and the one
applied last wins. What it deliberately does **not** hold is everything around
the text: a message's `[you]` prefix, a card's label gutter and the line's `: `
prompt are drawn by the caller and spliced around the spans it returns, because a
conversation's sender participates in wrapping and a card's label is a fixed
gutter, and one component that grew a `label: Option<&str>` to hold both would be
two components wearing a trenchcoat. It never wraps anything either — a row is a
byte range into a string its caller owns, so a selection across a wrapped value
is arithmetic on two ranges (`rows::clip`) and never a text-layout problem.

**The caret is a marker at a position, not a span appended to a row**, which is
the one thing about it that is easy to get wrong: appending it puts the caret one
cell past wherever it belongs, which on a two-character draft is invisible and on
a message is a column out. It is cut into the row at its own offset, beside the
selection's two edges, by one walk over the row. Its offset is a **byte** offset
like every other field, because a caret is clipped and wrapped by the same
arithmetic as everything else; a motion that produces a character position
(`domain::vim::char_motion` does) is converted at the call site with
`rows::byte_span`, which is the one converter in the program.

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
│   ├── net.rs          # Client bring-up, the account's profile, history
│   │                   #   fetches, update pump
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
up (build, fetch the chat list, take the feed — it does **not** sign in; see
**Key Decisions**), asks for a page whenever the conversation on show is near one
of its ends, drives the sign-in flow the panel asks for, and folds in whatever
arrives. Everything with a rule in it is a function over the state — `wanted`,
`open_first_chat`, `backoff` — so the part that can be wrong is tested without a
client or a datacenter; the rest is the calls. A `HistoryCursor` lives beside the
loop rather than in `tui`, because `tui` may not name `proto`.

Nothing about the account is required. `Config` reads `TELEVIM_API_ID`,
`TELEVIM_API_HASH`, `TELEVIM_PHONE`, `TELEVIM_CODE`, `TELEVIM_PASSWORD` and
`TELEVIM_SESSION_PATH`; with no credentials the client is never built and the
sign-in surface names the pair that is missing rather than drawing a form, and
with no `session_path` the session goes to the OS credential store. `phone`,
`code` and `password` are **pre-fills**, not the way in: a launch that finds no
session opens the sign-in field with the phone already in the bar, and the flow
asks for the code there, then for a two-factor password when Telegram says the
account has one.

The log goes to a file beside the configuration (`televim.toml` → `televim.log`)
and **never the terminal**: this program draws on the terminal, and `grammers`
logs at `info` as a matter of course, so a logger that shares the screen with
the interface takes it apart on the first connection. A directory that cannot be
written leaves the run with no log rather than an unreadable screen.

`runtime.rs` is the other half of that rule, in the opposite direction: it is the
only module holding the terminal, so it is where the OSC 52 clipboard sequence is
written after `net::drive` on every pass. `tui` records the text a yank asked to
copy and never writes it — a widget that writes to the terminal behind the
renderer's back is a race. It is also where the kitty keyboard protocol's
`DISAMBIGUATE_ESCAPE_CODES` is pushed before the loop and popped by a guard on
the way out: the push is what makes `Shift+Enter` arrive shifted (a newline)
rather than bare (a send), and the pop is a `Drop` because leaving the terminal
enhanced would change how the user's shell reads their keyboard after exit.

## Core Library Rationale

| Concern               | Crate                                                                           | Rationale                                                                                              |
| :-------------------- | :------------------------------------------------------------------------------ | :----------------------------------------------------------------------------------------------------- |
| **Telegram Protocol** | `grammers-client` / `grammers-tl-types` / `grammers-mtproto` / `grammers-mtsender` `0.10.0` | Wrapped in a first-party `telegram-framework` crate. Taken from crates.io, because upstream no longer tags releases. |
| **TUI Framework**     | `ratatui` `0.30`                                                                | Immediate-mode TUI, low overhead, ideal for redraw-only-what-changed.                                  |
| **Vim Motions**       | `vim-line` `7.7` (wrapped in `tui::line`)                                        | Trait-based line editor with Normal/Insert modes and motions. The wrapper owns the text, decides `Enter`/`Esc` itself, and routes around four upstream defects; see `tui::line`'s docs. |
| **Emoji Catalog**     | `emojis` `0.9`                                                                  | The GitHub gemoji set behind the input line's `:shortcode` completion. `&'static` `phf` tables, reached only while composing, costing ~0.5 MB of binary and ~2 MB of RSS and ~5 µs to scan. The whole catalog, not a curated subset. |
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
  `panic = "abort"`. The stripped binary is currently **5.2 MB**, well inside the
  15 MB target — about half a megabyte of which is the emoji catalog.

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
| `make ci` | `fmt-check lint boundary test design-check build-release` — the whole gate |
| `make design-check` | fail if `design/` and the OpenDesign project differ; **skips loudly** when there is no project on the machine |
| `make design-pull` | copy the OpenDesign project's files into the repo (after a design run) |
| `make design-push` | copy the repo's design files into the OpenDesign project |
| `make design-specimen` | rebuild the specimen's frame bodies from the engine |
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
5. `make design-check`
6. `cargo build --release`

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

- **Profile card:** one widget over two subjects, the signed-in account with `S`
  and a contact with `A`, shown in the conversation's rectangle rather than as a
  third column. **A row is the same kind of object a message is**: `j`/`k`/`gg`/`G`
  between rows, `h`/`l` and `w`/`b`/`e`/`0`/`$` *within* a row's value, `v` for a
  selection — and `j` to reach further, which is how a selection becomes a set of
  whole rows, so there is no `V` — `y` the selection, and `d` on a row that acts. A
  value wraps and is still one row, so its highlight covers all of it. `Esc` is a
  ladder — a selection, then the card, then out — and `h` at a row's first cell is
  the way back, because there is nothing to its left for the motion to reach. Pane
  movement is `Ctrl-w h`/`Ctrl-w l`, since `h`/`l` are a motion; `Ctrl-w l` says
  nothing is drawn to the right of a card.
  A row exists **only when the peer says something**, so a birthday a privacy
  setting hides is a row that is not there rather than one reading "not set", and
  the title's `(n/m)` is what tells a reader a row went. `y` with no selection is
  `yy`: there is no pending-yank latch on a card, so the second press yanks the same
  row again, and the hint naming `y/yy` is naming one key rather than two. The
  design's per-peer **colour slot is held and not built** — see **Known Gaps**.
  `add account` refuses with `not yet: this build cannot add an account`; `logout`
  confirms on the same screen-wide row and then signs out — the session is
  discarded, the list empties, and the sign-in field comes back for the phone; on a
  contact's card `d` refuses, because that card has no row to act on. Read once at
  start-up, not on open: a panel that blanked and refilled every time would be one
  the reader could not trust. With no credentials it says so, and says why.
- **A contact's card reads their profile, and says so until it arrives.** `A`
  queues a `users.getFullUser` for the person the highlight is on, and the card
  draws **no rows at all** until the answer lands — a shell sentence naming them
  and saying it is reading, or a shell sentence saying why it could not be read.
  Half a card that fills in is a card the reader has to believe a moment before
  they do not have to, and a shell is the one thing that says what it is waiting
  for. The two shell sentences are the account's with their own wording: the
  account's first line is `not signed in` because its problem is credentials, and
  putting that on a failed profile read would send a reader to check something
  that is not the problem.
  A contact's rows are the ones **they** said, and a field their privacy hides is
  *absent* rather than empty — so the identity row degrades to the username alone
  and to **no row at all** when there is neither, which is not what your own card
  does. Your own card writes `not set`, because you always have a phone number and
  a missing one is a fact; a contact who sets no username has not told you
  something is wrong with them. The count in the title is what makes the
  difference visible.
  One read per card opened, dropped when the card is no longer on show, because a
  round trip is long enough that opening a second card first is the normal case
  rather than a race.
- **Authentication:** MTProto login with 2FA, asked for in the sign-in surface
  rather than in the configuration: `:signin` opens it, and a launch that finds no
  session opens it with the phone already in the bar. The phone, the code
  Telegram sends and a two-factor password are typed in the bar (titles
  ` Phone `, ` Login code `, ` Password `, the last masked); the panel draws its
  `Phone` and `Login code` rows, and a
  `two-factor password (n attempts left)` row **only** when Telegram answers
  `SESSION_PASSWORD_NEEDED`. A refusal is a sentence that stays up rather than a
  flash. `⏎` sends the field once — a second `⏎` while `Checking…` says so and
  sends nothing. The session is stored in the OS keyring (or a file, per
  `session_path`).
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
- **Message Composition:** `i`/`a` to compose, `Enter` to send, `Esc` to stop
  typing and a second `Esc` to leave — nothing typed is ever lost to an `Esc`.
  The line is a real editor (`vim-line`, wrapped in `tui::line`): caret
  movement, `w`/`b`/`e`, `x`, `dw`, `cc`, `p`, `gg`/`G`, a visual selection with
  `d` and `y`, and multi-line messages with `Ctrl+J` (`Shift+Enter` where the
  terminal volunteers the distinction). `gg` and `G` are the wrapper's, not the
  library's — see **Key Decisions**. The bar is always a draft: it grows to six
  rows, survives a conversation switch, and is drawn with a painted caret that
  `TestBackend` can assert, which the real terminal's could not.
  A `:shortcode` opens a completion popup above the bar: `↑`/`↓` choose a
  candidate, `⇥`/`⏎` accept one, `Esc` closes the popup, and every other key
  keeps typing into the draft. Reply with `r`, edit with `e`.
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
  another conversation. `p` is not bound in Visual. The line keeps its own
  internal yank buffer for its own `p`, fed by its own `y` and `d`; the two
  registers are deliberately not shared, because their formats differ.
- **Commands:** `:q`/`:quit` and `:chat <id>`. `q` in the conversation and `:q`/
  `:quit` both raise `Quit televim? (y/n)` first — the same screen-wide
  confirmation a deletion uses — because `q` is one keystroke away from a key
  that types nothing else. `y` quits, `n` or `Esc` stays. `Ctrl-C` does **not**
  ask: it is the way out when the program is wedged, and a terminal that is not
  answering cannot draw the question either.

Not built, and named here so nobody reads the roadmap below as current:

- **Visual mode** *(the conversation; a card has `v` alone and reaches further with
  `j`)* — `v` and `V` start a charwise or linewise selection at the
  cursor's message, `Esc` drops it, `o`/`O` swap its ends, `j`/`k` move the focus to
  another message and `h` `l` `w` `b` `e` `0` `$` `f` `t` `F` `T` move it by
  character *within* one. A selection spanning two or more messages is a set of
  messages; one inside a single message is a text range. `y` yanks it, `d` deletes
  it with a confirmation, and `r` is a **refusal** — see **Known Gaps**. `dd` is
  `d` with no second press to distinguish. `p` is unbound in Visual — replacing a
  selection with the reader's own text is a destructive reading of a key that
  looks additive.
- **`:w`** — not a command. The commands are `q`/`quit`, `chat <id>`, `settings`
  and `signin`.
- **Word motions *behind an operator* on non-ASCII text in the input** — refused
  with a message. `vim-line`'s word motions index bytes rather than characters,
  and behind `d`/`c`/`y` the motion and the slice to apply it happen inside one
  key, so there is no moment at which to snap anything. That is the only place a
  motion is refused: `w`/`b`/`e`/`W`/`B`/`E` in the line's normal mode, and a word
  motion extending a visual selection, both *run* on text with emoji in it, as
  does `j`/`k` on its own — where the only thing they can do is move a cursor the
  next key re-snaps. `dw` and `cc` are the keys still refused, and they say so.
  `h`, `l`, `0`, `$`, `^`, `dd` and the arrows have always run.
- **The yank clipboard is one-way and says nothing.** Whether a terminal honours
  OSC 52 at all is not something this program can find out, so a refused or capped
  write is not a failure of the yank and is not reported as one. `y` reaching the
  register and `p` is the whole feature; the clipboard is a convenience on top.

The planned shape of what is left here is worked out in `~/.opencode/plan/`.

## Key Decisions

- **Why `bring_up` does not sign in:** signing in needs a code Telegram has just
  sent and, when the account has one, a two-factor password only the reader has,
  and both are answered while the screen is up — so a bring-up that blocked on
  them would either hold the first frame until a datacenter answered a question
  nobody had been asked yet, or write a session its owner had no part in
  choosing. It builds the client, asks `is_authorized`, and reports either way:
  `Event::Ready` with no account is a machine with no session, and the panel opens
  the sign-in field for it. Two consequences are deliberate. `Ready` no longer
  implies an account, so every caller treats it as "you are in" and
  `account: Err(String::new())` is the one value that means "there is nothing to
  read yet" rather than "a read failed". And a bring-up failure narrowed: it is
  now only "the client could not be built" or "the chat list could not be
  fetched", because a missing phone is no longer something bring-up needs.
- **Why logging out rebuilds the client:** the update feed is single-shot. The
  framework's relay hands its receiver to a subscriber once (`UpdateRelay::take`)
  and nothing puts one back — not a sign-out, not dropping the subscription — so a
  second `subscribe_updates` on the same client refuses with
  `UpdatesAlreadySubscribed` before it touches the network. A sign-out that kept
  the client would leave a reader who signs back in with a live-looking window
  that never receives another message, and the fault would surface long after the
  act. So `Event::LoggedOut` drops the `Arc` — which is what ends the old pump,
  because the relay aborts with the client — and asks bring-up for a fresh one,
  whose `Ready` carries `Err(String::new())` and installs the signed-out screen and
  the sign-in field together. `State` holds the `Config` and the channel, so the
  respawn needed no change to `apply`'s signature.
- **Why the sign-in surface is an overlay and not a `Pane` variant:** `Pane` is
  `Copy`, and its lifecycle belongs to the layout — `set_focus` closes the
  profile, `Tab` and `Ctrl-w` walk out of the right-hand column — while a flow in
  progress holds `String`s and has to survive every one of those. So
  `App::signin: Option<SignIn>` is intercepted ahead of the pane walk, the same
  place `Mode::Confirm` and the `:` completion are intercepted, and neither `Pane`
  nor `AccountState` grew a case for it. `AccountState`'s fourth case would have
  been redundant besides: the account's card already says `not signed in` and
  names `:signin`.
- **Why a first-party `telegram-framework` instead of `ferogram`:** The `ferogram` crate has a small contributor base and pins specific `grammers` revisions, which couples `televim` to an external maintainer's release cadence. By writing our own thin wrapper over `grammers-client`, we own the abstraction, keep the dependency surface minimal, and can tailor the API exactly to `televim`'s needs. The wrapper lives in `crates/telegram-framework` and is the only crate that touches `grammers`; `proto`, `domain`, and `tui` never see a `grammers` type.
- **Why `grammers` from crates.io rather than git:** this used to be the other way round, and the reason it changed is that upstream stopped tagging. The newest tag is `v0.8.0`; 0.8.1, 0.9.0 and 0.10.0 exist only on the registry, so a `tag =` pin cannot name the current version at all. The registry artefact is checksummed, is what upstream publishes, and a `rev` pin in place of it would make every consumer track `master` by hand to get a patch.
- **Why a peer with no bare identifier is skipped rather than given a number:** `grammers` reports none only for the account's own sentinel peer, and the account's real user identifier is only ever disclosed by asking Telegram for the account's own user. Substituting a constant would put a number in the chat list that addresses no conversation, so the conversation is dropped instead. It is unreachable for anything Telegram named — a received peer is a user, a group or a channel, and only `InputPeerSelf` yields the sentinel — and `every_real_user_keeps_its_identifier` in `updates.rs` is the test that would catch it becoming reachable, because the skip would then swallow real conversations in silence. `Client::fetch_account` is that call, and it does not change the skip: naming the account is not filing a conversation under it, and Saved Messages still has no bare peer to file it under.
- **Why the right-hand column is a `Pane` and not a third `Focus`:** `Focus` says where a keystroke lands, and being on the line is what makes it in insert mode — it is about the *line*. What the right-hand column holds is a different question, and a contact profile is the same question with a different answer, so folding it into `Focus` would make one enum mean both "which pane" and "what is in this one". Two axes means every existing `Focus` arm keeps working, the layout and the focus ring are untouched, and a contact profile is later a `ProfileId` and nothing else. The bill is four sites that read the focus and assumed the column held a conversation — the hint row, the mode label, the status bar's style, and each panel's border — and all four read the pane now. That is the argument for doing the split once: those are the four arms a third `Focus` variant would each have grown.
- **Why the profile has its own highlight:** `App::vim`'s total is the conversation window's length, and `VimState` is one value. Driving two lists from it means each one's row count stands in for the other's, and neither test fails on its own. `VimState` knows nothing about what an item is, which is what makes the second value free.
- **Why a `:` line never completes a shortcode:** the completion is for the reader's *own text*, and a command line is not that — `refresh_completion` requires a buffer, so a `:` line is out before the catalog is consulted. This is worth stating because the catalog would otherwise decide what a command does: it holds `seedling` and `seven` but no `settings`, so a `:settings` that reached it would work by the catalog running out. `a_command_line_never_completes_a_shortcode` is the test, and it is a test of a reason rather than of an output.
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
- **Why `r` in Visual mode is a refusal and not a reply:** two reasons, and they
  differ. A selection that is not inside one message has no quote to send —
  Telegram quotes a fragment of *one* message, and there is no wire representation
  for quoting five. And a quote of one message cannot be sent at all on the pinned
  `grammers`: `InputMessage` has no field for one, and the `input_reply_to` helper
  hard-codes `quote_text`/`quote_offset` to `None`. That is the upstream gap
  `~/.opencode/plan/pr-grammers-quote-support.md` is about. It is a refusal rather
  than a workaround because composing the quote as ordinary message text produces
  something that *looks* like a quote and is not, and the difference is visible to
  the person receiving it. It is not "reply to the cursor's message instead"
  either: a key that answered a different question than the one asked, while the
  screen said `-- VISUAL --`, would be worse than a refusal. The refusal is
  replaced in a follow-up once the upstream patch lands; the two wordings are
  distinct so a reader can tell which of the two applies. For the same reason
  `domain::utf16_len` — the UTF-16 offset conversion a quote needs — is **not**
  written yet: it would have no caller, and the whole point of having one is that
  there is exactly one thing to test.
- **Why an operation that finishes in Visual leaves Visual:** a selection's own
  note on the status line outranks a transient status, so a `flash` written while a
  selection is up is a line the reader never sees. `y`, `d` and `r` all return to
  Normal whether they worked or not, and a prompt carries its counts rather than
  flashing them.
- **Why a card is a `Paragraph` and not a `List`:** two things, and both are the
  same thing. A value wraps, and a wrapped value is **one row** the reader moves
  over with one `j` whose highlight covers all of its lines; a `List` selects one
  item, so it would need one item per drawn line — which makes `j` move a third of
  the way through a field. And a caret is a *cell within* a row, which
  `ListItem` cannot hold. So the reversed cursor row is patched onto the spans by
  hand, in the order `theme`'s module doc states, and the panel must not add
  `Wrap` on top: it wraps its own values with `wrap::wrap` and hands over finished
  lines, because the rows a value occupies are the geometry `j`, the highlight and
  the caret all count.
- **Why a caret is an overlay and not a cell of its own:** a cell inserted at the
  caret's position pushes the text after it along, and a card's values would jump a
  column every time the cursor landed on one of them — a column of a table that
  moves when you look at a cell in it. So the caret is a one-character *style
  range*: the character at its offset, re-styled. It matches the design's own
  stylesheet, where the caret is a `::after` pseudo-element drawn on the cell rather
  than a character in the stream. Only a position one past the end of a value has
  no character to mark, and that one takes a cell of its own, which is at the end
  of a value where a column of movement is invisible.
- **Why `h` does two jobs on a card:** it is a motion everywhere else, and the way
  back at a row's first cell. It is sound because a card is **one column of
  values**: there is no column to the left of the first one, so at that edge the
  motion is finished rather than the key having been repurposed mid-word. A reader
  with no way back has a stuck screen, which is worse than a key with a second
  meaning at its own edge. Pane navigation moved to `Ctrl-w h`/`Ctrl-w l` for the
  same reason `h` cannot be both a motion and a pane: one key in two places is a key
  a reader learns twice.
- **Why the design model is vendored into the repository:** it is the most faithful
  model of every screen — 43 drivable scenes, each verified at exactly 24 rows by
  80 columns, with a generator that rebuilds the specimen from them — and it was
  living in an application data directory that is not a git repository.
  OpenDesign keeps its own store of UUID-named files, which is an *undo* history
  rather than a reviewable one: no branches, no diff on a screen, no pull request.
  So the repo is the versioned record and OpenDesign is the working surface, and
  `scripts/design-sync.sh` holds the two together with an explicit path map.
  The two hand-written frame sets it replaced had **3 commits against 129**,
  nothing referenced them, nothing measured them, and three of the eight already
  described a panel the code no longer shipped — a record nobody maintains is not a
  record.
- **Why the design model is not a reference implementation:** `televim-engine.js` is
  a third answer to "how tall is this message", beside `rows.rs` and `wrap.rs`, and
  nothing keeps the three in step. **When the engine and the Rust disagree the Rust
  is right.** A green engine run proves nothing about the binary: the behavioural
  source of truth is the Rust's own `TestBackend` assertions, and the engine is the
  *visual* one. `design/README.md` says so at the point of use, because the failure
  mode is somebody "fixing" the engine when the program is what moved.
- **Why the specimen is tracked even though its frames are generated:** it is a
  document, not a generated file. Its headings, ledes and notes are hand-written and
  the frame *bodies* inside it are written by `build-specimen.js`, so the prose is
  the reason it is in git and the generator is what keeps the frames honest. It is
  idempotent: run it and get no diff, and the committed specimen is already what the
  engine produces.
- **Why `anyhow` + `thiserror`:** `thiserror` for typed errors in `telegram-framework`, `proto`, and `domain`. `anyhow` at the `app` boundary.
- **Why `ratatui` + `crossterm`:** `ratatui` is the UI layer; `crossterm` is the terminal I/O backend. They are complementary.
- **Why `panic = "abort"`:** Reduces binary size and eliminates unwinding machinery. Requires explicit error handling throughout.
- **Why the message window is capped:** `domain::history::ConversationWindow` keeps a flat, bounded window of the messages the client has seen — a `VecDeque` capped at `CONVERSATION_WINDOW` — rather than one list per conversation or an unbounded buffer. The window exists so that an edit or a deletion can be matched to a message; capping it is what stops that from becoming the largest allocation in the process under a live feed. An event for a message that has scrolled out is not applied, which is the same answer the window already gives for a conversation it does not hold.
- **Why dropping a client stops the network:** the framework's `Client` owns the connection pool's runner task and the update relay, and aborts both when it is dropped. A detached task would leave the socket open until the process ended, and the relay would keep draining an unbounded channel nothing can read.
- **Why `gg` and `G` are the wrapper's and not the library's:** `vim-line` 7.7's
  motion table is `h l j k 0 ^ $ w b e W B E %`, and its normal mode answers
  everything else by switching mode, taking an operator, or deleting — so `g`
  and `G` fall out of the end of it. An unhandled key there is *dropped*, not
  refused, which is why they arrived as silence rather than as a message. They
  are two motions over the whole draft, which is the one thing the library has
  no notion of: its `0` and `$` are scoped to the caret's **row**, and nothing
  addresses the buffer. Both are kept, because they are not the same answer —
  a reader who has just pressed `gg` and then `0` means "and now this row", not
  "back where I was", and `zero_and_dollar_stay_on_the_caret_row_and_gg_and_upper_g_do_not`
  is the test that says so. `G` lands **on** the last character rather than past
  it, which is the library's own normal-mode invariant (`clamp_to_last_char`,
  applied to every motion it dispatches) and what `move_line_end` does for `$`; a
  caret one past the end is one the next `h`, `x` or `dd` would act on wrongly.
  They run in the line's own normal mode only, because in insert mode they are
  letters, and the wrapper's `pending_g` is spent by every key — including the
  `Esc` that returns before any dispatch happens, which is the one key that could
  otherwise leave a prefix standing behind a mode the reader had left.
- **Why the input line is a wrapper rather than the crate:** `vim-line` never stores the buffer, so somebody has to apply its edits — and that somebody is where the decisions live that the crate must not be asked about. `Enter` (send) and `Esc` (two stages, nothing lost) never reach it, because its own answers differ by mode; `:` and `/` get insert only, because a newline in either is a submission nobody asked for. The wrapper also snaps every position to a character boundary — the cursor around every key, and every index an edit carries on its way into the string — then widens a delete to the grapheme cluster that index falls in, and refuses the byte-counted motions *behind an operator* on non-ASCII text, because two of the crate's behaviours split multi-byte characters and this binary sets `panic = "abort"`. That is the only place a motion is refused: elsewhere a motion can only move a cursor, which is re-snapped before anything can slice on it.

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
11. **Consult `tui::line`'s module docs before `vim-line`'s docs** when writing motion logic. The crate's API differs substantially from earlier major versions, and four of its behaviours are defects this workspace routes around rather than depends on.
12. **`make ci` before every commit.** It is the same gate CI runs, and it catches the `boundary` violation that nothing else does.
13. **This file describes what exists.** If a change makes a section here wrong — a new dependency, a moved module, a feature that now works — update it in the same commit. A file that is aspirational is worse than no file, because it is trusted.

## Known Gaps

Real, and named so they are not mistaken for oversights:

- **The per-peer colour slot is held, not built.** `CardRow::reserved` is emitted
  between a contact's `name` and `username`, draws nothing, is not selectable, is
  not something `d` can act on, is skipped by a yank, and is neither counted nor
  numbered in the title. It is there to hold an **index**, because a reservation
  held only by a comment is one the next person to insert a field between the name
  and the username loses silently. The design's five constraints for the picker — a
  fixed named set, a contrast floor against `--text`, the chat list's cursor row
  winning, a light-terminal value, local state keyed by the peer's id — are in
  `DESIGN.md`. The slot is *interior* on a contact's card, so `j` and `k` step over
  it, and `card::navigable` bounds the highlight past a slot at the end.
- **The design artifact does not model a contact card before its read lands.**
  The engine has a shell for the account's two states — `reading` and `signedout`
  — and none for a contact's, so the third and fourth shells the binary now draws
  are only in the Rust. That is a divergence in the *other* direction from the
  usual one, and it is left rather than hand-fixed: the engine is a 59 kB design
  model that an agent wrote against `DESIGN.md`, and editing it by hand to add a
  state is the failure `design/README.md` names. It wants a design run that adds
  the state to `DESIGN.md` and the engine together, and `make design-pull` after.
- **The join of a `userFull` to its `user` is unasserted.** Every decision it makes
  is tested at the level it can be written at — an empty `about` is not a bio, an
  empty username is not a username, a date that is not a date is dropped, a
  `userEmpty` is not a profile — but the two objects are not joined in a test.
  Layer 227's `tl::types::UserFull` is 58 fields with no `Default`, so the fixture
  would be a 58-field literal that breaks on every schema bump and reads as noise.
  The `live` integration test in `app/tests/proto_integration.rs` is the other end
  of it, and it needs a datacenter.
- **`add account` is a row that refuses.** It flashes
  `not yet: this build cannot add an account`, one word long because a deliberate
  refusal is not a `[failed: …]` — that form is for something that tried and did
  not come back, and a reader who reads it as a bug will go looking for one that
  does not exist. `logout` used to refuse the same way; it signs out now.
- **The design artifact still refuses a sign-out the program performs.** The
  engine's only logout path is `cardAct`'s `not yet: this build cannot sign out`,
  and it has no post-logout state at all: `fresh(view)` knows
  `signin|stale|nocreds|signedout|reading`, and the nearest of them, `stale`, is a
  session Telegram revoked rather than one the reader ended. The model and the
  binary disagree, and the specimen's frame still shows the refusal. It is left
  rather than hand-fixed, the way the missing contact-card shell is: the engine is
  a design model an agent wrote against `DESIGN.md`, and editing it by hand to add
  a state is the failure `design/README.md` names. It wants a design run that adds
  the state to `DESIGN.md` and the engine together, and `make design-pull` after.
- **`Config::code` and `Config::password` are pre-fills, not a way in.** They
  fill the code and the password fields when a flow reaches them; the flow is the
  only path that writes a session, and the phone has to reach the bar once.
- **A failed chat-list fetch has no retry.** Bring-up sends `Event::Offline` and
  the screen says so. The update feed backs off; this does not.
- **Two sign-in hints in the Rust differ from the engine's text.** The bar's field
  hint is ` ⏎: send  Esc: cancel` and the waiting hint is
  ` Checking… — the request is in flight`, where the design model reuses its
  insert hint and the single word `Checking…`. A login code is one row with no
  newline to type and `Shift+⏎` is refused there, so the insert hint would name a
  key that does nothing; the waiting hint spells out the state the design leaves
  to the status line.

- **Visual mode's `r` refuses rather than replying.** `v`, `V`, `o`, `Esc`, `y` and
  `d` work and a selection is drawn. `r` in Visual says no, for one of two reasons —
  see **Key Decisions**. Neither `domain::utf16_len` nor the rendering of an
  incoming quote's fragment is written, because both exist only for a quote this
  build cannot send.
- **Word motions *behind an operator* in the input refuse non-ASCII text.** `w`,
  `b`, `e` and a vertical motion say no with a flash, because `vim-line` counts
  those motions in bytes and `d`/`c` slice in the same key. Everywhere else they
  run: in the line's normal mode and as a visual selection's extent, where the
  only thing a motion can do is move a cursor the next key re-snaps. The
  wrapper's cursor is snapped to boundaries around every key either way.
- **Three keys mean something else while a `:shortcode` completion is up.**
  `Tab`, `Enter` and `↑`/`↓` choose or accept a candidate instead of walking a
  pane, sending the message, or moving the caret. `Esc` closes the popup and
  gets them back, and the status line names them for as long as it is up.
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
