# Known Gaps

Real, and named so they are not mistaken for oversights:

- **The typed update path cannot say "media `grammers` did not decode".** Every
  message description carries `media: Option<MediaKind>`, and the classification
  is tested on both paths as free functions (`classify_raw` for the raw
  `GetHistory` fetch, `classify_typed` for the `grammers` feed). On the typed
  path there is still one hole: `grammers`' `Media::from_raw` returns `None` for
  a handful of variants, and this build reads no wire type behind that `None`. So
  a message whose media `grammers` declines to build arrives as *no media at all*,
  where the raw history path would have said `File`. The two answers are the same
  for everything modelled and disagree only for media `grammers` itself does not
  carry. Closing it means classifying the raw field on the feed path as well,
  which the crate's two-path split exists to avoid — the update feed arrives as a
  built `grammers` `Message`, and there is no raw variant to match on there
  without re-deriving one.
- **A media download is a whole `Vec<u8>`, and nothing in the interface calls it.**
  `Client::download_media` and `ProtoClient::download_media` exist and are tested,
  but no key is bound to them: a fetched attachment has nowhere to go yet — no
  cache directory, no viewer, no save path — and a program that asked for one
  before it could put the bytes somewhere would be guessing. The return type is
  the other half: streaming, and a cache on disk, are CUR-9 and CUR-10, and the
  16 MiB `MEDIA_LIMIT` is what a viewer will have to do something about rather
  than merely report.
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
- **The specimen's frame set does not include the post-logout state.** The design
  run added it to the engine — a `loggedout` start state with six scenes, from the
  resting signed-out screen through signing back in — but `components.html`'s
  frames are a hand-maintained list (`design-system/build-specimen.js`'s
  `CARD_FRAMES`), and nothing there renders one of them yet, so the document stops
  at `logout: the confirmation`. Nothing in it is wrong: that frame's keys are
  `SGd`, which stop before the `y` that signs out. Adding the frame is one entry in
  that list plus `make design-specimen`.
- **`Config::code` and `Config::password` are pre-fills, not a way in.** They
  fill the code and the password fields when a flow reaches them; the flow is the
  only path that writes a session, and the phone has to reach the bar once.
- **A chat-list fetch that runs out of retries is still the reader's to clear.**
  The launch fetch is bounded at `CHAT_LIST_ATTEMPTS` = 3 and waits
  `backoff(&error)` between attempts — Telegram's own flood wait when it gave one,
  the fixed `RETRY` = 5s otherwise — so a transient refusal no longer ends the
  launch at `offline:`, and the status line names the reason, the wait and the
  count while it waits. What is left is the exhaustion: past the bound the
  bring-up is a failure again, and the only way out is `:retry`, which re-runs it
  with a fresh budget. A launch that spends all three attempts before the reader
  can type anything still needs a restart. A *corrupt stored session* remains the
  one bring-up failure recovered rather than reported: `bring_up` discards it and
  carries on to the sign-in path with a status sentence, so an unreadable session
  ends at ` Phone ` rather than at `offline:`. An unreachable store is still the
  `offline:` line and has never been recoverable.
- **The update feed neither backs off nor re-subscribes.** `pump` loops on
  `updates.next()`, and an `Err` is `tracing::warn!`ed and carried past — no sleep,
  no attempt count; the feed stays usable and resumes where it left off. When the
  stream *ends*, the task stops: the position it reached is recorded by
  `finish()` and nothing asks for the feed again, so a launch that ends up in
  `offline:` never reaches a feed and a session that stops mid-stream does not get
  a new one. This is a separate request from the chat-list fetch, which is the one
  thing a launch cannot start without.
- **Two sign-in hints in the Rust differ from the engine's text.** The bar's field
  hint is ` ⏎: send  Esc: cancel` and the waiting hint is
  ` Checking… — the request is in flight`, where the design model reuses its
  insert hint and the single word `Checking…`. A login code is one row with no
  newline to type and `Shift+⏎` is refused there, so the insert hint would name a
  key that does nothing; the waiting hint spells out the state the design leaves
  to the status line.

- **Visual mode's `r` refuses rather than replying.** `v`, `V`, `o`, `Esc`, `y` and
  `d` work and a selection is drawn. `r` in Visual says no, for one of two reasons —
  see [`decisions.md`](./decisions.md). Neither `domain::utf16_len` nor the rendering of an
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
- **A jump cannot return to a not-yet-sent placeholder.** `domain::history`
  numbers an outgoing placeholder below zero, and `proto::history`'s `narrow`
  rejects an identifier the wire could not carry, so a page fetched around one
  comes back empty and `apply_jump` refuses it. A reader who jumps away from a
  message that has not been sent and presses `Ctrl-o` therefore cannot land on it:
  the return is armed anyway — nothing on this side of the boundary can know a
  fetch will come back empty — and the landing says `That message is no longer
  available.` and leaves the cursor where it was. This is a missing identity
  rather than a missing fetch, since a placeholder has no server-side identifier
  to fetch around, so it is pinned by a test
  (`a_return_to_a_placeholder_says_so_and_leaves_the_reader_put`) rather than
  worked around.
- **`Ctrl-i` is `Tab` on the wire, so forward navigation needs a terminal that
  reports the two apart.** `Ctrl-i` and `Tab` are the same byte unless the
  terminal speaks the kitty keyboard protocol or `modifyOtherKeys`; crossterm then
  delivers `KeyCode::Tab`, which is the pane switch and is answered before the
  conversation sees a key. Only a CONTROL-modified `i` moves forward, so on such a
  terminal `Ctrl-i` does not navigate and `Ctrl-o` alone returns — see
  [`decisions.md`](./decisions.md). `Tab` is not rebound: a key that is a motion
  in one place and a pane in the next is a key a reader has to learn twice.
