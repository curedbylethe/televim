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
| [`CHANGELOG.md`](./CHANGELOG.md) | Release history, generated per release from the commits |

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
| **Vim Fidelity**         | 95% of navigation commands from `vim-line` and common Normal-mode motions (hjkl, gg, G, /, n, N) work as expected. | Integration tests with `termlens` (`app/tests/tui_e2e.rs`). Today they cover launch, INSERT typing, `:q` and Ctrl-u/Ctrl-d scrolling, not the full command set. |
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
  A card opens from the chat list without replacing the conversation. Stopping on a
  chat after moving the highlight shows its card in the conversation's rectangle,
  and focus stays where it was, so `j`/`k` keep moving the list. `Enter` on the list
  shows the same card and moves focus to it. `Enter` on the card **confirms**: the
  conversation opens through the same path as choosing the chat from the list, and
  the card goes. A card whose chat is not in the list lists that private chat under
  the name the card holds (the peer id when it holds none) and opens it, the same way
  a new conversation from user search does. `Esc` closes the card
  and leaves the conversation as it was. `S` is unchanged and shows the account's own
  card.
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
  stored in the OS keyring, or in an **encrypted** file when `session_path` is set
  (AES-256-GCM-SIV, `0600`, written atomically; the profile card says
  `encrypted file <path>`). The file's key is `TELEVIM_SESSION_PASSPHRASE` (or
  `session_passphrase` in the configuration; prefer the variable), else a random key
  kept in the OS keyring. With neither, the launch ends at `offline:` naming the
  missing key and the file is left as it was — it is never written in plaintext —
  and a wrong passphrase ends the same way and never discards the session. A file
  written by an older build is encrypted on its first launch. A stored session that
  cannot be parsed is discarded at launch: the status line says
  so and the sign-in field opens, rather than the launch ending at an `offline:`
  the reader can do nothing with. A launch that does end at `offline:` has asked
  for the chat list three times and waited between them, naming the reason, the
  wait and the count on the status line each time — and `:retry` asks again.
- **Chat List:** private chats only, filtered to exclude bots, groups and
  channels, with unread counts and last-message previews. `j`/`k` move the
  highlight, `gg`/`G` reach both ends, and `Enter` shows the highlighted chat's
  profile card, which `Enter` in turn confirms into the conversation (see the
  profile card above). `:chat <id>` still works. Moving the
  highlight shows the card of the chat it lands on, once the reader has stopped
  moving — a held key would otherwise fetch the card of every chat it scrolled
  past, and the conversation itself is neither opened nor marked by browsing.
  Confirming a chat that has unread messages tells Telegram it is read, up to the
  newest message the list knows of, and the count clears once Telegram accepts
  that; a refused marker leaves the count where it was, and the next open asks
  again. While any card covers the conversation nothing is marked: the mark waits
  for the card to be confirmed or closed. A message that arrives in the
  conversation on show is marked read the same way, and one that arrives under a
  card is kept and marked on the pass after `Esc`; a read
  done on another device sets the count to whatever Telegram still counts as
  unread. `p` pins the highlighted chat, or
  unpins it; the pin is kept by Telegram, so it survives a restart, and pinned
  chats lead the list, newest first within each section. The focused pane's border is
  drawn in `Theme::border_focused`; `h`/`l` and `Tab` move between the two panes,
  and `Ctrl+w` steps back out of the line.
- **Starting a conversation with someone new:** `/` on the chat list finds a
  *person* rather than a message — there is no open conversation for it to
  search, so it does not borrow the conversation's meaning — and `:new <query>`
  opens the same prompt from anywhere `:` reaches, pre-filled. It is the same
  surface as the message search: a prompt on the bar behind a `/` prefix with a
  status label, submitted with `Enter`. The server
  answers with candidates; an ambiguous set is drawn as a short overlay list over
  the chat list, walked with `j`/`k` or `↑`/`↓` and opened with `Enter`, and
  dismissed with `Esc`. Each row is the person's name, their `@username` when
  they have one, and a standing — `chat` when a conversation with them already
  exists, `new` when choosing them makes one — with the fragment the reader typed
  inked `match`. Picking someone already in the list focuses that chat rather than
  creating a second; picking someone new lists and opens one. Submitting the
  prompt empty lists the reader's most-contacted people instead, the server's
  top correspondents (`contacts.GetTopPeers`), in the same overlay. The status line says
  where the search stands: `searching…` while an answer is in flight, the count
  when it lands, and why when it finds nobody or fails.
