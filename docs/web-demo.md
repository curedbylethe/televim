# Web demo

The detached `crates/web-demo` feasibility crate renders the real `tui::App` through Ratzilla. It depends on `domain` and `tui`; it does not depend on the native app or Telegram crates.

Build details and the backend feasibility record live in [`crates/web-demo/FEASIBILITY.md`](../crates/web-demo/FEASIBILITY.md). Feature evidence lives in the [parity matrix](web-demo-parity.md).

## Unicode and bidi

The browser renderer draws one glyph per cell. Televim's bidi modes reorder text for display; they do not shape Arabic or join letters across cells. A browser screenshot is evidence for the demo, not proof of terminal compatibility.

The demo sets `BidiMode::Visual`, which reorders mixed-direction rows before drawing. `BidiMode::Terminal` leaves reordering to a bidi-capable terminal and is not the browser default. `BidiMode::Words` reverses right-to-left word order for terminals that shape each run.

WebGL2 uses Ratzilla's dynamic glyph atlas; the DOM renderer is the accessible fallback. DOM text can shape glyphs within a cell, but adjacent cells still do not join. A missing WebGL2 glyph must use the DOM renderer rather than a replacement character.

Composer edits and Backspace operate on grapheme clusters. Selection and yank preserve logical text order. ZWJ emoji can occupy four cells under `unicode-width` even when displayed as two; the TUI keeps those cells, and `Buffer::diff` handles VS16. Do not drop joiners, variation selectors or combining marks to make the browser frame fit.

The mobile hidden input sends committed IME text as Televim keys. Intermediate composition input is ignored; Enter and Backspace wait until composition ends. Browsers without composition events use the input event fallback. Playwright synthetic composition checks are added in Stage 11.

The evidence suite covers Persian, Arabic, mixed-direction text, combining marks, emoji, search prefixes, composer editing and logical-order yank through the real `App`. Browser rendering checks remain unrun locally because Trunk is unavailable and this environment cannot reach crates.io. Real OS terminal shaping, RTL terminal emulators and hardware keyboards across mobile systems are not covered.
