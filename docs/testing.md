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
| `make update` | `cargo update` — **drops the `glass_pumpkin` pin**; see Dependency Notes |

`lint` and `test` both pass `--all-features` for the reason given above.

## CI Pipeline

`.github/workflows/ci.yml`, on every push and pull request, on `ubuntu-latest`:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `make boundary`
4. `cargo test --all --all-features`
5. `make design-check`
6. `cargo build --release`
7. `scripts/memory/measure.py` then `scripts/memory/check.py` — the regression
   budget, described above
8. `actions/upload-artifact` for `target/memory-report.json`, with `if: always()`
   so the report survives a failed comparison

Steps 7 and 8 are **not** in `make ci`: they need a release build and a pty, so
they run in CI on the artifact rather than on every commit. Step 7 exits zero on
a host with no stored baseline, which is why CI is green on its first run on a
new machine and enforcing from the second.

`libdbus-1-dev` and `pkg-config` are installed first, because `keyring`'s Linux
backend links against DBus at build time.

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

- **Unit Tests:** `#[cfg(test)]` modules live beside the code they cover, in
  `domain`, `tui`, and `telegram-framework`. Test the Vim motion logic
  exhaustively (e.g., `gg` on an empty buffer, `G` at the last message). Widget
  tests use `ratatui`'s `TestBackend`.
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
  are not left to it: `classify_raw`/`classify_typed`, the `MEDIA_LIMIT` check,
  and the `proto` mapping are all ordinary unit tests that run on every job, and
  the placeholder body is asserted in `tui`'s `TestBackend` tests. What only a
  datacenter can answer is whether Telegram hands over the bytes, which is why
  that one assertion is here and not in CI.
- **TUI E2E Tests:** `app/tests/tui_e2e.rs` holds seven `#[ignore]`d
  placeholders describing what a PTY harness should assert — the screen after
  launch, typing, `:q`, scrolling, and an arrival moving a pinned view. **None of
  them run**: `termlens` is not a dev-dependency, and the test bodies are `TODO`
  comments. Keystroke and rendering coverage today comes from the unit tests and
  the `TestBackend` assertions in `tui`'s conversation panel. Wiring this up means
  adding `termlens` to `app` and filling the bodies in.
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
    was dropped and that no credential reached the run.
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
    which takes the median across several reports.
  - **Recording a baseline is deliberate.** The driver never writes one: someone
    reads the report and decides what the tree costs.
- **Probes in the event loop:** `app/src/runtime.rs` records launch → first frame
  and keypress → frame, gated on `TELEVIM_MEASURE` and written to the log file
  beside the config, never to the terminal. An ordinary run reads an empty
  `OnceLock` per frame and returns on one atomic load, with no environment
  lookup and nothing to re-arm; the driver sets the variable. The first-frame
  figure is the empty/sign-in frame (drawn before any round trip), and the
  latency figure spans the loop taking the key to the draw completing — not the
  reader thread's blocking read, and not the terminal's paint.
