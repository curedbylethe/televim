# Key Decisions

Each entry is a decision and the reasoning behind it. They move verbatim from
the file this repository used to keep them in, because a record of reasoning
loses its value the moment it is paraphrased. The rules that follow from them
are in [`../AGENTS.md`](../AGENTS.md).

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
- **Why the session file is written atomically:** a session file is a permanent
  authorisation key, and a plain `fs::write` over it is a window where the file on
  disk is the *new* session truncated to however many bytes reached the disk — a
  kill, a full disk or a closed terminal in that window leaves a 0-byte file that
  the next launch cannot tell from a real one. So `FileStore::save` writes to a
  sibling temp (`<file_name>.<pid>.tmp`), restricts it to `0600` *before* a byte is
  in it, and `fs::rename`s it over the target: the target only ever appears as a
  whole file, replaced by a finished one, so a kill mid-write leaves the previous
  session or no session and never a partial one. The temp is a sibling rather than
  a scratch directory because a cross-filesystem rename is not atomic and degrades
  to a copy, and it is tagged with the process id so two processes sharing a
  session path do not write the same temp. Renaming rather than writing in place is
  also what keeps the mode: the target inherits the temp's inode and the
  permissions it was restricted to instead of being a fresh file behind the umask.
  A failed write removes the temp, since a half-written one is not a session file
  and nothing else would clean it up. It is `std` only — no temporary-file crate,
  no dependency, and nothing to audit beyond four calls.
- **Why a corrupt session is discarded at bring-up and the store is not asked to
  reset:** the store's contract is to *report* corruption rather than paper over it —
  `a_truncated_snapshot_is_reported_rather_than_reset` in `session.rs` is the test
  that holds it, and a store that silently dropped what it could not parse would be
  a store saying a session is gone when it is not, which is the one thing a
  credential store cannot afford to get wrong. So recovery is the app layer's
  decision: `bring_up` resolves `session_store(cfg)` *once*, probes it with
  `discard_corrupt_session`, and hands that same value to the client, so a reader
  cannot be told their session came from one store while a different file is the
  one being cleared. Only `SessionError::Corrupt` is recovered. `Load` and
  `Unavailable` stay fatal, because a store that could not be *reached* is a
  failure of the machine rather than bytes to throw away, and a sign-in prompt
  would be offering an answer that could not work — that reader is better served by
  the `offline:` line that names what happened. The answer they do get is a
  persistent status sentence, sent as `Event::SessionDiscarded` **after**
  `Event::Ready`: `Ready`'s sign-in path clears the status line, so a sentence sent
  first would be overwritten before it was ever drawn, and the one account of what
  happened to the session they were signed in with has to survive a minute.
- **Why the chat-list fetch is retried three times, why the wait is the one
  history paging already uses, and why `:retry` is a command:** the launch fetch
  ran once, and one `?` turned a single transient refusal into a terminal
  `offline:` — while Telegram rate-limits a client that has been off for a while,
  so the first request after a launch is the one most likely to be refused for
  something that passes on its own. The loop is **bounded** at
  `CHAT_LIST_ATTEMPTS: u8 = 3` rather than open, because every attempt is another
  request against a session Telegram may have decided against, and a client that
  retries forever is a client that never says it is offline; three is the bound
  the two-factor password already gets, and it buys two waits. The wait is
  `backoff(&error)` — Telegram's own when it asked for one, the fixed `RETRY` = 5s
  otherwise — because a chat list refused for having been asked too often has
  exactly the same answer as a page that was, and a second schedule here would be
  one more constant nobody has measured. Both facts live in the pure function
  `chat_list_retry(attempts_used, &error) -> Option<Duration>`, so the bound is
  asserted without a client or a datacenter, in the same style as
  `a_fetch_that_failed_holds_every_direction_until_its_backoff_passes`. Automatic
  retry covers the launches the reader is not present for; the `:retry` command
  covers the ones they are, because a bound that can only be cleared by restarting
  the program is one a reader on a bad network pays on every launch. The two do
  not overlap — a `:retry` while a bring-up is in flight is refused with a
  transient status, so a manual attempt cannot double the budget into two loops —
  and only exhaustion still reaches `Event::Offline`. The retry is the **launch**
  fetch only: the same fetch after sign-in tolerates failure by design, because
  reporting it there would wipe the account the reader had just earned, so it is
  left exactly as it was. The sentence sits in the persistent `app.status`, the
  rank the `offline:` line holds and above a `flash`, rather than in one: a wait
  the reader was not watching is still a wait when they look back, and the
  sentence has to name the reason, the wait *and* the count for that to be
  something other than a silence — see the ranking decision below.
