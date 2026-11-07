# WASM feasibility

## Pinned stack

- Rust 1.98.1, edition 2024, `wasm32-unknown-unknown`
- Ratzilla 0.3.1 with Ratatui 0.30.2 through Ratzilla
- `wasm-bindgen` 0.2.130, Trunk 0.22.1, `wasm-opt` version 131
- Detached crate depends on `domain` and `tui`; it does not depend on native `app` or Telegram crates.

## Build evidence

These commands passed with the pinned toolchain:

```console
cargo check --manifest-path crates/web-demo/Cargo.toml --target wasm32-unknown-unknown
cargo build --manifest-path crates/web-demo/Cargo.toml --target wasm32-unknown-unknown --release
(cd crates/web-demo && trunk build --release --locked)
```

Trunk used `wasm-bindgen` 0.2.130 and `wasm-opt` version 131 from `Trunk.toml`. The release profile omits `strip = true`: Rust stripping removed WASM target features, causing wasm-opt 131 to reject bulk-memory operations. Trunk performs optimization and stripping after wasm-bindgen.

## Backend matrix

`src/backend.rs` compiles both Ratzilla backend paths into the same WASM binary. `?renderer=dom` selects `DomBackend`; WebGL2 is the default and can be selected explicitly with `?renderer=webgl2`. WebGL2 uses a dynamic JetBrains Mono font atlas at 16 px. Canvas is not used.

| Check | WebGL2 | DOM | Canvas |
| :--- | :--- | :--- | :--- |
| WASM compile and release build | Pass | Pass | Not selected |
| Trunk release build | Pass | Pass | Not selected |
| Real App frame in browser | Deferred to Stage 11 Playwright | Deferred to Stage 11 Playwright | Not selected |
| Key reaches `App::handle_key` in browser | Deferred to Stage 11 Playwright | Deferred to Stage 11 Playwright | Not selected |
| Resize, emoji, RTL, accessibility | Browser checks deferred to Stage 11 Playwright | Browser checks deferred to Stage 11 Playwright | Not selected |

Compile and build evidence proves both backend implementations are included, not that either rendered correctly in a browser. No browser render, input, resize, Unicode, accessibility, or frame-cost result is claimed. The test plan now assigns those checks to Stage 11 Playwright.

The backend forwards Ratzilla key events to `App::handle_key`; keyboard behavior still needs browser verification. The WebGL2 canvas and DOM grid require a click before typing. No strict CSP claim is made; deployment must allow WebAssembly execution. No API credentials or native `app` dependency are used.
