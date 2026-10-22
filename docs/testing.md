# Quality Harness

- `rustfmt.toml` and `clippy.toml` are canonical in themselves; this file does
  not reproduce them.
- `Makefile` — `make ci` is the gate, and is what CI runs.

## The `Makefile`

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
| `make bench` | A/B compare of the criterion micro-benches; writes `target/bench-compare.json` and `.md`. `BENCH_MODE=smoke` (default) or `full`; `BENCH_ARGS="--save-baseline NAME"` or `"--baseline NAME"`. Reports deltas and never fails on slowness |
| `make update` | `cargo update` — **drops the `glass_pumpkin` pin**; see Dependency Notes |

`lint` and `test` both pass `--all-features` for the reason given above.

## CI Pipeline

`.github/workflows/ci.yml`, on every push and pull request, on `ubuntu-latest`:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `make boundary`
4. `cargo test --all --all-features`, with
   `TERMLENS_ARTIFACT_DIR=${{ github.workspace }}/target/termlens-artifacts` for
   the pty tests
5. `actions/upload-artifact` for that directory, with `if: always()` and
   `if-no-files-found: warn`
6. `make design-check`
7. `cargo build --release`
8. `scripts/memory/measure.py` then `scripts/memory/check.py` — the regression
   budget, described above
9. `actions/upload-artifact` for `target/memory-report.json`, with `if: always()`
   so the report survives a failed comparison

The PTY upload is step 5 because `termlens` writes a screen only when an error
carries one (a failed wait, EOF, emulator or write error); a green run produces
no files, which is why `warn` is the honest setting rather than `error`. The
variable is `termlens`'s, not televim's, so a bump can change the file naming or
scope; the test process reads it, not the spawned binary, which is also why it
is pinned to the workspace — an integration test's working directory is the
crate directory.

Steps 8 and 9 are **not** in `make ci`: they need a release build and a pty, so
they run in CI on the artifact rather than on every commit. Step 8 exits zero on
a host with no stored baseline, which is why CI is green on its first run on a
new machine and enforcing from the second.

`libdbus-1-dev` and `pkg-config` are installed first, because `keyring`'s Linux
backend links against DBus at build time.

`.github/workflows/release.yml`, on every push to `main`, is not a gate. It keeps
a release PR open (release-please), writes that PR's `CHANGELOG.md` section
(git-cliff, see `cliff.toml`), and when a release is cut builds
`x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` binaries, attaches them with
a `SHA256SUMS` file, and sets the release notes. It compiles in the application
credentials from the `TELEGRAM_API_ID` and `TELEGRAM_API_HASH` secrets, and fails
if either is missing.

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

## Test Layers

Everything below runs under `make test` unless its bullet says otherwise: the
goldens and property tests are ordinary `#[test]`s, and the deterministic
mutation tests are in `telegram-framework`'s test module. The libFuzzer run in
`fuzz/` is the only local-only command.

- **Unit Tests:** `#[cfg(test)]` modules live beside the code they cover, in
  `domain`, `tui`, `telegram-framework` and `app`. Test the Vim motion logic
  exhaustively (e.g., `gg` on an empty buffer, `G` at the last message, `o` on a
  one-line, mid-row, empty and multi-line draft). Widget
  tests use `ratatui`'s `TestBackend`. Two layers sit in those same modules: the
  committed golden frames below, and the `proptest` properties in `domain` and
  `tui`.
- **Golden tests:** `crates/tui/src/widgets/goldens/` holds two committed 80x24
  text frames, `conversation_frame.txt` and `chat_list_frame.txt`, drawn from
  `App::mock()` and compared one row at a time. They are text-only: the style
  assertions (selection reversal, colours, modifiers) stay as the inline
  `TestBackend` assertions beside each widget. `UPDATE_GOLDENS=1 cargo test -p
  tui` rewrites a frame and fails that run, so an accepted layout change is a
  reviewable diff rather than a hand-copied screen.