- **Why sign-in cannot report itself in flight without a client:** `waiting` is the
  panel's claim that a request is on its way, and it is a claim about something
  outside the screen: it puts `Checking… — the request is in flight` on the panel,
  makes a second `⏎` say so and send nothing, and leaves a reader no way to tell a
  slow answer from a request nobody is carrying. So it follows the *dispatch*, and
  `App::client_available` (default `false`) records whether there is a client to
  carry one — `net.rs` sets it on `Ready` and clears it on `Offline` and
  `LoggedOut`. Without a client, `submit_signin_field` flashes `not connected yet —
  the client is not up` and returns: the draft stays in the line, because a client
  coming up is not a reason to type it twice, and nothing is queued, because a
  queued login would fire on its own if a client appeared later and that is a
  sign-in attempt nobody asked for. Losing the client clears `waiting` for the same
  reason, and touches the draft not at all.
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
- **Why drafts are per conversation:** a draft belongs to the conversation it was
  typed in, so leaving one parks its `LineEditor` on `App` under the outgoing peer
  id and selecting a chat restores it — no words cross between readers, and
  re-entry finds the bar as it was left. It is a `tui` view-state map keyed by
  peer id, like `read_receipts`, not a `domain` history structure: `domain` keeps
  one flat message window and no per-conversation list, and a store that must
  reach disk would have to live in `app`, which owns the configuration path
  (AGENTS.md dependency rule) — so the map stays in `tui` and the file lives
  in `app`. It has no fixed cap, because evicting a live draft to
  satisfy one would lose the reader's words, the failure the feature exists to
  prevent; instead an entry is dropped when its draft becomes empty, so the map
  holds only peers with text in the bar. The draft's subject (`reply_to`,
  `editing`) is still dropped on a switch, because it lives on `App` rather than
  in the `LineEditor` and restoring a reply target is a separate product decision;
  only text, caret, mode and selection travel. Across restarts the map is
  persisted as plain `(peer id, text)` pairs in `televim.drafts.json` beside
  the configuration: every loop pass snapshots the parked map plus the live
  line and rewrites the file when the snapshot changed — only text crosses
  that boundary, no cursor, undo or purpose — through an atomic sibling-temp
  rename at `0600`, so a kill loses at most one 250 ms tick (and a kill in
  the pass after a send resurrects the sent line, accepted). The file carries
  `cfg.phone` as its account tag and a launch for another account discards
  it; signing out removes it.
