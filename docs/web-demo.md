# Web demo

The detached `crates/web-demo` feasibility crate renders the real `tui::App` through Ratzilla. It depends on `domain` and `tui`; it does not depend on the native app or Telegram crates.

Build details and the backend feasibility record live in [`crates/web-demo/FEASIBILITY.md`](../crates/web-demo/FEASIBILITY.md). Feature evidence lives in the [parity matrix](web-demo-parity.md).

## Unicode and bidi

The browser renderer draws one glyph per cell. Televim's bidi modes reorder text for display; they do not shape Arabic or join letters across cells. A browser screenshot is evidence for the demo, not proof of terminal compatibility.

The demo sets `BidiMode::Visual`, which reorders mixed-direction rows before drawing. `BidiMode::Terminal` leaves reordering to a bidi-capable terminal and is not the browser default. `BidiMode::Words` reverses right-to-left word order for terminals that shape each run.

WebGL2 uses Ratzilla's dynamic glyph atlas; the DOM renderer is the accessible fallback. DOM text can shape glyphs within a cell, but adjacent cells still do not join. A missing WebGL2 glyph must use the DOM renderer rather than a replacement character.

Composer edits and Backspace operate on grapheme clusters. Selection and yank preserve logical text order. ZWJ emoji can occupy four cells under `unicode-width` even when displayed as two; the TUI keeps those cells, and `Buffer::diff` handles VS16. Do not drop joiners, variation selectors or combining marks to make the browser frame fit.

The mobile hidden input sends committed IME text as Televim keys. Intermediate composition input is ignored; Enter and Backspace wait until composition ends. Browsers without composition events use the input event fallback. Playwright synthetic composition checks are added in Stage 11.

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