- **Property tests:** `proptest` is a dev-dependency of `domain` and `tui`; no
  release build carries it. `domain/src/history.rs` generates operation
  sequences over the window and asserts the cap (`CONVERSATION_WINDOW`), that one
  identifier never appears twice, and that a page from the correct side leaves
  the window ordered and its lookups agreeing. The flat chat list's separate
  `MESSAGE_WINDOW` cap is asserted in the same module. `tui/src/state/conversation.rs`
  generates window changes and asserts the reader's anchor is restored by
  identifier, the cursor stays inside the window, `auto_follow` agrees with the
  cursor, and a selection survives whole or is dropped whole. `PROPTEST_CASES=1000`
  deepens a local run; a failure shrinks and writes a seed under
  `proptest-regressions/`, which is committed.
- **Fuzzing:** `fuzz/` is a detached workspace with its own `[workspace]` and
  nightly toolchain, so it never enters the root pin, lockfile or `make` targets.
  Its one libFuzzer target, `session`, drives the two in-tree decoders of
  untrusted bytes: `SessionData::from_bytes` (a persisted snapshot) and
  `sealed::open` (the encrypted session envelope). The first input byte selects
  the decoder; a panic is the finding. Run it with `rustup toolchain install
  nightly`, `cargo install cargo-fuzz`, then `cd fuzz && cargo fuzz run session --
  -runs=10000`. Neither tool is on the runner, so the fuzzer is a local command,
  not a CI step. The seeds in `fuzz/corpus/session/` are committed; inputs
  libFuzzer grows and crash reproducers under `fuzz/artifacts/` are ignored. The
  MTProto wire decoder itself is external (`grammers-mtproto`), not this
  repository's code: the in-tree surface is the session snapshot and envelope.
  The same property runs on stable in `telegram-framework`'s test module, which
  flips every byte of a valid snapshot and envelope with three masks and asserts
  an answer rather than a panic on each.
- **Chat and contact actions:** the command surface is tested in
  `tui/src/app/tests.rs` by driving the real `:` line and asserting the queued
  `Action`: `:archive` moves the highlighted chat, `:block`/`:unblock`,
  `:delete-chat [both]` and `:report [reason]` raise their `ConfirmKind` with
  the subject and direction captured, `y` queues exactly that action and `n`
  changes nothing, bare `:rename` clears while a name queues
  `SetLocalName`, and an unknown report reason is refused before any question.
  A command typed on a contact's card acts on that person, not on the
  highlighted chat (`a_command_typed_on_a_card_…`), and the card's `rename` row
  prefills the name. The contact card's three action rows and `d` dispatch are
  covered in `tui/src/card.rs` with the same `App::mock` and `TestBackend`
  harness the other card rows use, and the two overlays draw in
  `tui/src/widgets/{contact_list,archived_list}.rs`. Local names, the
  archive moving a chat, and a deletion forgetting the row, draft, alias and
  card — the alias refetch, the archived double-apply, the unarchive highlight
  — are `App` tests in `tui/src/app/tests.rs` and `tui/src/state/chat_list.rs`.
  The app side — the block, report and delete status sentences, a refused
  action changing nothing, a rename the file cannot hold reaching the reader
  as `rename: could not save — …`, and the per-chat media keys going with a
  deleted conversation — is in `app/src/net.rs` and `app/src/media_cache.rs`
  over `State` with no client, and the alias save/load is in
  `app/src/history_store.rs`. The wire is held in
  `telegram-framework`: `delete_history_request` (`revoke` true and false),
  `archived_from` (a user, a group, a message, the seeded peers and the pinned
  order) and the ten report reasons.

