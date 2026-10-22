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
- **The media cache has five known limits.**
  - *Same-account sign-back-in clears the cache.* Sign-out removes the account tag,
    so signing back in under the same phone re-tags an empty directory. Kept on
    purpose: the cache is the account's, and sign-out ends the account.
  - *An unwritable cache directory moves the cache to the platform cache directory.*
    The fallback is not the configured directory — that one is never rewritten or
    migrated, and the next launch probes it again. The fallback lives under
    `dirs::cache_dir()`, so it survives a restart; the system temp directory is used
    only when no cache directory can be found, where the cache does not survive and
    the warning says so. If the chosen fallback is unwritable too, `o` says the media
    could not be cached.
  - *Legacy temp files are left.* Files named `televim-<pid>-…` from before the cache
    are not migrated or swept; the OS temp cleanup reclaims them.
  - *A viewer's file is the one exception to a clear.* A path handed to the viewer is
    held until the viewer reports (`ViewerDone`) or fails to start. Eviction and `clear`
    leave the file on disk; a sign-out or account change does not wait for the viewer.
    The file is removed on release, or by the orphan scan on the next launch if the
    program ended first. Where the opener returns before the viewer does (Linux,
    Windows), the hold ends with the opener, not the viewer.
    A store that reserved before a `clear` and commits or fails after it leaves a held
    file alone too; the release removes it.
  - *Two instances sharing one cache directory are still not supported.* The launch
    sweep removes a `.download-<pid>-<n>.tmp` only when the pid in its name is this
    process or its last write is over 24 hours old, so a second instance's in-flight
    download survives the first one's open. A name that does not parse is kept. `clear`
    still removes every download temp by name, as a deliberate wipe, so a sign-out in
    one instance can still fail the other's download at its rename, and two live
    instances on one cache directory remain an unsupported mode.
- **The viewer hand-off no longer blocks the loop, and has four known limits.** `o`
  runs the platform opener on a thread of its own, so the loop keeps running while
  the viewer is open: network events are applied, and queued media opens one viewer
  at a time. When the wait ends the terminal comes back once, to the same
  conversation and the same cursor, with no restart. Ctrl+c is the emergency exit
  only: it is not needed to return, and no signal handler is installed, so pressing
  it while a viewer holds the terminal still ends the program.
  - *Reader gating.* The keyboard reader is gated for the whole viewing, so keys typed
    into the viewer are the viewer's. A key the reader took just before the gate
    closed is held, in arrival order with any focus event read alongside it, and
    delivered to the program once the terminal is back, not to the viewer. The one
    byte that can still reach the wrong side is one the reader had already taken
    from the terminal in the instant the gate closed; that race is accepted. A test
    pins the gate open, closed for the viewing and open after, and the held order.
  - *Network during a viewing.* The wait runs on a thread of its own, so the loop
    never parks in it and network events are applied while the viewer is open; a test
    sends one mid-viewing and checks it lands. The runtime stays current-thread, so a
    long viewing is serviced between loop passes, not in parallel with them.
  - *The opener's exit, not the viewer's.* macOS waits only because of `-W`. `xdg-open`
    usually returns once a GUI handler is launched, so on Linux the terminal can come
    back under a running viewer, and the sentence `Opener exited (code …); the viewer
    may still be open` reports the opener's exit. A configured viewer command (`viewer`,
    `viewer_image`, `viewer_video`, `viewer_audio`) is run directly and waited on, so
    its sentence reports the viewer's own exit; only the platform default keeps this limit.
  - *Windows is untested.* The default is `cmd /C start "" /WAIT`, which waits for the
    started process, so its sentence reports the viewer's exit. That wait has not been
    run on Windows: no CI leg runs there, and the release check is the manual steps in
    the `OPENER` doc comment in `crates/app/src/runtime.rs`. A target that detaches
    itself can still return early; a configured viewer command is the escape hatch.