- **Searching every private chat:** `?` opens a prompt on the bar, from the
  conversation and from the chat list alike, and `:search <query>` runs the same
  search from anywhere `:` reaches; a bare `:search` or an empty `?` repeats the
  last one. The search asks the server once for matches across every private
  chat. The answer is drawn as an overlay over the whole body: one rule per chat
  with the chat's name and its count, then one row per message with the time, the
  sender as `[them]` or `[you]`, and a snippet cut around the first match, with
  every match inked `match`. `j`/`k` or the arrows move between messages, `{` and
  `}` between chat headers, and `gg`/`G` to the first and last message. `Enter`
  opens the message's chat with the cursor on that message: a message already on
  screen, or in the local history cache, is a cursor move, and one that is neither is
  fetched, as `gd` fetches a quoted message, with `Jumping…` on the bar until it lands. `Esc` closes the overlay,
  returns focus to the pane the search was raised from, and forgets the query. The
  overlay owns the keys while it is up, so none of them writes to a chat; `/`, `:`
  and `?` still open their prompts, and `?` is refused while the new-chat list is
  up. The status line says where the search stands: `?<query> — searching…`, then
  `?<query> — N results in M chats` when it lands, `no matches`, or
  `search failed (<reason>)`. A result list that is more than 100 matches says
  `100 of <total>`, and only the first 100 are shown. Only private chats are
  searched; groups and channels are dropped. `/` keeps its meaning on each pane.
- **Conversation View:** `j`/`k` by message, `g`/`G`, `Ctrl+d`/`Ctrl+u` by a
  screenful, `gg` to the first unread, `n` to cycle search matches. `gd` on a
  message that quotes another goes to the quoted message: a cursor move when it
  is loaded, and otherwise a page fetched around it that replaces the window,
  with `Jumping to the quoted message…` said while that page is in flight.
  `Ctrl-o` goes back to the message the jump left and `Ctrl-i` forward again —
  a return to a message the window no longer holds is fetched the same way, and
  says `Jumping back…` or `Jumping forward…`. While a page is in flight only
  `Esc` answers, and it leaves the reader where they were. `gd` on a message
  that quotes nothing refuses with `Not a reply: gd jumps to the message a reply
  quotes.`, and a quote the client cannot fetch at all with `That message is no
  longer available.` Messages soft-wrap to the panel's width — at whitespace
  where there is whitespace to break at, and at the panel's edge where there is
  not — so a message is as many rows as its text needs. Everything that measures
  the conversation counts those rows rather than messages: the slice, the
  scrollbar beside it, a page, and the
  `FETCH_MARGIN` (20) that asks for a page. `[you]`/`[them]` and a reply's quoted
  target are on the first row of a message, `[sending…]`/`[failed: …]` on the
  last.
