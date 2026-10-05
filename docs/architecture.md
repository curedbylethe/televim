# Architecture

`televim`'s crate layout, the rule that holds it together, and the per-crate
design. The rules themselves are in [`../AGENTS.md`](../AGENTS.md).

## Workspace Layout

```
televim/
├── Cargo.toml                  # Workspace root, [workspace.dependencies]
├── Cargo.lock                  # Tracked: a binary's lockfile is its reproducibility
├── rust-toolchain.toml         # Pin the toolchain (1.98.1)
├── rustfmt.toml                # Formatting rules
├── clippy.toml                 # Linting rules
├── Makefile                    # fmt / lint / boundary / test / ci / build-release
├── AGENTS.md                   # The hard constraints, and pointers to everything else
├── README.md                   # Overview, goals, feature set, acceptance criteria
├── docs/                       # The reasoning: architecture, dependencies,
│                               #   decisions, testing, memory, known gaps
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
    │   │                       #   account, history, messages, search, users,
    │   │                       #   updates, testing
    │   └── tests/auth_integration.rs
    ├── proto/                  # Thin adapter: telegram-framework -> domain types
    │   ├── src/                # lib, error, client, account, auth, types, stream,
    │   │                       #   history, messages, search, users
    │   └── Cargo.toml
    ├── domain/                 # Pure business logic (no async, no UI)
    │   └── src/                # lib, account, chat, message, history, search,
    │                           #   session, updates, user, vim
    ├── tui/                    # ratatui widgets & input handling
    │   └── src/                # lib, app, bidi, card, date, emoji, event,
    │                           #   grapheme, jumplist, line, rows, state,
    │                           #   text_row, theme, widgets, wrap
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
| `tui` | `domain`, `ratatui`, `crossterm`, `vim-line`, `unicode-width`, `unicode-segmentation`, `unicode-bidi`, `emojis` |
| `proto` | `domain`, `telegram-framework`, `thiserror`, `tracing` |
| `app` | `proto`, `telegram-framework`, `domain`, `tui`, `tokio`, `anyhow`, `clap`, `config`, `serde`, `tracing`, `tracing-subscriber`, `crossterm`, `ratatui`, `base64` |


## Config that lives on disk, not here

Four things this document used to reproduce are canonical in a file beside it,
and are deliberately **not** copied here — a copy would drift:

| What | Canonical in |
| :--- | :----------- |
| `[workspace]` members | `Cargo.toml` |
| `[workspace.package]` (`version`, `edition`, `rust-version`, `license`) | `Cargo.toml` |
| `[workspace.dependencies]` (every version) | `Cargo.toml` — the *reasons* are in [`dependencies.md`](./dependencies.md) |
| `[profile.release]` (`lto = "fat"`, `codegen-units = 1`, `strip`, `panic = "abort"`) | `Cargo.toml` |
| `[workspace.lints.clippy]` (`all` denied, `pedantic` warned) | `Cargo.toml`; individual crates opt in with `[lints] workspace = true` |
| `rustfmt.toml` | `rustfmt.toml` |
| `clippy.toml` (`msrv`, the `unwrap` ban) | `clippy.toml` |

`panic = "abort"` means no unwinding — error recovery must be explicit. This is
consistent with the `clippy.toml` ban on `unwrap()` in production code.

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
`account`, `history`, `messages`, `search`, `users` and `updates`. Each keeps every decision that can
be wrong in a free function over primitives, so it is testable on CI without a
datacenter, and keeps the `grammers`-reading code as thin as it can be made. That
is the same rule `app/src/net.rs` follows.

`users` is the one that reaches a person the reader has never talked to: it
resolves a `@username`, or searches the account's contacts by display name, and
seeds the returned peer's `access_hash` into the session's peer cache so the
person is addressable by every other call afterwards — without that seed, every
typed path (`send_message`, `fetch_history`) would refuse the stranger as an
unknown peer.

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
│   ├── search.rs       # Server-side search
│   └── users.rs        # Resolve a @username, or search people -> candidates
└── Cargo.toml          # deps: domain, telegram-framework, thiserror, tracing
```

### `domain`

Pure business logic. **No `tokio`, no `ratatui`, no `grammers`.** Only `thiserror`.

