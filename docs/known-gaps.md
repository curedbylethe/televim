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
- **A media download is a whole `Vec<u8>`, saved whole to the media cache.**
  `Client::download_media` and `ProtoClient::download_media` return the bytes, and
  `o` stores them as one file in the media cache before the platform viewer opens
  it. Progress is reported per chunk and `Esc` stops the transfer, but the bytes
  still arrive whole, so the return type is the limit: streaming to disk is a later
  change (CUR-9), and the 16 MiB `MEDIA_LIMIT` refuses rather than truncates.
- **The media cache has four known limits.**
  - *Same-account sign-back-in clears the cache.* Sign-out removes the account tag,
    so signing back in under the same phone re-tags an empty directory. Kept on
    purpose: the cache is the account's, and sign-out ends the account.
  - *An unwritable cache directory moves the cache to the temp directory for one run.*
    The fallback is per launch and not persisted: the configured directory is never
    rewritten or migrated, and the next launch probes it again. If the temp fallback
    is unwritable too, `o` says the media could not be cached.
  - *Legacy temp files are left.* Files named `televim-<pid>-…` from before the cache
    are not migrated or swept; the OS temp cleanup reclaims them.
  - *The history file does not hold media ids.* A message restored from the history
    file before any refetch carries no id, so it is looked up by its `(chat, message)`
    key alone. A forwarded copy in another chat is served from disk only in the live
    session, until that message is fetched again. The history file format is not
    changed for this.
- **The viewer hand-off has four known limits.**
  - *Stdin race.* The loop blocks on the viewer with the terminal released, but the
    reader thread keeps calling `crossterm::event::read`. A key it captures during
    the suspend window is queued and then dropped on resume, so it reaches neither the
    viewer nor the program. Input already queued is dropped the same way; network
    events are kept.
  - *Network stalls while the viewer is open.* The runtime is current-thread and the
    loop is parked in the wait, so no network task runs until the viewer exits. A long
    viewing can let the connection go quiet.
  - *The opener's exit, not the viewer's.* macOS waits only because of `-W`. `xdg-open`
    usually returns once a GUI handler is launched, so on Linux the terminal can come
    back under a running viewer, and the sentence `Viewer exited (code …)` reports the
    opener's exit code.
  - *Windows is untested.* `cmd /C start` returns without waiting, so the terminal is
    back at once there. No Windows leg runs in CI.
- **Global search shows only the first page, capped at 100 hits.** `?` and
  `:search` ask the server once. The overlay keeps the oldest 100 matches, and the
  status line says `100 of <total>` when there are more; there is no way to page
  past them (server pagination is not built). There is no local pre-pass, so a
  match that exists only in the local cache and is not on the server's first page
  does not appear.
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
- **Three PTY tests are deferred, `#[ignore]`d with reasons.** In
  `app/tests/tui_e2e.rs`: AC5 `an_arrival_scrolls_a_pinned_view`, AC6
  `an_arrival_leaves_a_scrolled_back_view_alone`, and AC7
  `a_fetch_in_flight_is_shown_at_the_edge_it_is_coming_from`. The warm-start cache
  loads once, and arrivals and fetches enter only as network events, so no offline
  run can produce them, and no seam injects them. Their bodies are empty. The
  follow-up is a seam (an issue is to be filed). The other PTY tests (launch,
  seeded chat list, INSERT typing, `:q`, scrolling) run in the normal gate.
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
  emulator, not through a shaping terminal. The input bar's right-to-left
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

- **The open draft row is not a cursor stop.** It takes no caret, selection or
  motion, and the cursor never rests on it. Acting on it from the panel, reaching
  it and resuming the draft there, is CUR-25 and is not built; until then the bar
  is the only place a draft is edited.
- **The draft row is shown only while following the newest message.** Scrolled
  up, it is not drawn and reserves no rows, because it is drawn after the slice
  and would sit under an older message, misplacing the draft. The input bar still
  shows the words while scrolled up.
- **The draft row is not in `DESIGN.md`, and its look is DESIGN-TBD.** It is built
  to the existing `[you]` tag and `text-dim` vocabulary; no OpenDesign artifact
  was commissioned for it. `DESIGN.md` mirrors the OpenDesign project, so an entry
  there is a `make design-push`, which edits the project, and it waits for that call.
- **The draft row's `BidiMode::Visual` path has no test.** `draft_items` permutes
  the words the way `message_row` does, but no test in `widgets/conversation.rs`
  sets that mode on a draft; the draft tests all run in the default mode.
