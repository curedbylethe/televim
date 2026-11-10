# AGENTS.md

Hard constraints for AI agents and contributors working on `televim`. Every
line here is a rule that, broken, causes a silent failure, a broken build, or a
violated architectural boundary. Everything else lives in `docs/` and
[`README.md`](./README.md) — read those for the reasoning, not for the rules.

`Cargo.lock` and this file are both tracked, and both are load-bearing.

## Toolchain

- **Rust edition:** `2024`
- **MSRV / pinned toolchain:** `1.98.1`
- **Resolver:** `2` (workspace-wide)

Do not bump `edition` or `rust-version` without an explicit request; they are
tied to the toolchain pin in `rust-toolchain.toml`. The formatting edition must
match the compiler edition — both are `2024`.

## Rules

1. **Never hardcode dependency versions in a crate manifest.** Always use
   `workspace = true`.
2. **Respect the dependency rule.** `domain` knows nothing of
   `telegram-framework`, `tui`, `tokio`, `ratatui` or `grammers`; `tui` depends
   on `domain` but **not** on `proto` or `telegram-framework`; `app`
   orchestrates them. Detached `web-demo` may depend on `domain`, `tui` and
   `demo-support`, but not on `app`, `proto` or `telegram-framework`. Only
   `telegram-framework` may reference `grammers` types, and `make boundary` plus
   `make wasm-boundary` prove the two trees. Need protocol data in `domain`? Add
   a plain DTO.
3. **Do not use `unwrap()` in production code** — it is denied by Clippy. Use
   `expect()` with a message, or propagate with `?`.
4. **Keep `panic = "abort"` semantics in mind.** No unwinding: error recovery
   must be explicit, and nothing may catch a panic across an FFI boundary.
5. **`Cargo.lock` is tracked.** Do not add it to `.gitignore`.
6. **Runtime kind is a policy decision, not a feature flag.** Even with
   `tokio`'s `full` features enabled, `app/src/runtime.rs` must use
   `Builder::new_current_thread()` to honor the memory budget.
7. **Build and test with `--all-features`** when touching `telegram-framework` or
   `proto`: their `grammers` dependency sits behind the `live` feature, off by
   default, so a default-feature build silently skips half the workspace.
   `make lint` and `make test` already do this.
8. **Consult `tui::line`'s module docs before `vim-line`'s docs** when writing
   motion logic. The crate's API differs substantially from earlier major
   versions, and **four of its behaviours are defects this workspace routes
   around rather than depends on**: `EditResult::edits` must be applied in
   reverse; the word motions index bytes rather than characters; a visual
   selection is `cursor + 1`, so a range can run past the end of a buffer or
   inside a character; and a delete removes one code point, so the wrapper widens
   it to the grapheme cluster it touched. Every index an edit carries is snapped
   on its way into the string.
9. **The `grammers` crates come from crates.io**, upstream stopped
   tagging after `v0.8.0`. Do not move them back to a `tag =` or `rev =` pin
   without checking `git ls-remote --tags` first — a `rev` pin reintroduces
   hand-tracking of `master`. If an upgrade fails to build, suspect an
   inconsistent published release before suspecting this workspace.
10. **`Cargo.lock` pins `glass_pumpkin` at `2.0.0-rc0` on purpose.** Published
    `grammers` 0.10.0 does not compile with any later `glass_pumpkin`; dropping
    the pin breaks the build inside `grammers-crypto`, with a `BigUint` type
    mismatch, and not inside this workspace. `cargo update` drops it. Restore it
    with:

    ```console
    $ cargo update -p glass_pumpkin --precise 2.0.0-rc0
    ```
11. **`make ci` before every commit.** It catches the `boundary` violation that
    nothing else does, and it is the gate CI runs on a pull request that changes
    code, less `build-release` — the release build and the memory budget run
    only on a push to `main` that changes code, nightly, and pull requests
    labelled for the budget. For changes under `crates/web-demo` or
    `crates/demo-support`, also run the relevant `make wasm-*` targets; `make ci`
    remains the native gate.
12. **These files describe what exists.** If a change makes one of them wrong —
    a new dependency, a moved module, a feature that now works — update it in
    the same commit. A new decision is a new entry in `docs/decisions.md`;
    earlier entries are not rewritten, because a record of reasoning loses its
    value once it is paraphrased.
13. **Logs goes to a file beside the configuration and never the terminal**: this
    program draws on the terminal, and grammers logs at info as a matter of course,
    so a logger that shares the screen with the interface takes it apart on the first connection.

14. **`design/` is a copy, and it must agree with the screens.** It mirrors the
    OpenDesign project `televim-app` path for path, and `make design-check`
    fails when the two differ. An edit made inside `design/` is not finished
    until `make design-push` has sent it upstream. A change to what the screens
    show updates `design/` and [`DESIGN.md`](./DESIGN.md) in the same commit.
    The model is not a reference implementation: where its engine and the Rust
    disagree, the Rust is right.