- **The global search has no sender filter.** `from:`, `sender:` and `by:` are
  refused by the prompt's parser, each naming the word as typed (`from:ada is not
  a filter here: global search has no sender filter.`), and the prompt reopens with the line still in it
  so the token can be removed. Only `messages.search` with `peer = inputPeerEmpty`
  takes a `from_id`, and it is a different method: its own paging, no chat
  grouping, and it does not carry the `users_only` scope. So a sender filter is a
  second request shape and a page model for it, not another token in the parser.
  Closing it means building that path, or Telegram adding `from_id` to
  `messages.searchGlobal`.
- **A bot's direct messages are searched but not shown.** The global request asks
  for `users_only`, so the server answers with a bot's messages too, and the
  client drops them and counts them in `, N dropped`, because the chat list holds
  people only and a hit is opened by selecting its chat there. Closing it means
  listing bots, which touches the chat pane, the update filter and the history
  cache; see "Why a bot's hits are dropped, for now" in `decisions.md`.
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
- **The join of a `userFull` to its `user` is unasserted.** Every decision it makes
  is tested at the level it can be written at — an empty `about` is not a bio, an
  empty username is not a username, a date that is not a date is dropped, a
  `userEmpty` is not a profile — but the two objects are not joined in a test.
  Layer 227's `tl::types::UserFull` is 58 fields with no `Default`, so the fixture
  would be a 58-field literal that breaks on every schema bump and reads as noise.
  The `live` integration test in `app/tests/proto_integration.rs` is the other end
  of it, and it needs a datacenter.
- **A second account can be added, but the process holds one account at a time.**
  `add account` on the self card and `:add-account` detach the signed-in account
  without signing it out — the client goes, the files stay — and open the sign-in
  at the phone step for a new one; the new account's first `Ready` adopts it, and
  `Esc` at the phone step cancels and restores the first. That `Ready` also puts
  the reader back: the conversation open at detach is reopened by peer id — never
  by row, and never by name — with the new account's newest page, so a reader who
  had scrolled away lands at the newest message and the line says `newest`. When
  the new account has no chat with that person, nothing is opened, the highlight
  goes to the top row and the status line says `that account has no chat with
  this person`. Nothing else crosses the switch — no message cursor, draft,
  register or scroll position, because message ids are per account. Four limits
  remain. Only one account is open per process: the detached account receives no
  updates while the new one signs in. Switching is add-account-only until the
  picker lands, so reaching an account already stored means signing in to it
  again. Nothing is revoked on the server, so the detached session remains
  valid. And with a configured session *file*, the panel names the configured
  path behind the sign-in until the first `Ready` names the new account and its
  suffixed path is known.
- **Account storage is partitioned, and one account is open per process.** The
  session, drafts file, history file and media directory resolve
  through `accounts::slot_paths`, so an account added later writes
  `<stem>.<id>.*` and `session-<id>` and cannot read, overwrite or delete the
  first account's bytes; the first account keeps the legacy names and the
  legacy slot's tag, so an upgrade moves nothing and clears nothing. A launch
  opens the slot the accounts index names, and `add account` writes a second one
  while the program runs, but there is still one client and one account's state
  at a time, and no picker to move between the slots already stored. `add
  account` is the only switch — see the entry above.
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
- **The update feed's stream end reconnects, and recoverable feed errors back off
  and re-subscribe.** `pump` loops on `updates.next()`. A recoverable `Some(Err)`
  is waited out in the pump's task up to `CHAT_LIST_ATTEMPTS` = 3, sleeping
  `backoff(&error)` between attempts — Telegram's own flood wait when it gave one,
  the fixed `RETRY` = 5s otherwise — and each wait is reported as
  `Event::FeedRetrying`, naming the reason, the wait and the count, so the reader
  sees the wait coming. Past the bound the position is recorded with
  `UpdateSubscription::finish` and `Event::FeedEnded` is reported, and when the
  stream itself *ends* (`None`) the same two things happen without the wait. Either
  way `net::drive` rebuilds the client from the same stored session and takes a new
  feed in-process — no restart and no re-login — through the `state.bringing_up`
  single-flight guard the reader's `:retry` already uses. The rebuild is a new
  `Client` because the update relay is single-shot (see
  [`decisions.md`](./decisions.md)); the resulting `Ready` is place-preserving, so
  the open conversation and its cursor, the chat-list highlight restored by id, the
  jumplist, the selection, the register and the draft all survive, and the stale
  `state.history` anchors are cleared so paging re-anchors from the preserved
  window rather than wedging. One automatic reconnect is allowed per working feed:
  a feed that ends again before any update has arrived is a persistent `offline:`
  the reader clears with `:retry`. What remains is live-path verification: a real
  end or error cannot be provoked from the repository and the opt-in datacenter
  suite cannot tell a dead feed from a quiet one, so the backoff and the reconnect
  are verified by unit tests over `App` and `State`. The boundary with the
  overlapping reliability issues, recorded so they cannot silently re-tread each
  other: **CUR-45** owns a connection-state indicator (a `●` beside the ranked
  sentence — green while the feed delivers, yellow while a bring-up or a
  rebuild is under way, red once the budget is spent — while the sentences
  keep saying what happened in words); **CUR-49** owns a manual `:reconnect` command
  (there is none); **CUR-47** owns queued in-flight sends (none are queued). This
  is a separate request from the chat-list fetch, which is the one thing a launch
  cannot start without.
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
- **Only `W`, `B`, `E`, `j` and `k` *behind an operator* in the input refuse
  non-ASCII text.** They say no with a flash, because `vim-line` counts those
  motions in bytes and `d`/`c` slice in the same key. `w`, `b` and `e` on
  non-ASCII text are the wrapper's own, through `domain::vim::char_motion`, so
  `dw` deletes and `cw` changes; ASCII-only text keeps `vim-line`'s motions.
  `h`/`l` and the arrows step by painted column on non-ASCII text, from the rows
  the last frame measured, so a resize is one frame late.
- **The input line's `w`, `b` and `e` break at a ZWNJ (U+200C) on non-ASCII text.**
  A Persian compound typed in the bar is two words to these keys, as it is in a
  message. A word motion that has nowhere to go stays put, as in a message, where
  `vim-line` would run to the last character. See [`decisions.md`](./decisions.md).
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
- **`Ctrl-i` is `Tab` on the wire, so forward navigation carries a second key.**
  `Ctrl-i` and `Tab` are the same byte unless the terminal speaks the kitty
  keyboard protocol or `modifyOtherKeys`; crossterm then delivers `KeyCode::Tab`,
  which is the pane switch and is answered before the conversation sees a key.
  Forward is therefore bound twice — `Ctrl-i` where the terminal reports it apart,
  and `Ctrl-n` always — so on a terminal that cannot tell the two apart, `Ctrl-n`
  moves forward and `Ctrl-o` returns — see [`decisions.md`](./decisions.md). `Tab`
  is not rebound: a key that is a motion in one place and a pane in the next is a
  key a reader has to learn twice, so it keeps its precedence as the pane switch.
- **A feed dropped without `finish` persists a stale update position.** See
  [`decisions.md`](./decisions.md).
- **No figure measures a connected session under load.** The 50 MB ceiling is
  measured ([`memory.md`](./memory.md)) on three processes: the harness (60 chats
  and a drawn screen, none of the network half), the binary with an empty chat
  list, and, since the seeded scenario, the binary with a sealed 60-chat cache
  loaded (`rss_idle_populated_binary_bytes`, 7.55 MB median of five runs, 125
  samples between 4.23 and 9.34 MB). That last one is the real program drawing a
  real list, launched credential-free so it builds no client. What is still not
  taken is a session connected to Telegram with fetched history, because
  `make measure` drops every `TELEVIM_*` and `TELEGRAM_*` credential name on
  purpose. The allocator question itself is closed rather than open: the global
  allocator is the system allocator, chosen on the measured margins, and no arena
  pays, see [`decisions.md`](./decisions.md).
- **One candidate is benchmarked; it is not in production.** The gap that "nothing
  compares two implementations" is retired for the harness: `make bench` (criterion
  micro-benches, A/B compare report; see [`testing.md`](./testing.md)) compares a
  declared `reference`/`candidate` pair in one run, and one revision against a saved
  baseline. One pair exists: `word_prefix_match`, with its byte-scan candidate in
  `crates/domain/benches/search.rs`. Production still runs only the reference, so no
  one has yet replaced it. Every other hot path (`wrap`, the history window, the
  character motions) is measured as a workload with no candidate. So a claim that a
  candidate should replace production code has no full-mode result behind it until
  that change is made. `make measure` still measures one tree and does not compare.
- **The arrival and in-flight-fetch PTY cases are covered only in a debug
  build.** In `app/tests/tui_e2e.rs`, `an_arrival_scrolls_a_pinned_view`,
  `an_arrival_leaves_a_scrolled_back_view_alone` and
  `a_fetch_in_flight_is_shown_at_the_edge_it_is_coming_from` replay their state
  through a scenario file: `TELEVIM_SCENARIO` names a script, the test writes
  `<path>.fire`, and the binary sends each directive through its event channel.
  That seam is `#[cfg(debug_assertions)]`, so the shipped binary carries neither
  the module nor the variable, and `cargo test --release` ignores those three
  cases with that reason. The normal gate and CI use a debug build, so the
  coverage exists there; a release-profile PTY run is the half that does not.
  The other PTY tests (launch, seeded chat list, INSERT typing, `:q`, scrolling)
  run in both.
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
  the follow-up was an end-to-end harness driving a real terminal. That harness now
  exists (`app/tests/tui_e2e.rs`, a pty through `termlens`), so the follow-up is
  unblocked for the screen-level half. It still cannot show how the user's own
  terminal shapes or draws the bytes: `termlens` reads the screen through its
  emulator, not through a shaping terminal, and `make test` proves no shaping.
  The shaping check is manual: `make rtl-shots` (not in `make ci`, no assertions)
  has a `lam_alef_note` scene, `سلام` with a trailing clock in the default mode,
  where a person reads `target/rtl-shots/lam_alef_note.txt` for the last letter
  and a clock that is not on top of it. The width it rests on is pinned in
  `make test`: `columns("لا")` is 1 and `drawn_cells("لا")` is 2, and the Arabic
  trailing note's column is asserted from `drawn_cells` in all three modes. The input bar's right-to-left
  tests inherit the same ceiling whole: the glyph-order tests drive
  `BidiMode::Visual` only, and the default path is pinned by the ASCII tests beside
  them, which cannot tell a shaper's reorder from no reorder at all. The caret tests
  after an Arabic draft run in both modes and check cell positions, not glyph shape.
  The reversed modes rest on one observed behaviour: kitty 0.49.2 keeps a cell's
  background on its emitted column while it reverses the glyphs, so `emitted_caret`
  styles the emitted cell whose index is the caret's visual column. Checked by eye
  on kitty (`make rtl-shots`) at the start and end of an Arabic draft, in `terminal`
  and `words`; a middle position and any other shaping terminal are not checked.
  `words` is written for a terminal that reverses each word on its own, and is wrong
  on one that reverses the whole line, where the words would be reversed twice.
