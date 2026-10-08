# Dependencies

**No version numbers live in this file.** Every version, feature and default is
canonical in `[workspace.dependencies]` in the root `Cargo.toml`, and crate
manifests inherit from it with `workspace = true` — never a hardcoded version in
a crate manifest. This file records *why* a dependency is there, what it is
pinned against, and what breaks when it moves.

## Changing one

Edit only `[workspace.dependencies]` in the root `Cargo.toml`, then
`cargo update -p <crate>` if the lockfile needs to follow. If the crate is
`glass_pumpkin`, use `--precise 2.0.0-rc0` — see below for why.

## Why each dependency is here

### The `grammers` saga, and other pins

- The `grammers` crates are taken from crates.io, **not** from git. This reverses an earlier decision, and the reason is upstream's: `Lonami/grammers` stopped tagging releases after `v0.8.0`. 0.8.1, 0.9.0 and 0.10.0 are all published to the registry with no corresponding tag, so no `tag =` can name the version that is actually current, and a `rev` pin would mean tracking `master` by hand to receive patches. The published artefact is checksummed and is the one upstream supports.

  ```console
  $ git ls-remote --tags --refs https://codeberg.org/Lonami/grammers.git
  2d51ad1e…  refs/tags/mvp
  5f153640…  refs/tags/v0.2.0
  4c2e7fc3…  refs/tags/v0.3.0
  fcb9fe38…  refs/tags/v0.4.0
  a69198d4…  refs/tags/v0.8.0        ← the newest tag that exists
  ```

  There is no release automation upstream — `.forgejo/` holds only a PR template and the `Makefile` is `fmt` and `dev` — so the version is hand-bumped per manifest and published by hand. **If a `grammers` upgrade ever fails to build, check whether a published release is internally inconsistent before assuming the fault is ours.** That is not hypothetical: 0.10.0 requires `glass_pumpkin = "2.0.0-rc0"` while depending on `num-bigint 0.4`, and `glass_pumpkin 2.0.0-rc1` moved to `num-bigint 0.5`, so the published 0.10.0 does not compile. The fix is on `master` and is unpublished; until 0.10.1 ships, `Cargo.lock` holds `glass_pumpkin` at `2.0.0-rc0` **on purpose**, which is why the lockfile is tracked. A `cargo update` that drops the pin breaks the build inside `grammers-crypto`, with a `BigUint` type mismatch, and not inside this workspace. To restore it:

  ```console
  $ cargo update -p glass_pumpkin --precise 2.0.0-rc0
  ```

  `grammers-tl-types` generates from **TL layer 227**. That is the datum the next
  schema bump is diagnosed against.
- `grammers-mtsender` is what exposes `SenderPool`, which is the only way to construct a `grammers_client::Client`. `grammers-client` re-exports its `InvocationError` but not the pool itself, so the direct dependency is required.
- `grammers-client` is built with `default-features = false`, which drops only its `fs` feature. `grammers-tl-types` is pulled in by `grammers-client` with its default features, so `tl-api` is enabled through feature unification even though this workspace asks only for `tl-mtproto`.
- `tokio` enables the `full` feature set for flexibility across crates, but the application runtime is chosen explicitly at startup (see [`memory.md`](./memory.md)).
- `vim-line` is pinned to `7.7` and used by `tui` for the input line, behind the
  `tui::line` wrapper. Its API differs substantially from older major versions, so consult the current docs when writing motion logic — and consult the wrapper's module docs first, because four of its behaviours are defects this workspace routes around rather than depends on: `EditResult::edits` must be applied in reverse, the word motions index bytes rather than characters, a visual selection is `cursor + 1` — so a range can run past the end of a buffer, or inside a character, and every index an edit carries is snapped on its way into the string — and a delete removes one code point, so the wrapper widens it to the cluster it touched.
- `keyring` needs `libdbus-1-dev` and `pkg-config` on Linux to build the Secret Service backend; CI installs them before the first `cargo` step.

### The rest of them

