# Known Gaps

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
- **A feed dropped without `finish` persists a stale update position.** See
  [`decisions.md`](./decisions.md).
- **No figure measures the program under load.** The 50 MB ceiling is
  measured ([`memory.md`](./memory.md)) on two processes, and neither is the
  loaded one: the harness holds 60 chats and a drawn screen but none of the
  network half, and the binary is the whole program with an empty chat list.
  The figure for 50 chats in the real binary sits between 3.41 MB and 8.25 MB
  and has not been taken, because reaching a populated list offline needs a
  product change. The allocator question itself is closed rather than open: the
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
  The follow-up is the opt-in `BidiMode::Visual`, which applies the permutation
  here instead; see [`decisions.md`](./decisions.md).
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
  needs `app/tests/tui_e2e.rs` un-stubbed first.

## v2 Hooks

The architecture leaves clear extension points for future features: a notification daemon (via `notify-rust`), file upload/download (using `tokio::fs` and `reqwest`), or a plugin system (using `wasmtime` for sandboxed extensions). Because the `domain` layer is pure, adding these features won't require touching the protocol or UI layers.