- **The search's new design frames are byte-synced, not scene-checked.**
  `design/global-search-states.html` and `global-search-states-frames.js` are
  drawn from the rules in `DESIGN.md`'s global-search section, not as scenes of
  `televim-engine.js`, so `make design-check` does not render them:
  `design/scripts/check-model.js` validates the engine's scenes, and
  `scripts/design-sync.sh check` then compares the vendored copies with the
  OpenDesign project byte for byte. A frame that drifts from its own rules is
  therefore caught only when the OpenDesign copy disagrees, and an edit made on
  both sides is checked by nothing. Closing it means teaching the checker a second
  frame format, which is a new harness rather than a change to this one.

- **The draft row is shown only while following the newest message.** Scrolled
  up, it is not drawn and reserves no rows, because it is drawn after the slice
  and would sit under an older message, misplacing the draft. The input bar still
  shows the words while scrolled up.
- **The draft row is not in `DESIGN.md`, and its look is DESIGN-TBD.** It is built
  to the existing `[you]` tag and `text-dim` vocabulary; no OpenDesign artifact
  was commissioned for it. `DESIGN.md` mirrors the OpenDesign project, so an entry
  there is a `make design-push`, which edits the project, and it waits for that call.
- **A passphrase typed at the prompt is not remembered.** When the session file has
  no key or a refused one, the launch asks for the passphrase (and `:unlock` asks
  again after `Esc`); it is held in memory for that process and never written, so
  every launch asks again until `TELEVIM_SESSION_PASSPHRASE`, `session_passphrase`
  or the OS keyring has the key. `:passphrase` re-seals the session file and the
  history cache under a new passphrase, but it leaves the keyring's `file-key`
  entry alone, so a reader who relied on the keyring is asked at the next launch.
  The Argon2id cost keys (`session_kdf_*`) are still not recorded in the envelope:
  a file opens only under the cost it was sealed with, and a changed value reads as
  a wrong passphrase. `:passphrase` seals under the configured cost but opens under
  it too, so it cannot move a file from one cost to another.