- **A feed dropped without `finish` persists a stale update position.** See
  [`decisions.md`](./decisions.md).
- **No figure measures the program under load.** The 50 MB ceiling is
  measured ([`memory.md`](./memory.md)) on two processes, and neither is the
  loaded one: the harness holds 60 chats and a drawn screen but none of the
  network half, and the binary is the whole program with an empty chat list.
  The figure for 50 chats in the real binary sits between 3.41 MB and 8.25 MB
  and has not been taken: the chat list now retries and `:retry` re-runs the
  bring-up, so a launch with a session can populate it, but `make measure` drops
  every `TELEVIM_*` and `TELEGRAM_*` name on purpose — the binary is launched
  with no credentials, so it builds no client — and nothing has yet weighed it
  with one. The allocator question itself is closed rather than open: the
  global allocator is the system allocator, chosen on the measured margins, and
  no arena pays — see [`decisions.md`](./decisions.md).
- **No benchmarks and no working PTY tests.** `app/tests/tui_e2e.rs` is
  `#[ignore]`d placeholders awaiting `termlens`. `make measure` is a
  measurement harness, not a benchmark suite: it reports what a run costs, and
  nothing compares two implementations.
- **`tui/src/event.rs` is unwired.** `key_to_action` is not called.
- **A peer with no bare identifier is skipped**, and the skip is unreachable
  today. See [`decisions.md`](./decisions.md) for why, and which test guards it.

- **xterm and alacritty render RTL wrong in the default mode.** The default emits
  logical order and leaves the reordering to the terminal, which is the right
  answer for a terminal that shapes — kitty, wezterm, foot, iTerm2, VTE — and the
  wrong one for the two that do not, because nothing reverses the run for them.
  The opt-in `BidiMode::Visual` applies the permutation here instead, and is
  reachable as `bidi = "visual"` (`TELEVIM_BIDI=visual`); it is one value for the
  machine rather than one per terminal, so an ssh hop into a shaping terminal
  keeps the setting it was launched with. Per-`TERM` is the v2 hook; see
  [`decisions.md`](./decisions.md).
- **Arabic and Persian letters are not contextually joined, and reordering cannot
  join them.** Nothing in this program performs script shaping, so in the
  opt-in `Visual` mode on a non-shaping terminal the letters render unjoined and
  the text is still not right typographically. The follow-up is a shaping
  library, which is a strictly larger piece of work than reordering and cannot
  defeat a terminal shaper anyway — a terminal consumes code points into cells
  and there is no glyph-placement protocol.
- **Rule L4 glyph mirroring is absent.** Neither bidi direction applies it, so
  `( ) [ ] { } < >` are not mirrored inside an RTL run; punctuation attaches by
  *position*, which is what the bidi algorithm delivers. The follow-up is a
  mirroring table added alongside a bidi pass, if a reader ever asks for the
  glyphs and not only for the order.
- **The default mode's correctness cannot be proved in this repository.**
  `ratatui::TestBackend` has no bidi algorithm and no shaper, so a screen-level
  test can show that a permutation reached the cells and nothing about how a real
  terminal drew the emitted bytes. `Visual` is the machine-verifiable half and
  `Terminal` rests on the terminal matrix in [`decisions.md`](./decisions.md);
  the follow-up would be an end-to-end harness driving a real terminal, which
  needs `app/tests/tui_e2e.rs` un-stubbed first. The input bar's right-to-left
  tests inherit the same ceiling whole: they drive `BidiMode::Visual` only, and
  the default path is pinned by the ASCII tests beside them, which cannot tell a
  shaper's reorder from no reorder at all.

- **The new-chat prompt's prefix and the status sentence's punctuation differ
  between the model and the binary.** The revised design run agrees with the
  binary on the invocation (`/` on the chat list, `:new <query>`), the
  submit-time list, the `New chat` titles, the `j`/`k`/`↑`/`↓`/`⏎`/`Esc` keys,
  the `chat`/`new` standing, and the `match` ink — but two small drawing details
  remain. The model draws the prompt line with **no prefix**, beside `Find`'s `/`
  and `Command`'s `:`; the binary draws the shared `/` prefix, because the person
  search is the message search's idiom and the two are told apart by what answers
  them rather than by a second glyph (`App::prompt_prefix`). And the model writes
  the status sentence with an ASCII hyphen and three dots — `/query -
  searching...` — where the binary uses the design system's own em dash and
  ellipsis — `/query — searching…` — the punctuation the conversation's search
  label has always used (`SearchState::label`). Each is a one-line change in the
  binary, but neither is a hand edit to `DESIGN.md` or the engine
  (`design/README.md`); a design run settles them.
- **The new-chat row order and a new chat's place in the list are the binary's
  own.** The model sorts its local candidates closest-match, then
  no-conversation-first, then by name, and inserts a new conversation at the top
  of the list; the binary draws the candidates in the order the lookup returned
  them and appends a new chat (`ChatList::ensure_private_chat`, which leaves the
  existing order alone rather than guessing a comparator). Both are properties of
  the lookup and the chat list rather than of the widget.

## v2 Hooks

The architecture leaves clear extension points for future features: a notification daemon (via `notify-rust`), file upload/download (using `tokio::fs` and `reqwest`), or a plugin system (using `wasmtime` for sandboxed extensions). Because the `domain` layer is pure, adding these features won't require touching the protocol or UI layers. Media download is the half that arrived first: the fetch path exists and returns bytes, and the `tokio::fs` cache and any viewer are what is still ahead of it.
