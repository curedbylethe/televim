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
- **Why the measurement harness is an `[[example]]` and not a binary or a test:** `cargo build --release` does not build examples, so a harness that is one cannot reach the shipped artifact, and `app` is bin-only so its own event loop is unreachable from `app/tests` anyway. An example is also the only vehicle that needed no change to `[profile.release]`, which is load-bearing for the binary-size budget and out of bounds for measurement work. The fixture is built through `tui`'s public API rather than its `#[cfg(test)]` sample data because that data is invisible to every dependent crate: a harness that compiled only under `cfg(test)` could not be measured as a release build.
- **Why RSS is read in-process rather than by a profiler:** neither `valgrind`, `heaptrack`, nor `hyperfine` is installed on the development host, so a harness that required one could only be run in CI. Reading the process's own resident size — `proc_pidinfo` on macOS, `/proc/self/status` on Linux — is the kernel's number for this process rather than a profiler's estimate, needs no dependency, and makes the harness a build target rather than a machine with tools on it. `valgrind --tool=massif` is still used automatically where it exists, and the choice of source is recorded in the report so two hosts' figures are not silently compared. The cost is that the macOS figure carries the VM's page-in schedule with it — one run in fifteen lands near 1.5 MB rather than 3.4 MB, with all five samples inside a run agreeing exactly — so a threshold taken from it alone would report the OS rather than the program. The noise floor is recorded so that is visible before anyone draws one.
- **Why the harness records both a within-run and a cross-run spread:** the two answer different questions and only one of them is about this program. Within a run the samples measure the process settling; across runs they measure the machine the process started on. Quoting the first as the noise floor would understate it by a factor of two, and a threshold drawn from it would fire on the host.
- **Why the idle RSS is reported as two figures and not one:** neither candidate process is the program under load. The harness holds 60 chats and a fully drawn screen but none of the network half; the shipped binary is the whole program but with an empty chat list, because there is no offline path to a populated one. Averaging them, or picking one and calling it the measurement, would produce a number that describes no process that exists. Both are recorded, and the range between them is the honest state: the figure for 50 chats inside the real binary is bounded from both sides and has not been taken.
- **Why the timing probes are gated on an environment variable rather than a flag:** adding a flag would add a user-visible surface to satisfy a measurement, and the probes' cost in an ordinary run — one `var_os` per event and an `Option` that stays `None` — is smaller than the branch the loop already takes. They write to the log file beside the configuration, never the screen, because a `tracing` line written mid-frame lands on the display this program is in the middle of drawing.