- **A wrong passphrase is retried, and a lost one is not recovered.** A passphrase
  the file refuses leaves it untouched and asks again; there is no attempt limit,
  since nothing is written and a guess costs the reader only the key derivation.
  Nothing recovers a passphrase that is lost for good: the reader removes the file
  by hand and signs in again. A file that cannot be read at all is a separate,
  permissions-worded `offline:` sentence, and asks for no passphrase.
- **Legacy plaintext is still read, and its wipe is best effort.** A file from an
  older build loads and is encrypted in place, and the old bytes are zero-filled —
  on Unix only, and not past SSD wear levelling, a journaling or copy-on-write
  filesystem, a snapshot or a backup. The plaintext reader is removed after
  2026-12-31: from then on `FileStore::load` treats a file that is not an
  envelope as corrupt instead of reading and re-sealing it, and a reader still on
  such a file signs in again. Until then it is read as described; nothing is
  removed yet.
- **Signing out deletes the keyring entry and no more.** `KeyringStore::clear`
  deletes the `session` credential and counts a missing one as success. The OS
  credential store owns the bytes it keeps, so it may leave a backup, a version
  or a free-list copy that this repo cannot reach, and the in-memory mirror is
  grammers-owned, so its bytes cannot be wiped either; only the wrapper's own
  `AuthKey` values are zeroed on drop. What is proven is the logical state: after
  a sign-out the store, the update counters, the peer cache and the account are
  empty (`clearing_a_used_session_resets_counters_peers_and_the_stored_copy`), and
  the client is dropped, which aborts its runner, so no grammers session or socket
  outlives it. There is no integration test that watches a socket close.
