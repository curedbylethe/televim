# Web demo

The offline browser demo compiles Televim's real `domain` and `tui::App` into a
static WebAssembly page. Fictional data comes from `demo-support`; the demo does
not connect to Telegram, load credentials or save state.

## Architecture

`crates/web-demo` is a detached Cargo workspace with its own lockfile. It uses
`tui::key` as the portable key API, `web-time` for browser-safe clocks and
Ratzilla's Ratatui WebAssembly support. `crates/demo-support` stays in the root
workspace and provides the deterministic simulator and fictional fixtures.

The outer page keeps a static poster and provenance visible. Load creates a
same-origin iframe; the iframe initializes `web-demo`, which runs Ratzilla's
`draw_web` and `on_key_event` against the real app. `Driver` joins the `App` and
`DemoSimulator`; simulator effects update the app through public methods.
`BrowserBridge` handles browser-only clipboard, media and file-picker behavior.
Reset creates a fresh simulator and app. Nothing uses `localStorage`.

The browser crate depends on `domain`, `tui` and `demo-support`. It has no
dependency on `app`, `proto` or `telegram-framework`. The native `app` and
`release` pipeline remain separate.

## Build and test

Install Rust 1.98.1, Trunk 0.22.1, wasm-bindgen 0.2.130 and the
`wasm32-unknown-unknown` target. Browser tests use wasm-pack 0.15.1, Node 22,
pnpm 10.22.0 and Chromium. CI pins these tools in
`.github/workflows/ci.yml` and `.github/workflows/pages.yml`.

```console
rustup target add wasm32-unknown-unknown
make wasm-check
make wasm-build
make wasm-test
make wasm-e2e
make wasm-pages
make wasm-size
make ci
```

`make wasm-pages` builds for `/televim/` by default. Set `PUBLIC_URL` to build
another deployment subpath. `make ci` remains the native gate; the separate
`wasm` CI job runs browser checks on code changes. The Pages workflow deploys
from `main` or manual dispatch. Its build performs secret and debug-artifact
checks before upload.

## Renderers and parity

WebGL2 is the default Ratzilla renderer and uses a dynamic glyph atlas. If
WebGL2 does not start, the shell reports fallback and uses the accessible DOM
renderer. Add `?renderer=dom` to force DOM rendering. Canvas is not used.

The [parity matrix](web-demo-parity.md) maps controls, commands, overlays and
simulator actions to evidence. Core logic runs through the real Rust app;
browser-specific behavior uses same-origin messages and browser APIs. Browser
rendering does not prove compatibility with native terminals.

## Limits

- No Telegram connection, sign-in, session import/export, update download or server-side state.
- The cast and messages are fictional and fixed. Reset clears local simulator state.
- No persistent browser storage or native media cache exists.
- Browser rendering draws one glyph per cell. It does not shape Arabic or join letters across cells.
- Terminal text links do not open. Browser file selection cannot provide native filesystem paths.
- Clipboard and file saving depend on browser permissions and user gestures.

Build details and the backend feasibility record live in
[`crates/web-demo/FEASIBILITY.md`](../crates/web-demo/FEASIBILITY.md).
The [portfolio iframe contract](web-demo-embed.md) describes the external
integration. The real portfolio repository and live integration remain pending.

## Unicode and bidi

The browser renderer draws one glyph per cell. Televim's bidi modes reorder text for display; they do not shape Arabic or join letters across cells. A browser screenshot is evidence for the demo, not proof of terminal compatibility.

The demo sets `BidiMode::Visual`, which reorders mixed-direction rows before drawing. `BidiMode::Terminal` leaves reordering to a bidi-capable terminal and is not the browser default. `BidiMode::Words` reverses right-to-left word order for terminals that shape each run.

WebGL2 uses Ratzilla's dynamic glyph atlas; the DOM renderer is the accessible fallback. DOM text can shape glyphs within a cell, but adjacent cells still do not join. A missing WebGL2 glyph must use the DOM renderer rather than a replacement character.

Composer edits and Backspace operate on grapheme clusters. Selection and yank preserve logical text order. ZWJ emoji can occupy four cells under `unicode-width` even when displayed as two; the TUI keeps those cells, and `Buffer::diff` handles VS16. Do not drop joiners, variation selectors or combining marks to make the browser frame fit.