- **The session file's key has no prompt.** The key is `TELEVIM_SESSION_PASSPHRASE`
  or the OS keyring and nothing else, so a machine with neither ends at `offline:`
  and the reader sets the variable and types `:retry`. An interactive launch-time
  prompt is the deferred alternative: it needs a new prompt kind, a widget and a
  design pass. There is also no re-keying — changing the passphrase makes the old
  file unopenable, and the way out is to remove it and sign in again.
- **A wrong passphrase is never recovered, only reported.** An envelope that does
  not open is `offline:` with the file untouched, by design (see
  [`decisions.md`](./decisions.md)); a reader who has lost the passphrase for good
  removes the file by hand.
- **Legacy plaintext is still read, and its wipe is best effort.** A file from an
  older build loads and is encrypted in place, and the old bytes are zero-filled —
  on Unix only, and not past SSD wear levelling, a journaling or copy-on-write
  filesystem, a snapshot or a backup. Dropping the plaintext reader is a dated
  follow-up, not part of this change.
- **The expanded AES key schedule is not zeroized.** The `FileKey` and the
  passphrase are wiped on drop; the round keys inside the cipher are not reachable
  to wipe.
- **The 19 MiB Argon2id allocation is declared, not measured.** `make measure` runs
  with no credentials, so no RSS figure includes it; see [`memory.md`](./memory.md).
- **Live wire behaviour for restricted presence is unconfirmed.** The mapping of
  `UserStatus::Recently`, `LastWeek`, `LastMonth` and `Empty` (to `UserPresence`
  `Recently`, `LastWeek`, `LastMonth` and `Hidden`) is unit-tested against the schema,
  but what Telegram actually sends a peer who hides last-seen is not. The gate cannot
  check it without a datacenter. `proto_integration` does not assert presence today,
  so a hand run (`TELEVIM_TEST_DC=1 … cargo test --all --all-features --test
  proto_integration`) proves the suite passes, not the presence shape. Confirm by
  hand against a restricted account, and extend the suite if the answer differs.
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
- **The chat list shows no presence.** This is deliberate: presence reaches the
  screen through the open conversation's title and one contact-card row only. Chat-list
  rows are not decorated, and `widgets/chat_list.rs` has no test module to put a row
  assertion in.
- **The peer presence map is unbounded per session and unmeasured.** `UiState::peer_presence`
  gains one entry for each distinct peer that reports presence and is never evicted
  for the length of the session. It is not measured against the memory budget
  ([`memory.md`](./memory.md)).
- **The history file is not encrypted at rest.** `televim.history.json` is plain
  JSON restricted to its owner (`0600`), the drafts file's precedent; the session
  file is sealed, and the drafts and the history are not. Anyone who can read the
  reader's files can read the newest 200 messages of up to 32 conversations and
  the chat list's previews. Sealing both with the session file's key is the
  follow-up — see [`decisions.md`](./decisions.md).
- **The history file's account tag is the configured phone, not the signed-in
  account.** The tag is `cfg.phone`, the same as the drafts', so two accounts
  signed in one after the other under a launch that configures no phone (or the
  same phone) share the tag. A `Ready` with no session removes the file, and
  sign-out removes it, which covers the usual way of changing account; a session
  swapped underneath a launch by hand is not covered.
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
- **`Ready` moves the focus to the conversation while the reader is typing under
  `connecting…`.** A warm launch lets the reader type into the cached
  conversation's draft before the wire answers; the draft is kept across the
  `Ready`, but the focus is settled by it, so the line can lose focus mid-word.
- **The history cache's worst case is past the memory budget.** Typically about a
  megabyte; with every cached message at Telegram's maximum length it is about
  80 MB in memory and a write briefly needs about three times that. The figures
  and the upgrade path (a byte budget beside the row bounds) are in
  [`memory.md`](./memory.md). The feed's per-peer *seen* map is outside the peer
  bound and grows by one small entry per peer that received an arrival in the
  session.
- **No history or chat-list cache for groups or channels.** Only private
  conversations are cached, because only private conversations are shown — the
  product's scope, not a limit of the store. Media bytes are not in the history
  file: they are in the media cache, and the history keeps only the media *kind*, so
  a cached `[image]` row draws its token and nothing more.

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

## v2 Hooks

The architecture leaves clear extension points for future features: a notification daemon (via `notify-rust`), file upload/download (using `tokio::fs` and `reqwest`), or a plugin system (using `wasmtime` for sandboxed extensions). Because the `domain` layer is pure, adding these features won't require touching the protocol or UI layers. Media download is the half that arrived first: the fetch path exists, and `o` stores the bytes in the media cache and opens the platform viewer. Streaming to disk is what is still ahead of it.