- **Key wiping cannot reach copies made before a `Drop`.** The `FileKey`, the
  passphrase, the cipher's intermediate keys and its expanded round keys are all
  wiped when they drop (the `zeroize` features on `aes-gcm-siv` and `aes`). A copy
  the compiler or CPU made on the way, in a register or a stack slot, and the live
  key in a process dump taken while a session is open, are not reachable.
- **The 19 MiB Argon2id allocation is declared, not measured.** `make measure` runs
  with no credentials, so no RSS figure includes it; see [`memory.md`](./memory.md).
- **Live wire behaviour for restricted presence is unconfirmed: observer landed,
  awaiting run.** The mapping of `UserStatus::Recently`, `LastWeek`, `LastMonth` and
  `Empty` (to `UserPresence` `Recently`, `LastWeek`, `LastMonth` and `Hidden`) is
  unit-tested against the schema, but what Telegram actually sends a peer who hides
  last-seen has not been observed. The gate cannot check it without a datacenter.
  `presence_on_the_wire_for_a_restricted_peer_is_observed` in `proto_integration`
  prints the resolved bucket and the wording the UI would show for the account's own
  profile and for a peer's (`own status` / `peer status` lines); it asserts no
  particular bucket. Run only that test, with a second account that hides its
  last-seen sharing a private conversation with the first, since each test requests
  its own login code and Telegram throttles hard:

  ```console
  $ TELEVIM_TEST_DC=1 TELEVIM_API_ID=… TELEVIM_API_HASH=… \
    TELEVIM_TEST_PHONE=+… TELEVIM_TEST_CODE=… \
    TELEVIM_TEST_PEER_PHONE=+… TELEVIM_TEST_PEER_CODE=… \
    cargo test --all --all-features --test proto_integration \
    presence_on_the_wire_for_a_restricted_peer_is_observed -- --nocapture
  ```

  Not yet run. Paste the two lines here when it is. The mapping and wording change
  only if the observation contradicts them.
- **A missed peer read is recovered only when the chat list is fetched.** The
  live `updateReadHistoryOutbox` receipt is best-effort: Telegram delivers each
  update to one randomly chosen active session, so a session can miss it. The
  chat list's per-dialog outgoing read position (`read_outbox_max_id`) recovers
  it, and it is folded into the record on each bring-up that fetches the list
  (launch, reconnect, sign-in), with no receipt in flight. A read the peer makes
  while this session is running and the feed misses is therefore not shown until
  the next bring-up. Private chats only: groups and channels stay out of scope.
- **The dialogs read position is unverified against a live account.** Its
  mapping is unit-tested against the pinned layer-227 schema, and `proto_integration`
  does not assert it. Whether Telegram fills it for a private peer is shown only by
  an opt-in run (`TELEVIM_TEST_DC=1 cargo test --all --all-features --test
  proto_integration`), which has not been run with that field asserted. Until it
  is, the recovery's live behaviour is inferred from the schema.
- **Presence is sticky and can go stale.** A peer's status stands until the next
  `PeerStatus` for them. There is no expiry, and Telegram's `expires` deadline on
  `Online` is ignored, so a peer who goes offline without an update still reads
  `online` until one arrives. Revisit on a reader complaint, not before.
- **The chat list shows no presence, by decision.** Presence reaches the screen
  through the open conversation's title and one contact-card row only. Rows stay
  undecorated for three reasons: `UiState::peer_presence` holds only peers who
  signalled this session, so a fresh launch would mark a few rows and leave the
  rest blank; `DESIGN.md` specifies no presence on the list, so showing it is a
  design change first; and the row already carries title, unread count and pin.
  `a_presence_update_leaves_every_row_unchanged` in `widgets/chat_list.rs` pins
  it. Reversing it is a design-first item (`DESIGN.md`, the OpenDesign sheet, a
  specimen), then one `Span` in the row closure and its tests.