The entities are `Chat`, `Message` and `Session`; the parts with real rules in
them are the conversation window, search, the update vocabulary, and a person the
reader might start a conversation with.

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
│   ├── user.rs         # UserCandidate, UserSearchState: finding a person
│   └── vim.rs          # Pure Vim motion calculator (no UI): VimState between
│                       #   items, char_motion within one item's text
└── Cargo.toml          # deps: thiserror
```

`domain::user` is plain data, like the rest of the crate: `UserCandidate` is one
person the server offered (their identifier, display name and optional
`@username`), and `UserSearchState` holds the query, the candidates and the
reader's place among them — with `adopt` refusing an answer whose query the
reader has already replaced, the same stale-answer discipline `SearchState`
keeps.

`domain::history::ConversationWindow` is a flat, bounded `VecDeque` capped at
`CONVERSATION_WINDOW` (200). It is a window over the messages the client has
seen, not one list per conversation, because a single flat cap is what stops an
edit or a deletion from needing to find the conversation it belongs to, and what
stops that from becoming the largest allocation in the process under a live feed.
The per-peer draft map is a different thing one layer up: `tui` view state keyed
by peer id, like `read_receipts`, not a `domain` history structure, so it does
not reopen this decision.

### `tui`

`ratatui` widgets and `crossterm` event handling. Depends on `domain` but **not**
on `proto` or `telegram-framework`.

```
crates/tui/
├── src/
│   ├── lib.rs
│   ├── app.rs          # Key dispatch, layout, paging. Nine fields, defined in state/
│   ├── state/
│   │   ├── mod.rs
│   │   ├── session.rs      # SessionState: the session and the sign-in surface
│   │   ├── profile.rs      # ProfileCard: the card's buffer and its keystrokes
│   │   ├── chat_list.rs    # ChatListState: the chat list and its selection cursor
│   │   ├── outbox.rs       # Outbox: queued actions, fetches in flight, the clipboard
│   │   ├── pending.rs      # Pending: half-typed keys and deferred requests
│   │   ├── conversation.rs # ConversationState: the open view, registers, searches
│   │   ├── input.rs        # InputState: the live line and the emoji popup
│   │   ├── drafts.rs       # DraftStore: per-peer parked drafts and read receipts
│   │   └── ui.rs           # UiState, FrameMetrics: mode, focus, the frame's cells
│   ├── card.rs         # One profile panel over two subjects: the row model, the
│                      #   drawing, and which rows exist
│   ├── date.rs         # Civil dates: day keys, labels and `HH:MM`. Pure; no clock
│   ├── emoji.rs        # The `:query` under the caret, and its candidates
│   ├── event.rs        # crossterm KeyEvent -> AppAction (partly unwired)
│   ├── bidi.rs         # Which way a message reads, and the logical ranges one
│                      #   wrapped row is drawn in, already permuted
│   ├── grapheme.rs     # Cluster edges: what a delete removes, where a row may break
│   ├── jumplist.rs     # Where the reader has been, so a jump can be taken back
│   ├── line.rs         # The input line: owns the text, wraps vim-line; the
│                      #   value `App` parks under a peer id
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
│   │   ├── status_bar.rs
│   │   └── user_list.rs # The new-chat results overlay, over the chat list
│   └── theme.rs        # Color schemes
└── Cargo.toml          # deps: domain, ratatui, crossterm, vim-line,
                       #        unicode-width, unicode-segmentation, unicode-bidi, emojis