| Dependency | Why it is here |
| :--------- | :-------------- |
| `tokio` | The async runtime. One event loop, so the runtime is current-thread: `app/src/runtime.rs` chooses `new_current_thread()` explicitly at startup even though the `full` feature set is enabled — see [`memory.md`](./memory.md). |
| `anyhow` | Error handling at the `app` boundary, where there is nothing typed left to match on. |
| `thiserror` | Typed errors in `telegram-framework`, `proto` and `domain`. |
| `serde` / `serde_json` | Configuration and the session cache. |
| `grammers-client` / `grammers-tl-types` / `grammers-mtproto` / `grammers-mtsender` | The Telegram protocol, wrapped in a first-party `telegram-framework` crate. Optional behind that crate's `live` feature, off by default, which is what makes `make boundary` mechanically checkable. See the notes above for why they come from crates.io and why `glass_pumpkin` is pinned. |
| `ratatui` | The TUI framework. Immediate-mode, so a frame draws what changed. |
| `crossterm` | The terminal backend behind `ratatui`, and the path through which `app` holds the terminal — it is also what delivers `Shift+Enter` as a newline rather than a send, via the kitty keyboard protocol. |
| `vim-line` | The input line's editor: a trait-based line editor with Normal/Insert modes and motions, behind the `tui::line` wrapper — the wrapper owns the text, decides `Enter`/`Esc` itself, and routes around four upstream defects. `7.7` is a caret range, so `tui::line`'s spike tests pin the behaviour the wrapper depends on against whatever resolves. |
| `unicode-width` | One use: how wide a string is on a terminal, in `tui::wrap::columns`. `ratatui` already resolves it, so the row layout and the caret column share one table. |
| `unicode-segmentation` | One use: grapheme cluster edges, in `tui::grapheme`. A delete removes one and a row is never cut inside one. Already in the lockfile via ratatui. |
| `unicode-bidi` | One use: which direction a message reads, in `tui::bidi`. A terminal that neither shapes nor applies UAX #9 (xterm, alacritty) reorders nothing, so in the opt-in `visual` mode a right-to-left row has to be permuted into visual order here; UAX #9 is the only thing that knows by what rule. `base_direction` uses `get_base_direction_full` so a leading all-neutral paragraph cannot decide the direction, and `visual_row` returns an ordered list of *logical* byte ranges rather than a reordered string, snapped to `tui::grapheme` cluster edges. Rule L4 (mirroring) and shaping are deliberately not taken: no terminal applies either, and both would mean two more crates. Unlike `unicode-segmentation` beside it, `ratatui` does not resolve this one, so it is a direct dependency with nothing else to share it with. |
| `emojis` | The GitHub gemoji set behind the input line's `:shortcode` completion: `&'static` `phf` tables, reached only while composing, costing ~0.5 MB of binary and ~2 MB of RSS and ~5 µs to scan. The whole catalog, not a curated subset — 1914 emoji, of which 1870 carry a shortcode — rather than a table this workspace otherwise maintains by hand for bytes it does not need. |
| `image-webp` | The static-sticker decoder, in `tui::sticker`. Pure Rust and dedicated: weighed against `image` with only its `webp` feature (+66 KB of `moxcms`/`pxfm` scaffolding) and `webp` (+50 KB plus vendored C compiled via `cc`, an FFI surface and a build-time C toolchain). Decode is clamped before allocating and a failure draws `[sticker]`, so hostile bytes are a fallback, not a panic. |
| `ratatui-image` | Deliberately **not** a dependency. The recommended half-block renderer, weighed for the sticker block and rejected: ~1.26 MB of stripped binary over an `image`-decode baseline (release-harness to release-harness), `ravif`/AVIF and friends even at `default-features = false`, alpha dropped in its half-block encoder, a Triangle resample that renders the 4-by-3 fixture's red-over-black cell as `Rgb(75, 74, 82)`, and protocol detection by probing the terminal — a probe this program will not run. The painter is ~100 lines of `Buffer` cells in the conversation widget instead, asserted cell by cell. |
| `tracing` / `tracing-subscriber` | Structured logging, written to a file. **Never the terminal** — this program draws on the terminal, and `grammers` logs at `info` as a matter of course. |
| `clap` | CLI parsing in `app/src/main.rs`: `--config` plus `--chat <id>`, the latter carried `Cli`-only into launch state (`runtime::run`). |
| `config` | Layered TOML + env configuration. |
| `dotenvy` | One use: loading `.env` so the credentials it holds reach `Config`. A missing file is not an error. |
| `base64` | One use: base64 inside the OSC 52 clipboard sequence. `app/src/runtime.rs` writes that sequence because it is the only place holding the terminal; `tui` records the text and never writes it. |
| `keyring` | Cross-platform OS keyring for the session string, and for the random file key that seals the session *file* (`service "televim"`, `account "file-key"`); never store session strings in plaintext. It ships no default backend, so every supported platform store is named explicitly. They are target-gated, so enabling all of them is safe everywhere; on Linux the Secret Service backend links against `libdbus` and needs `libdbus-1-dev` and `pkg-config`. |
| `aes-gcm-siv` | The AEAD that seals the session file, in `telegram-framework::sealed`: AES-256-GCM-SIV (RFC 8452), built with `default-features = false` and only `aes` + `alloc`. Chosen over XChaCha20-Poly1305 for two reasons. It reuses the `aes`/`cipher`/`ctr` stack already locked through `grammers-crypto` and `keyring`'s `crypto-rust`, so it adds a thin mode on top of a cipher the tree already builds rather than a second cipher. And it is nonce-misuse-resistant by construction: the nonce is random per save, and a repeat would leak only that two plaintexts were equal, not the authentication key. XChaCha20's 192-bit nonce makes a repeat negligible, but that is a property of the random source, where SIV's resistance is a property of the cipher. Pure Rust, so no C toolchain on any platform. New lockfile entries: `aead`, `polyval`, `universal-hash`. |
| `argon2` | One use: stretching the session passphrase into the 256-bit file key, as Argon2id with `m` = 19 MiB, `t` = 2, `p` = 1 (the OWASP minimum for Argon2id). Chosen over PBKDF2, which is not memory-hard — an attacker with GPUs or ASICs pays only for compute — and which is in the lockfile only transitively through `grammers`, so using it would mean depending directly on a crate this workspace does not otherwise ask for. Chosen over HKDF, which is the right tool for a key that is already uniformly random and the wrong one for a passphrase: it has no work factor, so guessing costs one hash. **The three constants are frozen.** They define what a passphrase derives to, so changing one orphans every file already written; a different cost needs a new envelope magic, not an edit. They are written out in `sealed.rs` rather than taken from the crate's defaults for that reason. The 19 MiB is transient — see [`memory.md`](./memory.md). New lockfile entries: `blake2`, `password-hash`, `phc`, `base64ct`. |
| `getrandom` | One use: the OS entropy behind every salt, nonce and the keyring-held file key. Already in the lockfile transitively. |
| `zeroize` | One use: wiping key material and the passphrase when they are dropped (`Zeroizing`), so a `FileKey` or a passphrase does not outlive its use in freed memory. Already in the lockfile transitively. It does **not** reach the expanded AES key schedule the cipher holds while it is alive — see [`decisions.md`](./decisions.md). |

| `tempfile` | Dev-only. |
| `static_assertions` | Dev-only. Compile-time assertions, used to lock the "a login token cannot be cloned or reused" guarantee with the compiler rather than with a convention. |

### Declared but not depended on

`criterion`, `bumpalo`, `termlens` and `ratatui-testlib` are **not** dependencies.
Widget tests use `ratatui`'s own `TestBackend`; there are no benchmarks; and
`app/tests/tui_e2e.rs` is waiting on `termlens` before any of it can run. See
[`testing.md`](./testing.md).

`tikv-jemallocator` is in that state too, and was there until it was removed: it
had been declared in `[workspace.dependencies]` with no dependents and no entry in
`Cargo.lock`, which is a name in a manifest standing in for a decision nobody had
made. The global allocator is `std::alloc::System` — measured, not assumed; see
[`decisions.md`](./decisions.md) and [`memory.md`](./memory.md).
