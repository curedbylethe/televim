# Memory

The memory budget: what is in place, what is declared but not installed, and
what nobody has measured. The 50 MB ceiling is a **target**, not a verified
number — see "not in place" below before quoting any memory claim.

## What is in place

- `tokio` **current-thread** runtime: `Builder::new_current_thread()` in
  `app/src/runtime.rs`, explicitly, even though the `full` feature set is enabled.
- A bounded conversation window: `domain::history::ConversationWindow` caps at
  `CONVERSATION_WINDOW` messages, so full history is never held.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`. The stripped binary is currently **5.2 MB**, well inside the
  15 MB target — about half a megabyte of which is the emoji catalog.

What is declared but **not** in place, and so cannot be relied on:

- `tikv-jemallocator` is not installed as the global allocator. The binary runs on
  the system allocator.
- No arena allocator is used for MTProto deserialization. `bumpalo` is not a
  dependency.
- No `heaptrack`/`valgrind` step measures RSS in CI, so the 50 MB ceiling is a
  target nobody has measured yet.

### Detailed Rationale

1. **Current-thread runtime:** a TUI has one event loop, so a work-stealing
   runtime is overhead with nothing to schedule. Offload to `spawn_blocking` only
   where work genuinely blocks.
2. **Bounded window:** the cap is what keeps the process's largest allocation
   bounded under a live feed. See the `domain` section for why it is one flat
   window rather than one per conversation.
3. **Release profile:** `lto = "fat"` and `codegen-units = 1` buy size and speed
   at the cost of build time, which is the right trade for a distributed binary.
4. **Zero-copy where possible:** translating framework types into domain types
   should use `Cow<'_, str>` and byte slices rather than cloning strings, so the
   domain types stay lightweight DTOs.
5. **Allocator work is still ahead.** `jemalloc` and an arena for the parse path
   are the intended answer to long-tail fragmentation; neither is built, so treat
   any memory claim as unverified.