```

`App` is nine fields — `ui`, `session`, `profile`, `list`, `outbox`, `pending`,
`conversation`, `input` and `drafts` — one sub-struct per concern, not one
struct that holds every one of them. The methods stayed in `app.rs`, so the
file is still large: key dispatch, the scroll arithmetic, the paging decision
and the prompt handling are still methods on `App`. Key dispatch is
`App::handle_key`, `Focus` first and `Mode` second. Both fields live on
`UiState` in `state/ui.rs`, and the frame-measurement cells live on that
type's `FrameMetrics`. The conversation owns a mode (Normal, Visual, Confirm)
and the input line owns a mode of its own inside `tui::line` — having the line
at all used to be its insert mode, until the line grew a normal mode and a
visual one, and one enum could no longer describe a conversation being
selected in *and* a half-written line waiting. `Tab` and `BackTab` walk the
panes in the order they are drawn, `h`/`l` step between the two panes,
`Ctrl+w` leaves the line from any of its modes without throwing anything away,
and the focused pane's block is drawn in `Theme::border_focused`. The per-peer
draft map and the read receipts live on `DraftStore` (`state/drafts.rs`): each
conversation's `LineEditor` is parked under its outgoing peer id when the
reader leaves and restored when they return. `event.rs` exists but
`key_to_action` is not yet called: `App::handle_key` matches on `KeyEvent`
directly. That is pre-existing dead code — do not delete it without asking.

The new-conversation search is the second prompt-driven surface. `/` on the chat
list opens it (the conversation's `/` still searches messages), `:new <query>`
opens it from anywhere `:` reaches, and `⏎` queues an `Action::ResolveUser` for
the composition root — `tui` never reaches the network. Its answer is drawn by
`widgets/user_list.rs`, a transient overlay over the chat list in the
`emoji_popup`'s shape, walked with `j`/`k`/`⏎`/`Esc` while it is up; the status
line carries the query and the count, ranked above the conversation's own search
label. It is a prompt and an overlay, not a third pane.

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
**painted** caret. The bar keeps one draft per conversation: leaving parks the
buffer's `LineEditor` under the outgoing peer id and selecting a chat resumes
that peer's, so another conversation starts with its own bar. While the
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
panel's height and its width in `Cell`s on `UiState`'s `FrameMetrics` on the
way past, because a frame is drawn from a shared reference and only the panel
knows them. Nothing is cached:
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
│   ├── main.rs         # Entry point, CLI parsing (clap: --config, --chat)
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
[`decisions.md`](./decisions.md)), asks for a page whenever the conversation on
show is near one
of its ends, drives the sign-in flow the panel asks for, and folds in whatever
arrives. A recoverable feed read failure is waited out in the pump's task up to
the same bound the launch fetch uses, reporting `Event::FeedRetrying` with the
reason, the wait and the count; past the bound the feed is ended the way a dead
one is. When the update feed ends mid-session, `pump` records the position with
`UpdateSubscription::finish` and reports `Event::FeedEnded`; `drive` then rebuilds
the client from the stored session and takes a new feed in-process, through the
same `state.bringing_up` single-flight guard `:retry` uses. The relay is
single-shot, so a rebuild is the only way to re-subscribe (see
[`decisions.md`](./decisions.md)). One automatic reconnect is allowed per working
feed, and the re-`Ready` is place-preserving — the open conversation and its
cursor, the chat-list highlight restored by id, the jumplist, the selection, the
register and the draft all survive — because the reader did not navigate.
Everything with a rule in it is a function over the state — `wanted`,
`auto_reconnect`, `open_first_chat`, `backoff`, `chat_list_retry`,
`feed_error_retry` — so the part
that can be wrong is tested without a client or a datacenter; the rest is the
calls. A `HistoryCursor`
lives beside the loop rather than in `tui`, because `tui` may not name `proto`.

Nothing about the account is required. `Config` reads `TELEVIM_API_ID`,
`TELEVIM_API_HASH`, `TELEVIM_PHONE`, `TELEVIM_CODE`, `TELEVIM_PASSWORD` and
`TELEVIM_SESSION_PATH`; with no credentials the client is never built and the
sign-in surface names the pair that is missing rather than drawing a form, and
with no `session_path` the session goes to the OS credential store. `phone`,
`code` and `password` are **pre-fills**, not the way in: a launch that finds no
session opens the sign-in field with the phone already in the bar, and the flow
asks for the code there, then for a two-factor password when Telegram says the
account has one.

`Config` also reads `TELEVIM_BIDI` — `terminal` (the default) or `visual` — which
says whether this program or the terminal arranges a right-to-left row. It is
read once in `runtime.rs` and handed to `tui` as a plain `tui::bidi::BidiMode`
through `App::with_bidi`, which is the only path a configuration value takes into
`App`; `tui` names no configuration type. The value is per machine rather than per
terminal, so an ssh hop keeps it — see [`decisions.md`](./decisions.md).

`bidi.rs` is the authority and both consumers go through it: the conversation
panel and the input bar each ask `base_direction` for the direction of the text
as a whole, then `visual_row_in` for the pieces of one row of it — in the
**text's** coordinates, which is what `text_row::spans_permuted` slices the text
with, and the difference between a wrapped row drawn right-to-left and a row that
paints nothing. Under `Visual` a row is drawn through `spans_permuted` with its
caret and selection clipped to the piece that owns them; under `Terminal` the
whole row goes through `spans`, which is the same paint over one piece. Nothing
about a row's **geometry** reads the mode: the wrap, the bar's height and the
panel's layout are pure functions of the text and the width, so the same draft is
as tall in both modes. `LaidOut::visual_column` is the one place a column
depends on it — where the caret is *painted*, for the emoji popup to anchor on —
and it is `None` unless the caller passed `Visual`.

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

Which library answers which concern, and why. The per-dependency detail — what
each crate is pinned against, and what breaks when it moves — is in
[`dependencies.md`](./dependencies.md); versions are canonical in `Cargo.toml`.