- **Why the measurement harness is an `[[example]]` and not a binary or a test:** `cargo build --release` does not build examples, so a harness that is one cannot reach the shipped artifact, and `app` is bin-only so its own event loop is unreachable from `app/tests` anyway. An example is also the only vehicle that needed no change to `[profile.release]`, which is load-bearing for the binary-size budget and out of bounds for measurement work. The fixture is built through `tui`'s public API rather than its `#[cfg(test)]` sample data because that data is invisible to every dependent crate: a harness that compiled only under `cfg(test)` could not be measured as a release build.
- **Why RSS is read in-process rather than by a profiler:** neither `valgrind`, `heaptrack`, nor `hyperfine` is installed on the development host, so a harness that required one could only be run in CI. Reading the process's own resident size — `proc_pidinfo` on macOS, `/proc/self/status` on Linux — is the kernel's number for this process rather than a profiler's estimate, needs no dependency, and makes the harness a build target rather than a machine with tools on it. `valgrind --tool=massif` is still used automatically where it exists, and the choice of source is recorded in the report so two hosts' figures are not silently compared. The cost is that the macOS figure carries the VM's page-in schedule with it — one run in fifteen lands near 1.5 MB rather than 3.4 MB, with all five samples inside a run agreeing exactly — so a threshold taken from it alone would report the OS rather than the program. The noise floor is recorded so that is visible before anyone draws one.
- **Why the harness records both a within-run and a cross-run spread:** the two answer different questions and only one of them is about this program. Within a run the samples measure the process settling; across runs they measure the machine the process started on. Quoting the first as the noise floor would understate it by a factor of two, and a threshold drawn from it would fire on the host.
- **Why the idle RSS is reported as two figures and not one:** neither candidate process is the program under load. The harness holds 60 chats and a fully drawn screen but none of the network half; the shipped binary is the whole program but with an empty chat list, because a populated one needs a Telegram round trip and the measured run drops every credential name on purpose. Averaging them, or picking one and calling it the measurement, would produce a number that describes no process that exists. Both are recorded, and the range between them is the honest state: the figure for 50 chats inside the real binary is bounded from both sides and has not been taken.
- **Why the timing probes are gated on an environment variable rather than a flag:** adding a flag would add a user-visible surface to satisfy a measurement, and the probes' cost in an ordinary run — one `var_os` per event and an `Option` that stays `None` — is smaller than the branch the loop already takes. They write to the log file beside the configuration, never the screen, because a `tracing` line written mid-frame lands on the display this program is in the middle of drawing.
- **Why no arena allocator is used:** measurement found no path where one pays. The render pass allocates per frame and every byte it takes is dropped at the end of that frame, so an arena would move allocation *within* a pass it cannot outlive — it cannot lower the 3.41 MB harness RSS or the 8.25 MB the shipped binary holds, because neither number is carried by the per-frame arena's contents but by the long-lived window, the chat list, and the terminal buffers. On time the margin is larger than the effect available: a frame costs 0.518 ms against the 16 ms input-latency budget, about thirty times over. And the plumbing does not close. `App::row_layout(&self) -> Vec<RowSpan>` (`tui/src/app.rs:2231`) is a public method on an immutable borrow, and `conversation::render(app, area, frame, layout: &[RowSpan])` (`tui/src/widgets/conversation.rs:52`) is public and takes its layout as a slice; giving either an arena means a lifetime reaching through both, into the widget API, for no measured return. That is the whole finding, and it is recorded rather than assumed: the candidate sites are the outer `Vec<RowSpan>` plus one `Vec<Range<usize>>` per windowed message (`tui/src/app.rs:2231-2295`), the per-frame `Vec<ListItem>` and title strings in `conversation::render`, `rows::reply_prefix`, called by the layout (`tui/src/rows.rs:680`) and again by the painter (`tui/src/widgets/conversation.rs:502`), and the fetch/translate triple `Vec<Message>` path (`telegram-framework/src/history.rs:172`, `proto/src/history.rs:151-164`, `domain/src/history.rs:182-192`). The arena owns none of them. MTProto decoding, the largest allocation anyone might name, happens inside `grammers` and is not this workspace's to change. An arena does not fix fragmentation or cache behaviour either, and the harness cannot see either — so a future profile that shows a fragmenting heap is the thing that would reopen this, not a preference for bump allocation.
- **Why the global allocator is the system allocator:** measurement left nothing to buy. The alternatives were `tikv-jemallocator` and `mimalloc`, and against the ceilings they are already inside with room: the stripped release binary is 5,610,320 bytes against < 15 MB, and RSS at idle is 3.41 MB for the harness and 8.25 MB for the shipped binary against < 50 MB — two to three times of margin on each, so a candidate would have to give some of it back to be chosen. Neither the runtime nor the workload is the one those allocators are for: the runtime is a single-threaded current-thread `tokio` plus one reader thread, and what they target is many threads allocating against a fragmenting heap, which this is not. And the choice could not be measured here: macOS RSS moves by roughly a factor of two between runs with no code change, so a candidate's delta on this host is inside the noise rather than a result. `std::alloc::System` is the one choice that is the same on macOS, Linux and Windows and needs no C toolchain on any of them, which is what makes it consistent across every supported platform (AC-16); a bundled-jemalloc build is a different story on each. The declaration that would have wired this — `tikv-jemallocator` in `[workspace.dependencies]` — had no dependents in any crate and no entry in `Cargo.lock`, so it was a name in a manifest for a decision nobody had made, and it is gone rather than left to imply one. Were a `#[global_allocator]` ever warranted it would live in the composition root (`app/src/main.rs`), because an allocator is a property of the binary and a library that chose one would choose it for every dependent: the arena and allocator decisions belong at the composition root, not in a crate others link.
- **Why RTL word order is terminal-delegated by default and visual reordering is opt-in:** no mainstream terminal applies the Unicode Bidirectional Algorithm to displayed cells by default — not xterm, not alacritty, not kitty; wezterm carries `bidi_enabled=false`, VTE waits for `CSI ? 2501 h`, and Windows Terminal needs AtlasEngine and a setting turned on. The hazard is not the algorithm but the **shaper**: every terminal that renders Arabic or Hebrew at all runs HarfBuzz, CoreText, DirectWrite or Pango, and shaping reverses RTL runs. Logical emission is therefore correct on the shaping terminals a reader of those scripts must already be on, while visual emission reverses a second time there and scrambles the run. terminal-wg's "explicit mode", `CSI 8 l`, tells a UBA-aware terminal that the application did the bidi itself, and it does **not** disable the shaper, so it cannot make visual emission safe either. The default is `BidiMode::Terminal` — logical order, byte-for-byte what this binary already emits — and `BidiMode::Visual` is the opt-in escape hatch for the terminals that shape nothing (xterm, alacritty), where the application's own permutation is the only reordering that will happen. Terminal detection was rejected rather than offered as a third answer: bidi and shaping are settings rather than identity, and no probe of the far end survives tmux or ssh. What that leaves is named rather than claimed: in the default mode xterm and alacritty render RTL wrong, neither mode joins Arabic or Persian letters, Rule L4 glyph mirroring is absent, and the default's correctness rests on the terminal matrix above rather than on any assertion in this repository — `ratatui::TestBackend` has no bidi and no shaper, so it can prove that a permutation reached the cells and cannot prove how a terminal drew them.
- **Why the bidi mode is one configuration key rather than a per-terminal setting:** the escape hatch is reachable through the same key surface as everything else in `app/src/config.rs` — `bidi = "terminal"` or `bidi = "visual"`, `TELEVIM_BIDI` from the environment, default `terminal` — read once at composition and handed to `tui` as a plain `BidiMode`, so the `tui` crate names no configuration type and `make boundary` still holds. It is **per machine, not per terminal**: an ssh hop keeps whatever the session was launched with and `tmux` multiplexes one value over every pane it holds, so a reader whose terminals differ cannot have both answers at once. Per-`TERM` is the v2 hook, and it is the reason the value is a setting rather than a probe. Only the exact word `visual` selects it; any other spelling leaves the terminal in charge, because an unrecognised value that guessed `Visual` would scramble a run on precisely the terminals whose shaper is the reason `Terminal` is the default.
- **Why a media kind is a plain `domain` DTO and the unknown case degrades:**
  `domain::MediaKind` is `Photo`, `Video`, `Gif`, `Voice`, `File` — five `Copy`
  variants and nothing else. It is deliberately payload-free: no filename, no
  path, no access hash, no locator. A description is rebuilt on every history
  page, so anything cached on it goes stale, and a stale locator is worse than
  none: the bytes are re-derived from the message when something is actually
  fetched. That makes the field cost nothing per message beyond the byte it
  occupies, and it puts the vocabulary where the rendering lives — `domain`
  already names `ChatKind` and `MessageStatus`, and the interface is the only
  layer that can decide what a kind is *called*.

  The conversion in `proto` breaks the convention `chat_kind` sets. `chat_kind`
  matches the framework's peer taxonomy exhaustively, so a new peer kind stops the
  build; `media_kind` ends in a plain `_ => MediaKind::File`. The asymmetry is
  the requirement: a peer kind always has a consequence the client cannot avoid
  (a group must not be shown as a person), whereas an unmodelled media kind has
  none — the attachment is still a file somebody expects to open, so degrading is
  always safe and failing the build is not. Telegram adds media kinds faster than
  clients catch up, and an exhaustive match would turn every one of them into a
  release blocker for a cosmetic distinction. The framework's own classifiers
  already collapse anything unmodelled to `File` for the same reason; the
  catch-all at the `proto` boundary is what keeps that answer from being lost in
  translation.

  The visible token is the same decision applied to the screen: a media-only
  message shows `[image]`, `[video]`, `[gif]`, `[voice]` or `[file]` through
  `Message::display_body()`, and a caption always wins over it. Both halves are
  decided. The wording is final, and all five tokens are drawn in
  `Theme::text_dim`: a placeholder stands in for something the peer sent rather
  than for something they wrote, so it does not carry the weight of their words.
  A caption is the peer's own text and keeps the body ink.