- **History cache:** two layers, both on every job and neither needing a
  client. The **store** (`app/src/history_store.rs`) is pure functions over
  `HistoryCache` and one file in a temp directory: the file's round trip as an
  envelope on disk (through a passphrase provider, never the keyring), its
  `0600` mode, an atomic write that leaves no temp behind, a legacy plaintext file
  rewritten sealed with its old inode zeroed
  (`a_legacy_plaintext_file_is_rewritten_sealed`,
  `migration_wipes_the_pre_migration_bytes`), a file sealed under another key or
  behind an unreachable key source left alone and loaded empty, garbage or a missing
  file as an empty cache, the account rules (`a_payload_round_trips_the_signed_in_account_id`,
  `a_legacy_phone_tag_still_accepts_only_the_configured_phone`,
  `a_mismatched_account_id_does_not_seed_the_cache`), the depth, peer, text-byte
  (`the_byte_budget_evicts_the_least_recently_touched`,
  `a_five_megabyte_message_does_not_leave_the_cache_over_budget`,
  `a_file_past_the_byte_budget_is_cut_on_load`) and chat-list
  bounds (on load as well as on write), every merge rule per `PageKind` — overlap,
  replace past a gap, ignore when disjoint, deletion inside a page's stretch, an
  empty page changing nothing — and every feed rule: an edit in place, a deletion
  across all peers, an arrival only into a current run, and the marks an
  interruption, an overtaken page or an eviction take away. The **loop** (`net.rs`
  tests) drives `apply` and `drive` over a `State` with no client: a page landing
  in the cache and on disk, one write in flight at a time, writes coalesced
  within 500 ms (`ten_rapid_updates_are_coalesced_into_at_most_two_writes`,
  `a_burst_leaves_the_file_equal_to_memory`), a failed write that
  does not fail the page, sign-out emptying the cache, feed events and numbered
  sends owing a write, a seed on open and none over a window the wire answered
  empty, a warm start drawing the cached list and conversation before any `Ready`,
  the `Ready` refreshing around it, a page requested under a feed that has since
  ended (`a_page_from_a_dead_feed_cannot_mark_a_run_current_after_a_reconnect`)
  changing nothing while a current-generation page still marks its run current,
  `--chat` from the cache, a `Ready` with no
  session taking the cache down before the form, a `Ready` for another account
  discarding the cache before it draws
  (`a_ready_for_another_account_discards_the_cache_before_it_draws`) and a cache
  tagged with the signed-in account surviving a different configured phone, the draft surviving the `Ready`,
  a failed bring-up leaving the cache readable, and a cold cache launching as it
  always has. `tui`'s half — the seed accepted only for an empty open window, the
  revalidation sentence and its rank, paging held while the newest page is in
  flight — is in `tui/src/app/tests.rs` and the `TestBackend` tests of the
  conversation panel and the status bar.