The mobile hidden input sends committed IME text as Televim keys. Intermediate composition input is ignored; Enter and Backspace wait until composition ends. Browsers without composition events use the input event fallback. Playwright includes a synthetic composition test; browser execution awaits CI because Trunk cannot fetch `minicov` locally.

The evidence suite covers Persian, Arabic, mixed-direction text, combining marks, emoji, search prefixes, composer editing and logical-order yank through the real `App`. Browser rendering checks remain unrun locally: Trunk metadata resolution needs `minicov` from crates.io, and this environment cannot resolve `static.crates.io`. Real OS terminal shaping, RTL terminal emulators and hardware keyboards across mobile systems are not covered.

## Hosting and security policy

GitHub Pages serves the static artifact at `https://curedbylethe.github.io/televim/`.
`PUBLIC_URL` sets Trunk's asset base; `make wasm-pages` defaults it to `/televim/`.
The Playwright server mounts the build under that same path before checking the
poster, iframe, WASM request and controls.

Both HTML pages set a meta Content Security Policy. It allows same-origin assets
and frames, blob media, inline styles used by the terminal page, and
`wasm-unsafe-eval` required by WebAssembly. It does not allow `unsafe-eval`,
third-party connections or remote fonts. GitHub Pages cannot set response
headers; meta CSP cannot set `frame-ancestors`, so an embedding host controls
whether it permits this page in an iframe.

Run `make wasm-pages` to build and scan `dist/` for credential-shaped strings,
debug maps and credential environment variables. Run `make wasm-size` to report
per-file and summed raw, gzip and brotli sizes. Brotli reports `n/a` when its
command is unavailable.

## Size and browser performance

No final artifact or browser performance number is recorded yet. Trunk's build
cannot complete in the current offline environment because Cargo cannot fetch
`minicov`; the checked-in `dist/` is ignored build output and is not a valid
measurement source. CI runs the size report on the final optimized Pages build.

| Measure | Method | Current result |
| :--- | :--- | :--- |
| Artifact bytes | `make wasm-size`; sum each served file after Trunk and wasm-opt | Not measured; Pages build blocked by crates.io DNS |
| Poster load | Chrome navigation timing through `domContentLoadedEventEnd` | Not measured |
| First frame | `draw_web` posts ready after its first frame; the shell marks `televim-ready` when it receives that message | Not measured |
| Key-to-frame | Chrome Performance trace from key event to next frame | Not measured |
| DOM vs WebGL2 | Repeat first-frame and key-to-frame runs with `?renderer=dom` and default WebGL2 | Not measured |

Keep the first successful report as the baseline. Set an advisory size band only
after reviewing that measurement; do not infer a limit from the native binary
budget. The browser artifact is not part of native RSS or executable size.

## Troubleshooting

- **WASM request fails:** confirm the page uses the same base path as `PUBLIC_URL` and the host serves `.wasm` with `application/wasm`.
- **WebGL2 does not start:** the shell switches to DOM and announces the fallback. Add `?renderer=dom` to force it.
- **Clipboard does not copy:** allow clipboard access for the page or verify the app's visible fallback message.
- **File picker does not open:** start selection from Televim's file action after a user gesture. Browser policy may reject synthetic clicks.
- **Keyboard stays in Televim:** use **Release keyboard** or `Ctrl+Alt+Escape`; bare `Esc` belongs to Televim.

## Acceptance status

| Criterion | Repo evidence | Status |
| :--- | :--- | :--- |
| AC1: real TUI in a static WebAssembly page | WASM check and browser-test target compile; Playwright browser run pending CI | Implemented; runtime check pending |
| AC2: offline fictional data and no credential path | Detached dependency tree, simulator tests and artifact audit | Implemented; final Pages artifact audit pending build |
| AC3: supported flows use real app state | Native parity tests and [parity matrix](web-demo-parity.md) | Implemented |
| AC4: every action handled or refused | Simulator action test and action inventory | Implemented |
| AC5: browser, Unicode and accessibility evidence | Unit tests and Playwright specs; browser rendering cannot run locally | Implemented; browser run pending CI |
| AC6: portfolio loads on demand and works at Pages base path | Pages workflow, `/televim/` Playwright setup and [embed contract](web-demo-embed.md) | Repo complete; deployment and external portfolio PR pending |
| AC7: native boundaries and builds stay intact | `make ci`, WASM check and WASM test-target compilation | Native gate passed; browser CI pending |