- **Why a download returns owned bytes for now:** `Client::download_media`
  answers `Vec<u8>` rather than writing to a file or handing back a stream. The
  caller today is a test and a caller that wants to know whether the fetch works,
  and both want one value; a path, a cache directory, or a chunked reader would
  each impose a storage decision on a program that has none yet. The transfer is
  already streamed at the wire (`iter_download`) and already bounded
  (`MEDIA_LIMIT`, 16 MiB, checked against the declared size and again against what
  arrives, so an oversized attachment is reported rather than truncated), so what
  a viewer needs later is a different return type over the same work, not
  different work. Streaming and a cache directory are CUR-9 and CUR-10; `proto`
  narrows the two identifiers to the `i32` the wire uses on the way through, and
  says so with `ProtoError::MessageIdOutOfRange` rather than truncating.
- **Why the reply jump rides the first-unread pipeline, and why the key is `gd`:** a
  reply's quote is a message in the same conversation, and `gg` had already
  answered "a message the reader asked for by name, which the window does not
  hold": `pending_jump` → `net::wanted` → `request_jump` → `fetch_around` →
  `Event::Jumped` → `apply_jump`. A second fetch path would have been the same
  path twice, so `Jump` grew one field — a `JumpKind`, which exists so the status
  line can say where the reader is going — and nothing else in that chain moved.
  The in-window case is `gg`'s: a cursor move and `None`. The key is `gd`, Vim's
  *go to definition*: from a use to the place it refers to, which is what a quote
  is. `g` is already a prefix (`gg`), so `gd` costs no new pending state, and `d`
  after a `g` is not `dd` because the pending key decides. `gf` was the runner-up
  and lost for two reasons: a quote is a definition of context rather than a path,
  and `gd` reads as a pair with `Ctrl-o`, which is how Vim teaches it. `Enter` is
  bound and means "discard a draft" here, `'` and `` ` `` are marks this program
  does not have, and `K` would be expected to *show* a quote rather than move to
  it. The refusals are `flash`es because a refusal is the status line's to say,
  not the hint's, and the first one names the key as well as the refusal — a key
  that refuses in silence teaches nothing. `JUMP_LABEL` is reused for neither: it
  names a destination ("first unread"), and a reader who pressed `gd` and read
  "first unread" would have been told the program did something else. Hence
  `Jumping to the quoted message…`, `Jumping back…` and `Jumping forward…` at the
  same rank and the same ink.
- **Why the return is a bounded per-conversation jumplist, and why `Tab` still
  cycles panes:** the criterion asks for `Ctrl-o` or `Ctrl-i` to return to where
  the reader was, so CUR-34 builds that pair and leaves CUR-35 the general thing —
  no `gg`/`n`/`N` producers and no configurable depth, because only a reply jump
  records. `jumplist::Jumplist` is two `VecDeque` stacks per conversation, capped
  at `DEPTH` (100, against Vim's own `'jumplist'` depth of 50): unbounded, a
  reader who keeps jumping keeps every message they have ever been at, in a
  program whose whole memory budget is a few megabytes. A mark is a **message
  identifier**, not a row, which is the whole reason a return works at all after a
  jump replaced the window — a row means a different message on either side of
  that — and the stacks are keyed by peer id so one conversation's history cannot
  put `Ctrl-o` in another. Vim's move is kept as Vim has it: the entry walked off
  one stack is marked on the other, so a new jump discards what was ahead and the
  first `Ctrl-o` from the end of the list marks where the reader was standing.

  The precedence is the harder half. `Ctrl-i` and `Tab` are the same byte unless
  the terminal speaks the kitty keyboard protocol or `modifyOtherKeys`, and
  crossterm then delivers `KeyCode::Tab` — which is the pane switch here, answered
  globally and ahead of the conversation. So `Tab` is **not** rebound: forward
  navigation is bound to a CONTROL-modified `i` only. The consequence is recorded
  rather than worked around. On a terminal that cannot report the two apart,
  forward is unreachable and `Ctrl-o` carries the criterion alone, which the
  criterion's "or" allows; a second forward key would be a new decision with a new
  design entry, not a workaround invented here.
- **Why a jump keeps the search:** the design engine's `landJump` sets
  `s.search = null` — its hits were rows of the old window — and `DESIGN.md`
  records that. The Rust does the opposite on purpose. `apply_jump` does not touch
  `SearchState` because it is shared with `gg`, and two tests assert the match
  list survives both a jump and a jump page (`a_jump_keeps_the_match_list`,
  `a_search_survives_a_jump_page`); clearing it there would regress `gg` to
  satisfy a sentence about a reply jump. A match here is a message identifier
  rather than a position in the window that was on screen, so it survives the
  window being replaced for the same reason a jumplist mark does. Under this
  repository's own rule — when the engine and the Rust disagree the Rust is right
  — the divergence is recorded in prose rather than changed in code, and the
  design's sentence stands as a record of what the model does.
- **Why typing is a deadline in `tui`, not a message row:** the signal is not a
  message and has no place in the window. `domain` learns it —
  `UpdateEvent::PeerTyping { chat_id, typing }` — and both `ChatList::apply_update`
  and `ConversationWindow::apply_event` acknowledge it as a no-window change,
  because nothing arrived, changed or left and a reader scrolled back up must not
  be moved. What the event carries is a fact about the peer, not about the
  conversation's contents, so the window is the wrong home for it.

  The deadline is the interface's, and it is a deadline rather than a flag. A flag
  set by the event would have to be unset by another event, and the cancel is not
  sent reliably: a peer who sends instead of cancelling, or who stops without
  either, would be shown as typing forever. So `tui` holds `typing_until:
  Option<(i64, Instant)>` and re-arms it on every repeat (`TYPING_FOR`, six
  seconds) — the same deadline shape as `status_until` beside it. It is cleared
  three ways: a cancel, the peer's next message, and the deadline passing. The
  last is the only one that needs a clock, which is exactly why the field is a
  deadline and not a bool. `domain` never reads that clock: the event arrives
  clock-free and the crate stays free of `Instant`, so the whole time-dependent
  half lives in one layer.

  Expiry runs on the loop's existing tick, not on a repaint schedule. `net::drive`
  already calls `expire_status` every pass for the same reason — a frame is drawn
  from a shared reference and cannot expire anything itself — so `expire_typing`
  sits beside it and the note is gone by the tick after its deadline. A repaint
  timer would be a second clock the program schedules for one note, where the tick
  it already runs at 250 ms is finer than the six seconds the note lives for, and
  the draw reads no clock at all: liveness is the deadline being set, and the loop
  is what clears it. A `RowKind` was the other candidate and is worse for the same
  reason it is tempting: it would enter the window, the layout, the scroll
  arithmetic and the selection for transient state that can come and go between
  two frames. The note is drawn on the conversation title instead, so it cannot
  displace a message, and it is dropped whole rather than truncated when the title
  has no room — a half-written `· typ` is worse than no note.
- **Why the open draft is a layout span that counts nothing, and is reserved from the panel:** the draft is row-shaped. It wraps, it sits after the last message, and one wrap has to decide its height for both the layout and the paint, so it is a `RowKind::Draft` span from `App::row_layout` and `rows::draft_rows` is the one wrap, not a second paint pass that measures again. `rows::total_rows` stops before it, so typing a draft moves no scroll, no fetch trigger and no page target; `a_draft_is_in_no_count_while_it_is_on_screen` in `app/tests.rs` and the tests after it pin that. Its rows come out of the panel's budget before the messages get theirs (`Reserved::draft`), because a follow-mode slice fills every row it is given and the draft would otherwise be cut off. The cap keeps one message row on screen, and the reservation changes how many rows the messages may fill, never a count. This cuts against "typing is a deadline, not a row" above, which argues that a row kind for transient state puts it into the window's arithmetic and the selection. The draft stays out of exactly the parts that entry protects: it has no message index, so the cursor cannot stand on it, and the totals stop before it. It is also recomputed from the line on every frame and holds no state of its own, so it cannot go stale the way a status can. The rule it leaves: any count of the layout goes through `rows::total_rows`.
- **Why a reconnect rebuilds the client and preserves the reader's place:** the
  update feed is single-shot — the relay hands its receiver to a subscriber once
  (`UpdateRelay::take`), and a second `subscribe_updates` on the same client
  refuses with `UpdatesAlreadySubscribed` before it touches the network — so
  taking a new feed means building a new `Client`, exactly as signing out does.
  The rebuild reuses the stored session, so there is no re-login. The end of the
  feed is reported only after `UpdateSubscription::finish` has recorded the
  position, and the old `Arc<Client>` is left in `state.client` until the rebuilt
  one's `Ready` replaces it: dropping it first would sync a position behind the
  one the feed reached, and the next launch would replay updates the reader has
  already seen. A reconnect is also not the reader navigating — nothing they did
  moved them — so the `Ready` that lands must keep the place they were in. That is
  `App::refresh_chats`: it replaces the chat list but not the open conversation,
  restoring the highlight by the conversation's own id rather than by an index the
  re-fetch has reordered, and leaving the window, its cursor, the jumplist, the
  selection, the register and the draft untouched. The stale
  `state.history.{cursor,jump,retry_at}` anchors are cleared on that `Ready` so
  `wanted` re-anchors from the preserved window instead of wedging on a cursor
  whose page will never arrive. The automatic reconnect is bounded to one per
  working feed by `auto_reconnect` over `State` — an `Event::Update` clears the
  spent flag, so a rebuild that reaches a feed which immediately ends again becomes
  the reader-visible `offline:` rather than a loop — and it shares the
  `state.bringing_up` single-flight guard with `:retry`, so the two cannot start
  two clients. What it deliberately does **not** do is back off: a recoverable
  `Some(Err)` is logged and carried past exactly as before, and a schedule for
  those errors is CUR-98's decision, not this one's.
- **Why feed errors back off bounded and then rebuild:** CUR-98's recovery shape —
  "bounded backoff and re-subscribe, mirroring the chat-list retry, or an
  explicit reader-visible failure" — turned out to be all three at once. A
  recoverable `Some(Err)` is waited out in the pump's task up to
  `CHAT_LIST_ATTEMPTS` = 3 errors, sleeping `backoff(&error)`'s answer — Telegram's
  own flood wait when it gave one, the fixed `RETRY` = 5s otherwise — because a
  feed refused for being read too often has exactly the same answer as a page that
  was, and the sleep stays in the pump's task so the driver's 250 ms tick never
  waits on it. Each wait is reported as `Event::FeedRetrying`, naming the reason,
  the wait and the count, so the reader watching a live feed gets the same account
  a launch gets from `ChatListRetrying`. Past the bound the feed is ended the same
  way a dead one is: `finish()` records the position first and the old client
  stays in `state.client` until the rebuilt one's `Ready`, for the same replay
  reason as the reconnect above. The waits spend nothing of the one automatic
  reconnect — the retries run before any request, and only the rebuild the driver
  issues consumes it. The reader-visible failure is the existing `offline:` slot,
  recoverable with the existing `:retry`, which is not a budgeted attempt: no new
  command (CUR-49), no indicator (CUR-45), no queue (CUR-47). The persistent
  sentences — retrying, `reconnecting`, `offline:` — clear the flash deadline on
  the way in, because a sentence that ends in an event must not expire back to
  idle while what it reports is still true.
- **Why the sticker attribute is asked about last:** a document's attributes are
  read in the order they can be told apart in — the `voice` flag, then animation,
  video, and a voice-marked audio attribute — and the sticker attribute comes
  after all of them, so a sticker carrying another attribute keeps its more
  specific kind: a sticker that moves was already answered as a GIF, as any
  animation is. Only a document that is nothing but a sticker is one. On the
  typed path the same rule is a second clause: `grammers` reads the sticker off
  the document for us, and a sticker whose `animated` bit is set stays `File` —
  animated stickers are out of scope, and a static-only pipeline that promoted
  one would fetch and paint what it cannot show. The order is stated in a code
  comment at the arm, because an arm order that silently changed would silently
  reclassify.
- **Why sticker decode is synchronous and the cache is bounded twice:** decode
  runs on the event loop — no `spawn_blocking`, no threads — because the runtime
  is a current-thread one and a sticker that misses its frame draws `[sticker]`
  that frame either way; a thread would buy latency nothing can see. The two
  bounds are different dangers: the decode ceiling (1 MiB, a 512-pixel square in
  RGBA) is checked with checked arithmetic on the header's dimensions *before*
  anything is allocated, because 16 MiB of WEBP is not 16 MiB of RGBA, and the
  cache cap (32 KiB, sixteen fit-box pictures and change) evicts oldest-first,
  because identifiers repeat across conversations and the whole cache is dropped
  on switch. Both ceilings carry `const assert!`s against the 50 MB budget, the
  way `MEDIA_LIMIT` does: a number that can grow without failing the build is a
  suggestion.
- **Why the sticker painter is hand-rolled and not `ratatui-image`:** the job
  is 24 half-block cells by 8 rows — one `match` on two pixels — and the crate
  costs a stripped ~1.26 MB over an `image`-decode baseline (measured
  release-harness to release-harness), drags `ravif`/AVIF and friends even at
  `default-features = false`, drops alpha in its half-block encoder, resamples a
  4-by-3 picture through a Triangle filter so the fixture's red-over-black cell
  renders as `Rgb(75, 74, 82)`, and picks its protocol by probing the terminal —
  a capability probe this program is forbidden from running on a thread, and from
  running at all outside the loop. The hand-rolled painter maps exact pixels,
  keeps transparency as terminal-behind-the-picture, needs no dependency and no
  theme role, and is asserted cell by cell in `TestBackend`, which no graphics
  protocol reaches.
- **Why the stickers flag defaults to inline and unknown means inline:** the
  flag (`stickers`, `TELEVIM_STICKERS`, `off` to disable) copies the `bidi`
  pattern — a word in the file, read once, handed to `tui` as a plain
  `tui::state::ui::StickerMode` fixed at construction, because the layout counts
  different rows for a picture than for a token. Only the exact word `off` takes
  the pictures away; everything else, including a spelling the program does not
  know, draws them. The direction is deliberate: a value the program cannot read
  that guessed `off` would take the pictures away from precisely the reader who
  never asked for that, while a value that guesses `inline` on a terminal that
  cannot show pictures still shows `[sticker]` — the fallback the flag names. And
  `off` is zero traffic, not just token pixels: the drain drops the requests
  instead of downloading them, so the cache stays empty and geometry and draw
  take the token path on their own, with nothing left to gate.
- **Why the connection indicator is an always-visible dot beside the ranked
  sentence:** the sentences say what happened and the state says which of the
  four holds, and a rank-9 sentence is hidden under every selection,
  confirmation and search — exactly when the reader most needs the signal. So
  the indicator is not a rank at all: a `●` drawn beside the sentence in every
  state, green while the feed delivers, yellow while a bring-up or a rebuild is
  under way, red once the budget is spent. The connected form renders rather
  than vanishing, because absence is not a state a reader can tell from a
  sentence that outranks it. The detailed retry sentences stay byte-identical
  at rank 9 beside the dot. Two deliberate breaks follow. The dot ends the
  one-sentence invariant: `status_bar.rs` renders two spans now, the dot and
  whatever `status_text()` returns. And it adds three `Theme` roles
  (`conn_connected`, `conn_transient`, `conn_offline`) against the stage plan's
  "no new role" line — the maintainer explicitly asked for coloured dots, and
  the colours are proper roles rather than literals, in the mode labels' own
  hues as foregrounds. The state itself stays rendering-neutral in
  `state::connection`: wording and colour live with the draw, not the type.