- **Media:** a message that carries something and says nothing about it still has
  a body — `[image]`, `[video]`, `[gif]`, `[voice]`, `[sticker]` or `[file]` — so an
  attachment does not render as an empty row. A message with a caption shows the
  caption and nothing else, and a kind this build does not model becomes
  `[file]` rather than disappearing; a message with no media at all is unchanged.
  A static sticker with its bytes draws its picture inline — a bounded block,
  24 columns by 8 rows, half-block cells — and `[sticker]` while they are
  missing; `stickers = "off"` (or `TELEVIM_STICKERS=off`) draws the token for
  every sticker message and fetches nothing. `graphics = "auto"` (or
  `TELEVIM_GRAPHICS`) places the picture with the kitty graphics protocol where
  the environment names kitty, ghostty or WezTerm; `kitty` forces it and `off`
  keeps the half-block cells everywhere. Sixel is not yet supported. Fetching the bytes is a client-level operation today
  (`ProtoClient::download_media`, capped at 16 MiB, refused rather than
  truncated when it is over): the attachment is re-read by identifier, because a
  description is rebuilt on every page and a cached locator would be stale.
  `o` on a media message in Normal mode opens it in the platform viewer:
  `open -W` on macOS, `xdg-open` on Linux, `cmd /C start` on Windows (untested).
  The file comes from the media cache: a message opened before is served from
  disk with no request, and a first open downloads it and stores it there. The
  cache is a directory of `0600`, plaintext files named by the sha256 of their
  bytes (`<sha256>.<ext>`), so messages with the same bytes share one file. Two
  kinds of pointer name a file: `<chat>-<message>.ref` for each message, and
  `m<id>.ref` for each Telegram media id, which a forwarded copy shares. The
  directory is kept beside the config as `televim.media` unless `media_cache_dir`
  (or `TELEVIM_MEDIA_CACHE_DIR`) names another. It keeps at most
  `media_cache_max_bytes` bytes (or `TELEVIM_MEDIA_CACHE_MAX_BYTES`; default 1 GiB)
  and 256 pointers. A file over the byte cap is refused. Past either limit the least
  recently used pointer goes first, and its file with it once no pointer names it.
  An edit drops its message's pointer; a deletion drops the pointer for that id in
  any chat. The directory is emptied when the account changes and on sign-out. If that directory cannot be
  written, this run uses `televim.media-<account>` under the system temp directory
  instead, with the same limits; the configured directory is left as it is and the
  next launch tries it again. Files saved by earlier
  versions under the temp directory are not migrated or removed; the OS reclaims
  them. The terminal leaves the alternate screen and raw mode for the viewer and
  comes back where it was. A file over the 16 MiB limit is refused and nothing is
  saved, and the status line says what happened. While a download runs, its row shows `[image… 42%]` beside
  the placeholder (the megabytes so far when Telegram declared no size); `Esc` on that
  message stops it, and a failed download shows `[failed: …]`. There is no viewer setting. The known limits of the
  hand-off are in [`docs/known-gaps.md`](docs/known-gaps.md).
- **Message Composition:** `i`/`a` to compose, `Enter` to send, `Esc` to stop
  typing and a second `Esc` to leave — nothing typed is ever lost to an `Esc`.
  The line is a real editor (`vim-line`, wrapped in `tui::line`): caret
  movement, `w`/`b`/`e`, `x`, `dw`, `cc`, `p`, `gg`/`G`, a visual selection with
  `d` and `y`, and multi-line messages with `Ctrl+J` (`Shift+Enter` where the
  terminal volunteers the distinction). `gg` and `G` are the wrapper's, not the
  library's — see [`docs/decisions.md`](./docs/decisions.md). The bar is always a draft: it grows to six
  rows, and each conversation keeps its own: leaving parks it, re-entering
  restores it, and another conversation starts with its own bar. Quitting keeps
  them too: the loop writes `televim.drafts.json` beside the config whenever the
  drafts change, so a relaunch — even after a `kill -9`, which loses at most one
  250 ms tick — puts the words back. It is drawn with
  a painted caret that `TestBackend` can assert, which the real terminal's could
  not.
  A `:shortcode` opens a completion popup above the bar: `↑`/`↓` choose a
  candidate, `⇥`/`⏎` accept one, `Esc` closes the popup, and every other key
  keeps typing into the draft. Reply with `r`, edit with `e` — and since a
  reply carries the message it quotes, `gd` on one goes to that message and
  `Ctrl-o` brings the reader back.
- **Text direction:** the draft in the bar is drawn by the same rules as an
  incoming message. In the default mode the row is emitted as it is stored and the
  terminal's shaper reverses a right-to-left run; in the opt-in `BidiMode::Visual`
  (`bidi = "visual"`) this program applies the permutation instead, for the whole
  draft and every row of a wrapped one, and the caret and the selection are marked
  on the cells their logical positions land on. The `: ` and `/` prefix is chrome
  and is never permuted.