- **The peer presence map is unbounded per session, measured, and left unbounded.**
  `UiState::peer_presence` gains one entry for each distinct peer that reports
  presence and is never evicted for the length of the session. At the harness's 60
  peers it is invisible (+16 KB, one page, in `make measure`); a throwaway run puts
  it at about 38 to 71 B a peer, so the +1.05 MB regression allowance is crossed
  only at roughly 15,000 to 28,000 peers. No bound is justified at a realistic peer
  count; revisit if a flow can name peers beyond the chat list
  ([`memory.md`](./memory.md#the-history-cache)).
- **The media cache is not encrypted at rest.** The directory holds the bytes of
  the attachments a reader has opened and the previews their messages carry, as
  plaintext `0600` blobs named `<sha256>.<ext>`, beside pointer files that name
  them — `<chat>-<message>.ref`, `m<id>.ref` and `<chat>-<message>.thumb` — and
  the fallback directory `televim.media-<account>` under the platform cache
  directory is the same. The history file is already sealed with the session file's
  envelope, and the follow-up here is the same one: reuse the
  sealed envelope the session file already uses — `TVIM1 | salt(16) | nonce(12) |
  ciphertext | tag(16)`, AES-256-GCM-SIV — so a downloaded file is ciphertext at
  rest too (see [`decisions.md`](./decisions.md)). Building that is not this
  change's work. Until it is, the media directory should be excluded from backups
  by default.
- **A newest page requested on an old client can mark a run current that missed
  messages.** A latest page asked for before a reconnect and landing after the
  reconnect's `Ready` is counted as this session's newest page, so a message the
  dropped feed never delivered between the page and the new feed can be absent
  from a run the cache now calls current; the next arrival is then appended past
  the hole. A later latest page whose stretch covers the hole repairs it.
- **An edit or a deletion that arrives while an older newest page is in flight is
  overwritten until the next one.** The page was fetched before the event and
  speaks for its whole stretch, so merging it puts back the pre-edit text or the
  deleted row, in the cache and in the window it replaces. The next latest page
  corrects both.
- **An empty newest page leaves the cached rows in place.** A conversation cleared
  on the server answers with no messages, and the merge treats an empty page as no
  answer, because a fetch that short-circuited looks the same. So the cleared
  conversation's cached rows stay on screen until something else replaces them,
  and stay in the file.
- **Cached chat-list previews and unread counts are only as fresh as the last
  write.** A launch draws them as they were when the file was last written, and
  the `Ready` replaces them; between the two, a count can be wrong. Presence is not
  cached at all, so a cached list draws none until the feed reports it.
- **`Ready` no longer moves the focus while the reader is typing under
  `connecting…`.** A warm launch lets the reader type into the cached
  conversation's draft before the wire answers; the draft is kept across the
  `Ready`, and with a conversation open and the input line focused, so is the
  focus (`ready_leaves_input_focus_with_a_conversation_open`). With no
  conversation open, with a sign-in form up, or with a `:` / `/` prompt on the
  line, the `Ready` still settles focus as before.
- **Focus regain is proven against state, not a live terminal.** On `FocusLost`
  and `FocusGained` the event loop clears the pending key latches, and a gain asks
  for the keyboard flags again. Whether a given terminal reports focus, and whether
  it resets the flags on blur, is not checked: nothing has run in one. Suspend
  (`SIGTSTP`/`SIGCONT`) is not handled; it is job control, not a window blur.
- **A worst-case history-cache write peaks above the 50 MB budget.** The byte
  bound counts JSON-encoded text, so the serialised text is at most 8 MiB; but the
  in-crate worst-case test (6,400 rows of control characters, release) measured a
  60.4 MB process peak around a write, from a 9.8 MB baseline, and 62.7 MB on
  re-open (re-run at the baseline re-record: the same 60.4 MB and 62.7 MB; it is
  not part of `make measure` and has no band) ([`memory.md`](./memory.md#the-history-cache)). The write clones the
  cache for the encoder, then holds the serialised bytes, the sealed envelope and
  the re-opened verification copy at once, and the passphrase provider's Argon2id
  adds 19 MiB while sealing (the Argon2 share is inferred, not profiled). Follow-up,
  not done: seal and verify in place and drop the encoder's clone, to cut those
  copies. Until then a full cache of pathological text can exceed the budget
  briefly during a write; ordinary text is far smaller and was not measured here.
- **The history cache's feed bookkeeping is not byte-bounded.** The feed's per-peer *seen* map is outside the peer
  bound and grows by one small entry per peer that received an arrival in the
  session.
- **No history or chat-list cache for groups or channels.** Only private
  conversations are cached, because only private conversations are shown — the
  product's scope, not a limit of the store. Media bytes are not in the history
  file: they are in the media cache, and the history keeps the media *kind* and its
  Telegram media id, so a cached `[image]` row draws its token and nothing more, and
  a row restored from the file still finds its file in the cache.

- **Sixel is not implemented.** Sixel draws in pixels, so fitting the 24-by-8
  box needs the terminal's cell pixel size, and getting that is a probe the
  program does not run. Kitty graphics is the only protocol `graphics` selects.
- **Kitty pictures hide the cursor and selection under them.** In `kitty` mode
  the block's cells are blank and the picture covers them, so the reverse video
  of a cursor or selection shows only on the margins and the tag row, not on the
  picture. The half-block mode paints both.
- **Kitty placement is re-sent on every pass.** `tui::graphics::frame` clears
  and re-places each visible picture on every loop pass, transmitting its base64
  again. Not measured on a live terminal; a diff against last frame's placements
  is the upgrade path if the traffic shows.
- **A picture the panel cuts off is not drawn in `kitty` mode.** A block only
  partly on screen shows blank cells instead of the cut picture, because the
  terminal cannot draw part of one over the rows outside the panel.
- **The kitty path has no live-terminal proof.** It is covered by byte-exact
  encoder tests and one widget test that checks the blank cells and the recorded
  placement. Nothing has run in kitty, ghostty or WezTerm.
- **Half-block picture rows after the first start one tag-width left of row 0.**
  Read from `sticker_block_row`, not from a test: only block row 0 is padded by the
  tag, so rows 1 to 7 start at column 0 while row 0 starts after the tag. The
  `kitty` placement follows row 0's column, as the picture's first row does.
  Not changed here.
- **An animated sticker has no inline picture, not even from its thumbnail.**
  Telegram's preview for a TGS or WebM sticker is an SVG outline (`PhotoSize::Path`)
  or a stripped JPEG; the workspace decodes only WEBP, and adding a JPEG or SVG
  decoder is a new dependency and a design decision. The token is the representation;
  the thumbnail the `o` job already persists is not read by any draw path. Upgrade
  path: a bounded JPEG decoder for `Stripped`, or rasterising `Path`, plus a decision
  about the block's alpha.
- **An animated sticker's playback is the viewer's, not the program's.** `o` hands
  the cached `.tgs` or `.webm` to the generic `viewer` key, else the platform opener.
  There is no Lottie or VP9 decode here; a TGS usually has no default player, and a
  WebM plays only if the configured program supports it. `viewer_video` is
  deliberately not used, because a TGS is not a video.
- **An animated sticker whose bytes are neither gzip nor EBML is cached as `.bin`.**
  The first four bytes decide `.tgs` (`1f 8b`), `.webm` (`1a 45 df a3`) or `.bin`; an
  unreadable temp also falls back to `.bin`. The file is still opened by the viewer
  key, but a platform opener that dispatches on the extension cannot know the format.
- **Sticker sending and the sticker picker are not built.** The send half needs
  upload and `messages.sendMedia`, which CUR-11 covers; no wrapper for either exists
  in this tree. The two lists are scoped under `## v2 Hooks` below, and no Normal-mode
  key is bound until the picker can send. `:stickers` is not a command in this build.
- **Forwarding has no live proof in CI.** The batching and the error mapping are
  unit-tested, and the key path is driven through the `tui` app tests with
  action assertions. No test sends a forward to Telegram: the live round
  trip is deferred, and `proto_integration.rs` names it among its deferred cases.
  The opt-in run (`TELEVIM_TEST_DC=1 cargo test -p televim --test proto_integration`)
  does not exercise forwarding. The upgrade path is a Saved Messages round trip in
  that file.
- **A content-protected source is refused only from the wire.** Nothing reads the
  chat's `noforwards` flag before the send, so the picker opens over a protected
  chat and the refusal arrives after `Enter`, as `CHAT_FORWARDS_RESTRICTED`, which
  the status line reports as `<chat> does not allow forwarding`. The upgrade path is
  to read the flag and refuse in `begin_forward`, next to the placeholder refusal.
- **There is no `tui_e2e` keystroke test for the forward flow.** The `tui_e2e.rs`
  tests cover launch, seeded chats, typing, `:q` and scrolling, and none covers
  forwarding. The flow is asserted instead by the key-drive tests in
  `crates/tui/src/app/tests.rs` and the picker's `TestBackend` tests in
  `widgets/forward_picker.rs`, which is the layer that exists in this tree.
- **The forward picker walks the chat list, not a shared destination picker.**
  Epic 3's destination picker is not in the tree, so the picker is a chat-walker
  over `ChatListState`, and the person picker in `user_list.rs` is not reused, since
  it resolves people rather than chats. When Epic 3 lands, the picker should
  converge on its interface rather than keep a second one.
- **Times are shown in one offset: the reader's current zone, applied to every message.** The
  zone is read once per frame in `app`, so a DST change is picked up on the next frame, but
  every message on screen takes the offset in force now, including one sent before the change.
  Per-message historical zones are not built, because the wire carries zone-neutral unix
  seconds and no zone to read them from. There is no bundled tz database: the offset is the
  system's own zone as `time` reads it, and a platform that will not say falls back to UTC.
  The offset is read on each pass rather than cached, so it costs one `localtime_r` per frame.
  A change of zone within a running process (a `TZ` change, or a system zone change while the
  program is open) is not exercised by any test; the per-frame read is expected to follow it,
  but that path is unverified.
- **Pinned order is positional and only refreshed by a full fetch.** The order of the
  pinned block is the order Telegram sends the first page of `messages.getDialogs` in.
  The schema has no order field on a dialog and the official documentation is silent,
  so this rests on how mtcute, TDLib and tdesktop behave, not on a written contract.
  `updatePinnedDialogs` and `updateDialogPinned` are not handled, so a reorder or a
  pin made on another device shows only on the next full chat-list fetch, and
  `messages.getPinnedDialogs` is not called. A pin made here goes to the front of the
  block until that fetch says otherwise.

- **The chat and contact actions are unproven against a live account.** Archive
  and block are raw TL calls (`folders.EditPeerFolders`, `contacts.Block`/
  `Unblock`) because `grammers` wraps neither, and the archive folder id is the
  convention of `1` rather than a published constant. The both-sides form of
  `:delete-chat` sends `messages.DeleteHistory` with `revoke: true`, and whether
  a peer sees the conversation disappear is not something an offline test can
  answer. `:report` is likewise unexercised: reporting a real account is
  irreversible, so the live suite skips it. Contact browsing reads
  `contacts.GetContacts` whole, with no pagination, so a very large directory is
  one response.
- **Archive, block and folder updates from another device are not folded.** The
  update feed models messages and presence; `updateFolderPeers`,
  `updatePeerBlocked` and `updatePeerSettings` are dropped, so a change made
  elsewhere appears on the next fetch that carries it, not when Telegram sends
  it. A local name lives only in this process and its history file.
- **The archived list is fetched once per launch, not followed.** `:archived`
  draws what the last `messages.getDialogs { folder_id: 1 }` returned; an
  archive made on another device is not pulled in until the next launch,
  sign-in or `:retry`.
- **The archive is one page of 100.** The request has no offset, so an archive
  of more than 100 conversations shows the newest 100 (pinned first) and the
  rest cannot be reached, or unarchived, from `:archived`. Paging needs an
  offset peer built from the last row.

## v2 Hooks

The architecture leaves clear extension points for future features: a notification daemon (via `notify-rust`), file upload/download (using `tokio::fs` and `reqwest`), or a plugin system (using `wasmtime` for sandboxed extensions). Because the `domain` layer is pure, adding these features won't require touching the protocol or UI layers. Media download is the half that arrived first: the fetch path exists, `o` streams the bytes into the media cache, and the platform viewer opens it.

**The sticker lists, scoped.** Sending a sticker arrives with upload and
`messages.sendMedia` (CUR-11); both lists are deferred until then, and this is what
they will mean when it lands.

- **Sendable list.** The stickers in the account's installed sets, grouped by set.
  Telegram supplies them (`messages.getAllStickers` for the installed sets,
  `messages.getStickerSet` for a set's stickers) when the picker opens. It is not a
  local folder, not the media cache, and not a set the account has not installed.
- **Recent list.** Telegram's recent-stickers list (`messages.getRecentStickers`),
  most recently used first and shared across the account's devices. It is not a local
  MRU and is not derived from sent history.
- **Surface.** One picker with the two sections; `:stickers` opens it, `Enter` sends,
  `Esc` closes. A Normal-mode key is not bound until sending exists.
- **Out of scope.** Authoring or favouriting stickers, installing sets, searching a
  set, persisting the lists locally, and any Lottie or VP9 rendering.