- **Media cache:** `app/src/media_cache.rs` is tested over a temp directory and no
  client: a file found again after a restart under a pid-free name, the byte cap and
  the entry cap each evicting the oldest modification time first, a small configured
  cap evicting at that size, a deleted message dropped in every chat and a shared
  blob kept for its other key, a media id kept when its message goes, a file that
  could never fit refused without evicting anything, strangers' and temp files ignored, an
  account mismatch clearing the directory, `clear` on a missing directory, the
  owner-only mode before the rename, and a clear landing between a store's write and
  its commit, which refuses the late file without touching a newer store's file. A
  message's embedded thumbnail is stored as a blob of its own and found again after a
  restart, handed back on lookup before it is read, dropped with its parent message and
  its parent's blob, kept while a parent another message shares remains, evicted on
  lookup when corrupt, and taken by eviction with its parent. A
  stored file verifies its recorded CRC32 on lookup; a blob corrupted or truncated on
  disk is evicted — file and pointer gone — and the lookup misses, while a shared blob's
  other key is left alone. An unwritable configured directory (a path
  under a regular file) opens a fallback rooted under an injected cache directory,
  where a store round-trips and a second open finds the file after the restart
  analogue, and the configured path is never created; the fallback keeps the byte cap,
  eviction and `0600` mode; the system temp directory is the last resort when no cache
  directory can be found; and an unwritable fallback too degrades to `None` without
  panicking. A file held for a viewer survives its last pointer going and a `clear`, is
  removed on release (once per hold), and is swept by the next open when never released
  (`a_held_blob_*`, `a_held_then_cleared_blob_*`); `runtime.rs` drives `Viewers` with a fake
  viewer to check that `ViewerDone` releases the hold (`a_viewer_holds_its_file_until_it_reports`).
  The byte cap's file and environment reads, `the_media_cache_max_bytes_is_read_from_the_file`
  and `the_environment_supplies_the_media_cache_max_bytes`, are in `config.rs`'s tests
  under the `ENV` lock.
  The loop's half (`net.rs`) checks that a
  download is stored once and the next open is served from disk, with the download
  never asked for again, that signing out cancels every download in flight, that a
  download larger than the removed 16 MiB ceiling streams and is stored whole, and
  that a cancelled or refused download removes its temp file and stores nothing. The
  streamed store awaits its commit, which runs on `spawn_blocking`; that move is not
  observable from outside, so its evidence is this unchanged behaviour. The
  thumbnail gate answers for a cached preview, so the read it would make never runs on
  a second open.
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

  One of them, `a_media_message_downloads_through_the_client`, is the only
  assertion anywhere that a message's bytes can actually be fetched: it drives
  `ProtoClient::download_media` against a live conversation and checks the
  attachment comes back. It **cannot provision its own media** — it cannot send
  itself a photo, and the account it signs in with has no conversation carrying a
  known attachment — so it reads the newest messages of a conversation it is
  pointed at and skips, with its reason printed, when none of them carries
  anything. The parts of the media path that *can* be provoked without an account
  are not left to it: `classify_raw`/`classify_typed`, the chunk-count and abort
  rules of the streaming seam, the streamed store and its disk-space refusal, and
  the `proto` mapping are all ordinary unit tests that run on every job, and
  the placeholder body is asserted in `tui`'s `TestBackend` tests. What only a
  datacenter can answer is whether Telegram hands over the bytes, which is why
  that one assertion is here and not in CI. The sticker path is covered the same
  way, one layer up: the WEBP decode (fixture pixels, garbage-in-`Err`, the
  pre-allocation clamp), the bounded cache (eviction, replace, clear on switch),
  the eight-row block geometry and its half-block cells, the `[sticker]`
  fallbacks (miss, corrupt bytes, caption, flag off), and the fetch drain — a
  counting test double stands in for `download_media`, so inline drains and
  settles while `off` performs zero traffic. An animated sticker is pinned on
  both sides of the seam: `classify_raw` and `classify_typed` agree on
  `AnimatedSticker` for a TGS (sticker attribute and mime, no animation bit, as
  Telegram sends it) and for a WebM, the widget table draws
  `[animated sticker]` with no block and no pending fetch, `o` queues its open,
  `viewer_spec` gives it the generic viewer rather than the image or video key,
  and the media cache names its file `.tgs`, `.webm` or `.bin` from the magic
  bytes. All of those run on every job.
- **TUI E2E Tests:** `app/tests/tui_e2e.rs` runs the `televim` binary in a pty
  through `termlens` and asserts on the screen. Nine tests: the chat list after
  launch, a seeded cache drawn before the first frame, typing in INSERT, resuming
  a draft, `:q` quitting, Ctrl-u/Ctrl-d moving the conversation's visible slice,
  an arrival moving a pinned view, an arrival leaving a scrolled-back view alone,
  and a fetch in flight shown at the edge it is coming from. **Run policy:** none
  is `#[ignore]`d in a debug build; `make test` and CI's `cargo test --all
  --all-features` run them on every job. They need a pty, not a network or
  credentials. **File map:** the tests are in `tui_e2e.rs`; the helpers
  (`sandbox`, `seed_history`, `spawn_offline`, `spawn_offline_with_scenario`) are
  in `app/tests/common/mod.rs`. The last three use that last helper: it writes
  `scenario.txt` into the sandbox and passes its path in `TELEVIM_SCENARIO`; the
  binary waits for the sibling `<path>.fire`, which `Scenario::fire` writes, and
  then replays each directive through its event channel as an
  `AppEvent::Scenario`, an arrival through `net::apply` as a real
  `UpdateEvent::NewMessage` and a fetch through `App::begin_fetch`. The seam is
  `#[cfg(debug_assertions)]`, so a release build has neither it nor the string
  and those three are ignored with that reason. Rendering of other keystroke
  flows is still covered by `tui`'s `TestBackend` tests.