- **Send / edit / delete:** one message with `d`, or every message a selection
  covers in Visual, with a confirmation before deleting. The prompt counts, says
  which side the messages are from, and says how many were left out because they
  are placeholders. Delete removes for both sides. More than a hundred messages is
  several requests with a pause between them, and a failure part-way through is
  reported as how many went through rather than as a plain failure.
- **Forward:** `f` in Normal forwards the message under the cursor, or the current
  selection if there is one; `s` in Visual forwards the selection. Either raises a
  picker of your chats over the conversation. `j`/`k` or the arrows choose a chat,
  `Enter` queues the forward into it and clears the selection, and `Esc` puts the
  picker away and keeps the selection. The forward stays in the current chat: the
  destination is not opened, and the status line reports how many went through. The
  messages keep their authors and captions, as Telegram renders a forward. Placeholders
  are left out and counted, and more than a hundred messages go in several requests.
  Telegram can refuse a forward from a chat that protects its content; the status
  line then names that chat, and nothing checks for it before the send. The picker
  walks the chat list only, and leaves out deleted accounts, which stay in the list.
- **Search:** `/` searches the loaded window, then asks the server and prefers
  its answer. `n` repeats or cycles. This is the *conversation's* search; the
  chat list has its own `/` that finds a person instead — see *Starting a
  conversation with someone new* — so the two never shadow each other.
- **Typing indicator:** the conversation title grows a dim `· typing` note while
  the peer is composing one. It ends when they cancel, when their message lands,
  or six seconds after the last sign of it, and it is furniture on the title
  rather than a message row, so it never displaces a message; when the title has
  no room it is dropped whole rather than truncated. Advertising our own typing is
  not built.
- **Online / last seen:** the conversation title shows the open peer's status as a
  dim `· online`, `· last seen today`, `· last seen yesterday`, `· last seen on <day
  or date>`, or — for a peer who restricts it — `· last seen recently`, `· last seen
  within a week` or `· last seen within a month`. A contact's card carries the same
  words on one `status` row. A peer who hides presence shows nothing: no title note
  and no row. The title note yields to the typing note while the peer types, and is
  dropped whole when the title has no room. The status is sticky: it stands until
  the next update for that peer, with no expiry, so it can be stale. The chat list
  shows no presence.
- **Yank / paste:** `y` in Visual puts the selection in a register — the selected
  characters for a text selection, one line per message oldest-first for a set of
  them — and `p` in Normal opens the line with it. A yank is also offered to the
  system clipboard with OSC 52, best-effort and truncated to 74 kB of sequence;
  the register is the half that always works. The register is cleared by opening
  another conversation. `p` is not bound in Visual. The line keeps its own
  internal yank buffer for its own `p`, fed by its own `y` and `d`; the two
  registers are deliberately not shared, because their formats differ.
- **Commands:** `:q`/`:quit`, `:chat <id>`, `:new <query>`, `:search <query>`,
  `:settings`, `:signin` and `:retry`.
  `:retry` re-runs the bring-up — the client, the chat list, the feed — and is how
  a launch that exhausted its three chat-list attempts is cleared without a
  restart; while a bring-up is already in flight it says so and does nothing.
  `q` in the conversation and `:q`/`:quit` both raise `Quit televim? (y/n)` first
  — the same screen-wide confirmation a deletion uses — because `q` is one
  keystroke away from a key that types nothing else. `y` quits, `n` or `Esc`
  stays. `Ctrl-C` does **not** ask: it is the way out when the program is wedged,
  and a terminal that is not answering cannot draw the question either.
