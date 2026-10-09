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
   orchestrates them. Only `telegram-framework` may reference `grammers` types,
   and `make boundary` is what proves it. Need protocol data in `domain`? Add a
   plain DTO.
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
11. **`make ci` before every commit.** It is the same gate CI runs, and it
    catches the `boundary` violation that nothing else does.
12. **These files describe what exists.** If a change makes one of them wrong —
    a new dependency, a moved module, a feature that now works — update it in
    the same commit.
13. **Logs goes to a file beside the configuration and never the terminal**: this
    program draws on the terminal, and grammers logs at info as a matter of course,
    so a logger that shares the screen with the interface takes it apart on the first connection.

## Commands

`make ci` is the gate: `fmt-check lint boundary test design-check build-release changelog-check`.
Individual targets, and what each one proves, are in
[`docs/testing.md`](./docs/testing.md). The ones that catch the most:

| Command | Why it exists |
| :------ | :------------ |
| `make boundary` | asserts no `grammers` crate outside `telegram-framework` |
| `make lint` | `clippy --all-targets --all-features -- -D warnings` |
| `make test` | `test --all --all-features` |
| `make design-check` | fails if `design/` and the OpenDesign project differ; **skips loudly** when there is no project on the machine |

## Where the rest of it lives

| File | What is in it |
| :--- | :------------ |
| [`README.md`](./README.md) | Project overview, goals, the feature set as it ships, acceptance criteria |
| [`docs/architecture.md`](./docs/architecture.md) | Workspace layout, the dependency rule, per-crate design |
| [`docs/dependencies.md`](./docs/dependencies.md) | Library rationale, and why each dependency is chosen or pinned; the `grammers`/`glass_pumpkin` saga |
| [`docs/decisions.md`](./docs/decisions.md) | Key Decisions — the ADRs, each with the reasoning behind it |
| [`docs/testing.md`](./docs/testing.md) | Quality harness, the `Makefile`, CI pipelines, the test layers |
| [`docs/memory.md`](./docs/memory.md) | The memory budget: what is built, what is declared, what is unmeasured |
| [`docs/known-gaps.md`](./docs/known-gaps.md) | Known Gaps — real ones, named so they are not mistaken for oversights — and the v2 hooks |
| [`DESIGN.md`](./DESIGN.md) | The design system the vendored model in `design/` implements |
