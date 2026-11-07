# Web demo

The detached `crates/web-demo` feasibility crate renders the real `tui::App` through Ratzilla. It depends on `domain` and `tui`; it does not depend on the native app or Telegram crates.

Build details and the backend feasibility record live in [`crates/web-demo/FEASIBILITY.md`](../crates/web-demo/FEASIBILITY.md). The final browser shell and run instructions follow in later implementation stages.