- **Reconnect:** if the update feed stops mid-session, televim rebuilds the client
  from the stored session in-process and takes a new feed — no restart and no
  re-login — while keeping the reader's place: the open conversation and its
  cursor, the chat-list highlight, the jumplist, the selection, the register and
  the draft all survive. The status line says `reconnecting` while it happens, and
  one automatic reconnect is tried; if the feed stops again before any update
  arrives, the status line says `offline:` and `:retry` asks again. A `●` beside
  the sentence always shows which holds: green while the feed delivers, yellow
  while a bring-up or rebuild is under way, red once the budget is spent.
- **History cache:** media bytes are not in this file; they are in the media cache
  (see **Media** above). The newest 200 messages of each of the 32 most recently
  touched private conversations, and the first 500 rows of the chat list, are kept
  in `televim.history.json` beside the config — plain JSON, `0600`, written
  atomically and off the loop's thread, behind every page, feed event and list
  change. A launch with a warm cache draws the cached chat list and the first
  conversation (or the `--chat` one, when the cache holds it) **before any network
  round trip**, under `connecting…`; the `Ready` that follows refreshes the list
  around the reader, and the newest page replaces the cached rows, with
  `Cached messages — loading the latest…` on the status line while it is on its
  way. Every page the client fetches is merged into the cache, and an edit, a
  deletion or an arrival over the feed is folded in too. If the launch ends at
  `offline:`, the cached list and the cached conversations stay readable and
  switchable, and the status line still says `offline:`. Text and the media
  *kind* are kept; media bytes, presence and unsent placeholders never are. The
  file is used only by a launch configured with the same phone it was written
  under, and is removed on sign-out and when a launch finds no session. It is
  **not encrypted** — see [`docs/known-gaps.md`](./docs/known-gaps.md).

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
- **`:w`** — not a command. The commands are `q`/`quit`, `chat <id>`,
  `new <query>`, `settings`, `signin` and `retry`.
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

## Installing

Each release is on the [GitHub Releases](https://github.com/curedbylethe/televim/releases)
page, with a binary per platform and a `SHA256SUMS` file. Binaries are built for
Apple silicon macOS (`aarch64-apple-darwin`) and Linux on x86_64
(`x86_64-unknown-linux-gnu`). On Intel macOS, build from source (see
[Building](#building)).

On Linux, the binary links DBus for the credential store. Install the runtime
library first, for example `sudo apt install libdbus-1-3` on Debian and Ubuntu.

```console
$ VERSION=0.2.0      # the release you want, without the v
$ TARGET=aarch64-apple-darwin      # or x86_64-unknown-linux-gnu
$ BASE=https://github.com/curedbylethe/televim/releases/download/v$VERSION
$ curl -LO $BASE/televim-$VERSION-$TARGET.tar.gz
$ curl -LO $BASE/SHA256SUMS
$ sha256sum --ignore-missing -c SHA256SUMS      # shasum -a 256 on macOS
$ tar -xzf televim-$VERSION-$TARGET.tar.gz
$ install -m 755 televim ~/.local/bin/televim   # any directory on your PATH
```

`SHA256SUMS` confirms the download matches what the release published. It comes
from the same place as the binary, so it does not prove who built it.

### Credentials

televim signs in as a Telegram application. The release binaries are built with
the maintainer's `api_id` and `api_hash`, so they run with no setup. The
credentials are not secret from someone who looks inside the binary, so you can
use your own instead. Register an application at
[my.telegram.org](https://my.telegram.org) and set either:

```console
$ export TELEVIM_API_ID=1234567
$ export TELEVIM_API_HASH=0123456789abcdef0123456789abcdef
```

or, in a `televim.toml` in the directory you run it from:

```toml
api_id = 1234567
api_hash = "0123456789abcdef0123456789abcdef"
```

Either overrides the built-in pair. Pass another config path with
`televim --config path/to/televim.toml`. The drafts and history files are written
beside the config file. The login session goes to the OS credential store unless
`session_path` is set.

## Building

```console
$ make ci      # fmt-check lint boundary test design-check build-release
$ make run     # the debug binary
$ make watch   # the same, under cargo-watch
```

The planned shape of what is left here is worked out in `~/.opencode/plan/`.
