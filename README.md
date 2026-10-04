# televim

A cross-platform Telegram client for the terminal, written in Rust. `televim` is
a portmanteau of "Telegram" and "Vim," signalling its terminal-first,
keyboard-driven identity: the UI is driven by Vim keybindings (Normal and
Insert today; Visual mode is a stub).

It displays **only private user-to-user chats** — no channels, no bots, no
groups. The primary constraint is **memory**: the client must stay under 50 MB
RSS at idle after loading 50+ chats.

## Documentation

| File | What is in it |
| :--- | :------------ |
| [`AGENTS.md`](./AGENTS.md) | The hard constraints for agents and contributors |
| [`docs/architecture.md`](./docs/architecture.md) | Workspace layout, the dependency rule, per-crate design |
| [`docs/dependencies.md`](./docs/dependencies.md) | Why each dependency is chosen or pinned |
| [`docs/decisions.md`](./docs/decisions.md) | Key Decisions — the ADRs |
| [`docs/testing.md`](./docs/testing.md) | Quality harness, `Makefile`, CI, the test layers |
| [`docs/memory.md`](./docs/memory.md) | The memory budget: measured baseline, declared work, and what the harness does not measure |
| [`docs/known-gaps.md`](./docs/known-gaps.md) | Known gaps and v2 hooks |
| [`DESIGN.md`](./DESIGN.md) | The design system |

## Project Goals

1. **Performance First:** Operate within a 50 MB RAM ceiling (stretch goal of 20–30 MB) by using a compile-time TUI, an arena allocator, and avoiding dynamic dispatch in hot paths.
2. **Vim-Centric UX:** Provide a modal editing experience (Normal, Insert, Visual) for chat navigation, message composition, and search. Every action must be reachable from the home row.
3. **Focus & Distraction-Free:** Display only user-to-user private chats. No bots, groups, or channels. This scope drastically reduces the state surface and memory footprint.
4. **Architectural Integrity:** Enforce a clean architecture where the Telegram protocol, domain logic, and UI are independent crates. The domain layer must have zero knowledge of `tokio` or `ratatui`.


## Acceptance Criteria (v1)

| Metric                   | Target                                                                                                             | Measurement Method                                          |
| :----------------------- | :----------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------- |
| **Memory Usage**         | < 50 MB RSS at idle after loading 50+ chats.                                                                       | `make measure`, RSS read in-process; see `docs/memory.md`.   |
| **Startup Time**         | < 500 ms from launch to the chat list rendering on screen.                                                         | `make measure`, `Instant` probe to the first drawn frame (the empty frame; see `docs/memory.md`). |
| **Input Latency**        | < 16 ms (one frame) for a keypress to reflect in the UI.                                                           | `make measure`, `Instant` probe from keypress to frame (excludes the terminal read and paint). |
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
  design's per-peer **colour slot is held and not built** — see [`docs/known-gaps.md`](./docs/known-gaps.md).
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
  sends nothing, and `⏎` with no client up flashes
  `not connected yet — the client is not up` and keeps the draft. The session is
  stored in the OS keyring (or a file, per `session_path`), written atomically, and
  a stored session that cannot be read is discarded at launch: the status line says
  so and the sign-in field opens, rather than the launch ending at an `offline:`
  the reader can do nothing with.
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
  library's — see [`docs/decisions.md`](./docs/decisions.md). The bar is always a draft: it grows to six
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
  it with a confirmation, and `r` is a **refusal** — see [`docs/known-gaps.md`](./docs/known-gaps.md). `dd` is
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

## Building

```console
$ make ci      # fmt-check lint boundary test design-check build-release
$ make run     # the debug binary
$ make watch   # the same, under cargo-watch
```

The planned shape of what is left here is worked out in `~/.opencode/plan/`.