- **RTL Screenshots:** `make rtl-shots` (`scripts/rtl/kitty_shots.py`) launches the
  debug binary in kitty, types into it through `kitty @`, and crops a screenshot of the
  window to `target/rtl-shots/`. It writes the cell text beside each image. It shows
  what a shaping terminal draws, which `TestBackend` and the pty tests cannot. It is
  **not** in `ci`: it needs kitty, macOS, and Screen Recording permission for the
  terminal that runs it. Offline it reaches the chat list and the input bar only;
  a scene can seed cached messages into the conversation. The picture is a screen capture cropped to the window, so a window in front of kitty is in it: keep the screen clear and trust the `.txt` first. Nothing compares the images
  automatically yet; they are read by a person.
- **Memory Verification:** implemented as `make measure`, not as part of `ci`.
  The driver (`scripts/memory/measure.py`) builds the release binary and
  `crates/app/examples/memory_harness.rs`, runs the harness five times, launches
  the release binary on a pty to read its probes, weighs the binary, and writes
  `target/memory-report.json` beside a Markdown table on stdout. The stored
  baseline is [`memory-baseline.json`](./memory-baseline.json) and the numbers
  are quoted in [`memory.md`](./memory.md). It is not in `ci` because it needs a
  release build (`lto = "fat"`, `codegen-units = 1`) and a pty, and neither
  belongs in a per-commit gate.
  - **The measured binary runs isolated.** It is launched in a temporary sandbox
    *outside* the repository, with its own `HOME`, `TMPDIR`, session path and an
    empty config, inheriting only `PATH`/`TERM`/`LANG`-class variables. Without
    that it reads the root `.env` — `dotenvy::dotenv()` searches the working
    directory **and every parent** — signs in with the developer's credentials,
    fetches a real chat list, and its RSS becomes a property of that account
    rather than of the program. The environment block in the report records what
    was dropped and that no credential reached the run. The sandbox holds no
    history file either, so every figure is a cold-cache launch — see
    [`memory.md`](./memory.md).
  - **A second, populated launch set measures the binary with its data.**
    `crates/app/examples/seed_history.rs` writes a sealed 60-chat cache
    (32 peers, 200 rows each, below every eviction bound) beside the sandbox
    config, and the binary is launched credential-free with only
    `TELEVIM_SESSION_PASSPHRASE` added, which opens that cache and nothing else.
    The no-credentials path draws the cache offline and never touches it, so the
    list is full. The result is `rss_idle_populated_binary_bytes`, with a
    `seeded_history` block of what was loaded; the empty-list
    `rss_idle_binary_bytes` is kept unchanged for comparability.
  - **The compared harness never runs under a profiler.** RSS is read in-process
    (`/proc/self/status` on Linux, `proc_pidinfo` on macOS) because the
    development host has neither `valgrind` nor `heaptrack`, and because under
    valgrind that reading would report *valgrind's* memory and every timing
    would be valgrind's. A massif pass is available as an opt-in diagnostic
    (`scripts/memory/measure.py --massif`), lands in the report as
    `massif_diagnostic` with `compared: false`, and is never thresholded.
- **Regression Budget:** `make measure-check`, which
  `scripts/memory/check.py` implements. It reads the report `make measure`
  wrote, the stored baseline, and the bands in
  `scripts/memory/thresholds.json`, and fails when a figure rises past
  `baseline + max(relative × baseline, absolute)`. Only an increase counts as a
  regression. It is a guard on *change*; the README's declared targets are
  untouched and remain the contract. The failure message names the metric, the
  measured value, the baseline, the delta, and the threshold, and the same
  numbers go to `$GITHUB_STEP_SUMMARY` as a markdown table so a breach is
  visible in the run rather than only in a log line.
  - **One baseline per host.** A stored baseline names its `platform`, and the
    check enforces only when it matches the running host. On a host with no
    baseline it reports and exits zero — a threshold cannot be calibrated on a
    machine it was never measured on — so CI's first run on a new host is a
    recording run. Record it with
    `scripts/memory/check.py --record-baseline docs/memory-baseline.<host>.json`,
    which takes the median across several reports (the re-record of the 0.5.0
    tree used five, saved aside as the driver overwrites `target/memory-report.json`;
    the record is in [`memory.md`](./memory.md)).
  - **Recording a baseline is deliberate.** The driver never writes one: someone
    reads the report and decides what the tree costs.
