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
- **TUI E2E Tests:** `app/tests/tui_e2e.rs` holds eight `#[ignore]`d
  placeholders describing what a PTY harness should assert — the screen after
  launch, typing, `:q`, scrolling, and an arrival moving a pinned view. **None of
  them run**: `termlens` is not a dev-dependency, and the test bodies are `TODO`
  comments. Keystroke and rendering coverage today comes from the unit tests and
  the `TestBackend` assertions in `tui`'s conversation panel. Wiring this up means
  adding `termlens` to `app` and filling the bodies in.
- **Memory Verification:** not implemented. The 50 MB ceiling is a target with no
  measurement behind it; adding a `heaptrack` or `valgrind --tool=massif` step
  that fails past it is the way to make it real.