15. **Release files are written by the release process.** Do not hand-edit
    `CHANGELOG.md`, bump the workspace version, or create a tag: release-please
    and git-cliff do all three on the release PR. A change reaches the changelog
    only through its commit type (see [Commits, branches and pull
    requests](#commits-branches-and-pull-requests)).

### Execution Efficiency Rules

- Prefer the smallest correct implementation that satisfies the request.
- Inspect only files relevant to the task. Expand exploration when evidence requires it.
- Avoid unnecessary architectural changes, speculative improvements, and unrelated refactoring.
- Do not repeatedly read files whose contents are already available in context.
- Before running expensive commands, determine whether they are necessary.
- Prefer targeted tests, linting, and type checks over full-project validation.
- If a command fails repeatedly, diagnose the cause instead of retrying without changes.
- Do not perform multiple rounds of review unless failures or meaningful risks justify them.
- Complete the requested change before investigating optional improvements.
- Report blockers rather than continuing indefinitely without progress.

## Commands

`make ci` is the local gate: `fmt-check lint boundary test design-check build-release`.
Individual targets, and what each one proves, are in
[`docs/testing.md`](./docs/testing.md). The ones that catch the most:

| Command | Why it exists |
| :------ | :------------ |
| `make check` | the fast loop: type-checks the whole workspace without a full build |
| `make boundary` | checks root workspace for `grammers` outside `telegram-framework` |
| `make wasm-boundary` | asserts the detached `web-demo` tree has no Telegram crates |
| `make lint` | `clippy --all-targets --all-features -- -D warnings` |
| `make test` | `test --all --all-features` |
| `make design-check` | fails if `design/` and the OpenDesign project differ; **skips loudly** when there is no project on the machine, and a skip is not a pass |
| `make design-push` | sends this repository's `design/` files to the OpenDesign project (rule 14) |

**Not part of `make ci`**, and so not a per-commit gate: the memory budget
(`make measure`, then `make measure-check`, against `docs/memory-baseline.json`
and `scripts/memory/thresholds.json`), `make bench` (descriptive; it never fails
on slowness), `make rtl-shots` (images a person reads), the self-hosted
`.github/workflows/desktop.yml` (keyring round trip and live datacenter,
`workflow_dispatch` only), and `crates/app/tests/proto_integration.rs`, which
runs in no workflow. A check that exits zero because it has no baseline for the
host is a recording, not a measurement. [`docs/testing.md`](./docs/testing.md)
has the detail, and [`docs/memory.md`](./docs/memory.md) the budget.

## Commits, branches and pull requests

- **Commits are Conventional Commits:** `type(scope): lowercase imperative
  subject`, with a body that says what changed and why. Scope is a crate (`app`,
  `tui`, `domain`, `proto`, `telegram-framework`, `demo-support`, `web-demo`) or
  an area (`design`, `ci`, `docs`, `memory`, `release`). Every commit leaves the
  tree green.
- **The type decides the changelog.** `feat`, `fix` and `perf` are listed in
  release notes; `docs`, `build`, `test`, `chore`, `refactor` and `style` are
  skipped on purpose (`cliff.toml`). Commit a user-visible change as `feat`,
  `fix` or `perf`, or it never ships in the notes.
- **Branches are `<prefix>/<short-slug>`**, with a prefix from `feature/`,
  `fix/`, `chore/`, `refactor/`, `docs/`, `perf/`, `test/`, `build/`, `ci/`.
- **Branch names, commit messages, and PR titles carry no issue number.** A
  pull request body names the issue it resolves as `Closes #N`.
- **A pull request that needs the memory budget** carries the `memory` label.

## Where the rest of it lives

| File | What is in it |
| :--- | :------------ |
| [`README.md`](./README.md) | Getting started: install, first run, and essential controls |
| [`docs/product.md`](./docs/product.md) | Product goals, acceptance criteria, and the feature set as it ships |
| [`docs/installing.md`](./docs/installing.md) | Install, upgrade and remove televim on macOS, Linux and Windows |
| [`docs/architecture.md`](./docs/architecture.md) | Workspace layout, the dependency rule, per-crate design |
| [`docs/dependencies.md`](./docs/dependencies.md) | Library rationale, and why each dependency is chosen or pinned; the `grammers`/`glass_pumpkin` saga |
| [`docs/decisions.md`](./docs/decisions.md) | Key Decisions — the ADRs, each with the reasoning behind it |
| [`docs/testing.md`](./docs/testing.md) | Quality harness, the `Makefile`, CI pipelines, the test layers |
| [`docs/memory.md`](./docs/memory.md) | The memory budget: what is built, what is declared, what is unmeasured |
| [`docs/web-demo.md`](./docs/web-demo.md) | Browser demo architecture, build, hosting and limitations |
| [`docs/web-demo-parity.md`](./docs/web-demo-parity.md) | Browser feature parity and evidence matrix |
| [`docs/web-demo-embed.md`](./docs/web-demo-embed.md) | Portfolio iframe integration contract |
| [`docs/known-gaps.md`](./docs/known-gaps.md) | Known Gaps — real ones, named so they are not mistaken for oversights — and the v2 hooks |
| [`DESIGN.md`](./DESIGN.md) | The design system the vendored model in `design/` implements |