- **Micro-benchmarks (A/B compare):** criterion 0.8.2, a dev-only dependency, runs
  the benches in `crates/domain/benches` (`history_window`, `search`, `vim`) and
  `crates/tui/benches` (`wrap_rows`). `make bench` drives them through
  `scripts/bench/compare.py`, which runs each target with fixed flags and writes
  `target/bench-compare.json` (the report, schema `televim.bench-compare/1`) and
  `target/bench-compare.md`. It is not in `ci`: it gates nothing, and a full run
  takes minutes, so it stays beside `make measure` rather than inside the gate.
  - **Two legs, one report.** The *in-binary* leg reports each candidate minus a
    reference, from the same run. A pair is declared by bench id: a criterion group
    holding exactly one id whose last segment is `reference` and one or more whose
    last segment is `candidate` (e.g. `word_prefix_match/scan/reference` and
    `word_prefix_match/scan/candidate`). Any other id is reported alone and is never
    paired. The *revision* leg runs against a saved criterion baseline:
    `BENCH_ARGS="--save-baseline NAME"` saves a run, `BENCH_ARGS="--baseline NAME"`
    compares this run with it. **Caveat:** one pair exists, `word_prefix_match`
    (`word_prefix_match_ab/hit` and `/miss`). Its candidate is a byte scan defined in
    the bench file, not in production code, and the bench asserts it answers the same
    as the reference before timing. Other groups measure workloads of one function,
    not two implementations. See [`known-gaps.md`](./known-gaps.md).
  - **Modes.** `BENCH_MODE=smoke` (default) uses short fixed flags and checks that
    the harness runs; its numbers are noisy and are not evidence. `BENCH_MODE=full`
    uses long fixed flags and is the only mode whose numbers a claim may cite.
  - **Report shape.** Each bench gives a criterion median with a 95% confidence
    interval on it, and min/max over per-iteration samples, in nanoseconds. Deltas
    are descriptive (`abs_ns`, `pct`, and whether the CIs overlap). The report has
    no winner field and no pass/fail verdict; a harness fault (failed `cargo bench`,
    missing criterion output, missing baseline) exits 1 and names the leg or bench.
  - **Citing a benchmark claim.** Run the full mode, then cite the JSON:

    ```console
    $ make bench BENCH_MODE=full
    $ make bench BENCH_MODE=full BENCH_ARGS="--save-baseline main"   # on the reference revision
    $ make bench BENCH_MODE=full BENCH_ARGS="--baseline main"        # on the candidate revision
    ```

    Cite `target/bench-compare.json` (with its `.md` beside it) for the run that
    produced the claim. The report records the `mode`, the `criterion_flags`, the
    `toolchain`, the `host` and the `legs` block (which baseline names were saved
    and compared), but **not** the git commit: name the commit in the claim
    yourself. Raw criterion output is under `target/criterion/`. Never cite a smoke
    run's numbers.
- **Probes in the event loop:** `app/src/runtime.rs` records launch → first frame
  and keypress → frame, gated on `TELEVIM_MEASURE` and written to the log file
  beside the config, never to the terminal. An ordinary run reads an empty
  `OnceLock` per frame and returns on one atomic load, with no environment
  lookup and nothing to re-arm; the driver sets the variable. The first-frame
  figure is the empty/sign-in frame (drawn before any round trip), and the
  latency figure spans the loop taking the key to the draw completing — not the
  reader thread's blocking read, and not the terminal's paint.
