# Memory

The memory budget: what is in place, what is declared but not installed, and
what has been measured. The numbers below come from a real run of
`make measure` on the machine named in the environment block, and are stored
machine-readably in [`memory-baseline.json`](./memory-baseline.json). The
[regression budget](#the-regression-budget) below is what keeps them from
quietly going backwards.

## Measured baseline

Taken on a release build (`lto = "fat"`, `codegen-units = 1`, `strip = true`,
`panic = "abort"`), median of five `make measure` runs — which is what
`docs/memory-baseline.json` records. The baseline was re-recorded when STAGE-03/04
added `unicode-bidi`'s Unicode tables: binary size moved by +66,080 B and nothing
else in the tree grew with it. It was re-recorded again when PR-CUR-13 shipped
static stickers: binary size moved by +82,880 B — the dedicated WEBP decoder and
the hand-rolled half-block painter, both weighed against heavier candidates
first — and that anchor alone moved. It was re-recorded a third time with the local history cache:
binary size moved to 6,189,696 B, and again that anchor alone moved (see
[The history cache](#the-history-cache)). Three runs beside the sticker tree measured
harness input latency at 2.37 / 0.76 / 0.78 ms against the 0.509 ms anchor while
frame time read 1.59 / 0.51 / 0.52 ms against 0.510 ms; the same binary's RSS
swung ±20% between those runs, the first ran during disk-pressure recovery and
the third beside a concurrent `make ci`, and nothing in the sticker diff runs on
the harness's text-only hot path (a match arm and a small vector per message).
That is a host settling, not a regression, so the latency anchors stay: moving
one on three noisy runs is what the bands exist to prevent, and they keep
enforcing until a clean-host run confirms.

| Metric | Target | Measured | Verdict |
| :--- | :--- | ---: | :--- |
| RSS at idle, harness at 60 chats | < 50 MB | **3.06 MB** | inside |
| RSS at idle, shipped binary, unauthorised | < 50 MB | **7.08 MB** | inside |
| Startup, launch → first frame (empty/sign-in frame) | < 500 ms | **0.778 ms** | inside |
| Startup, harness launch → populated screen | — | **1.058 ms** | recorded |
| Input latency, key→draw (real binary) | < 16 ms | **0.063 ms** | inside |
| Input latency, key→draw (harness, 50 keypresses) | < 16 ms | **0.509 ms** | inside |
| Frame time, harness | — | **0.510 ms** | recorded |
| Binary size, stripped release | < 15 MB | **5,759,280 bytes** (5.49 MB) | inside |

### Two corrections to how these figures were taken

Both were found by an audit of the first version of this harness, and both
changed the numbers rather than just the wording.

**The "real binary" run was a signed-in session.** `measure.py` used to pass the
whole environment through and start the binary in the repository root. The root
`.env` is gitignored and holds `TELEGRAM_API_ID`, `TELEGRAM_API_HASH`,
`TELEVIM_SESSION_PATH` and `TELEVIM_PHONE`, and `main.rs` calls
`dotenvy::dotenv()` — so the run fetched a real chat list over the network and
its RSS was a function of an account's history. This file described that figure
as "no account: the chat list is empty", which was false.

The measured process now runs in a **sandbox outside the repository**: a
temporary directory with its own `HOME`, `TMPDIR`, session path and an empty
config, with only `PATH`/`TERM`/`LANG`-class variables inherited and every
`TELEGRAM_*`/`TELEVIM_*` name dropped. With no application credentials the
program builds no client at all (`net::spawn_bring_up` returns `NoCredentials`
before constructing anything), so the run takes the unauthorised path
deterministically on any machine. The evidence is in the report's environment
block: `credentials_reaching_the_run: false`, the list of dropped names, and
`outside_repository: true`.

The sandbox is outside the repository rather than under `target/` because
`dotenvy::dotenv()` searches the working directory **and every parent of it** —
a sandbox at `target/measure/sandbox` still found the root `.env`, which is
exactly the failure this is fixing. That was caught by checking the run's log for
a salt exchange: with credentials present, `grammers` performs one even before
signing in. The log now contains only the probe lines.

The cost of this correction is that the "real binary" RSS figure fell by roughly
half. The old figure was measuring a connected client — its buffers, its TLS
state, its datacenter connections — none of which is present in the
unauthorised state the report claims to describe.

**`valgrind --tool=massif` was wrapping the compared run on Linux.** Under
valgrind the harness's own `VmRSS` reading reports *valgrind's* memory, and
every timing is valgrind's, so the compared metrics were a distortion — worse on
CI, where valgrind is more likely to be present than on a developer machine. The
harness now always runs directly on every platform. A massif pass exists as an
opt-in diagnostic (`make measure` with `--massif`), reported separately as
`massif_diagnostic` with `compared: false` and never thresholded.

**Environment.** `Darwin arm64`, Darwin 25.5.0; `rustc 1.98.1
(48a229cea 2026-09-01)`, host `aarch64-apple-darwin`, LLVM 22.1.8; toolchain pin
`channel = "1.98.1"`; release profile as above; `Cargo.lock` sha256
`8d313aba49edc58d…`; 8 CPUs. RSS read in-process from `proc_pidinfo`
(`/proc/self/status` on Linux), and from `/proc/<pid>/status` (`ps rss`) for the
binary. No `valgrind`, `heaptrack`, or `hyperfine` on this host. The block also
records the confounds that decided the figures above: credential and session
presence, the sandbox's isolation, how many launches each figure came from, what
the first-frame number excludes, and that no profiler took part.

**"Idle" is defined operationally**, because otherwise the word means nothing:
after the 60-chat / 250-message load is applied and drawn, 2,000 ms of quiet
with no keypress pending and no frame in flight, then five RSS samples 200 ms
apart, of which the median is the number above.

**Two RSS figures, not one**, because no single process in this tree is the
program under load. The harness is 60 chats with the screen fully drawn, but
none of the network half — no `grammers`, no session, no update stream. The
binary is the whole shipped program, but unauthorised and with an empty chat
list: a populated list needs a Telegram round trip, which a credential-free run
deliberately cannot make. So the binary's figure bounds the program from below
and the harness's bounds the loaded screen, and the number the program reaches
with 50 chats is between them and has not been measured. Any figure quoted as
"RSS at idle after 50+ chats" should say which of these it is.

### What the startup and latency figures do not include

Two words in this project's own vocabulary need narrowing, because the
measurement is narrower than the words.

**First frame is the empty/sign-in frame, not the chat list.** The loop draws
before any network round trip, so the frame being timed is the one that says the
program has nothing to connect as. The README's < 500 ms target is unchanged and
still the contract; this note is about which frame the number describes. The
"populated load" row is the harness's substitute, and it measures a synthetic
60-chat screen rather than a fetched one. A launch with a warm history cache
draws the cached chat list in its first frame, still before any round trip, but
the measured launch has no cache, so its first frame is still the empty one.

**Input latency is key→draw, not end-to-end.** The probe spans from the loop
taking the keypress to the completion of the draw that shows its effect. Two real
costs sit outside it: the reader thread's blocking `crossterm::event::read`,
which is where a keystroke waits for the terminal to deliver it, and the
terminal's own paint, which happens after `Terminal::draw` returns. Both are in
the harness's `not_measured` list as well as here.

### The 20–30 MB stretch target: not attempted

Recorded, not chased and not abandoned. Measured RSS is **below** the 20–30 MB
band on both figures, which is a third answer from "met" — the stretch asks for
a process between 20 and 30 MB, and the binary's figure is a different thing
from either end of it. The range was not changed.

### The harness

`make measure` (driver: `scripts/memory/measure.py`, harness:
`crates/app/examples/memory_harness.rs`). It builds a synthetic load through
`tui`'s public API only — never the `#[cfg(test)]` sample data, which no
dependent crate can see — draws frames into an in-memory terminal, and writes
`target/memory-report.json`. An `[[example]]` rather than a second binary
because `cargo build --release` does not build examples, so nothing in it can
reach the shipped artifact.

`docs/memory-baseline.json` is the stored baseline, and it is recorded by
hand, never by the driver: `check.py --record-baseline` takes the median across
several reports, so one slow run cannot anchor a rule above what the tree
normally costs. It also carries a `cross_invocation` block, and every metric
carries its sample count — so the figures behind the baseline can be recomputed
from the tree rather than taken on trust.

The release binary is launched **five times** per run and its startup reported
warm. This was not in the first version of the harness and it mattered: a single
launch reads 10–17 ms because it is paging the binary in from disk, while the
launches after it read ~0.7 ms. A figure that swings an order of magnitude on
page-cache state is not a measurement of the program, and a threshold calibrated
on one would be a threshold on disk speed. The cold figure is still recorded, as
`first_frame_cold_ms`, and the report says how many launches the warm median came
from — so `startup_first_frame_ms` can never be mistaken for a single sample
with a spread of zero.

### Noise floor

A regression threshold has to be wider than this or it fires on the machine
rather than on the code. The driver keeps the last twenty invocations'
per-metric medians in `target/measure/history.jsonl` and reports the aggregate
in `cross_invocation`; recording a baseline pools every run recorded into it, and
the per-invocation values are carried alongside so the spread can be recomputed
from this file rather than trusted.

| Metric | Typical | Range across 20 invocations |
| :--- | ---: | ---: |
| RSS, harness | 3.06 MB | 2.52 – 3.47 MB |
| RSS, binary | 7.08 MB | 6.97 – 8.23 MB |
| Startup, first frame | 0.721 ms | 0.696 – 1.851 ms |
| Startup, populated load | 1.058 ms | 0.993 – 2.748 ms |
| Input latency, harness | 0.509 ms | 0.500 – 0.515 ms |
| Input latency, binary | 0.057 ms | 0.053 – 0.141 ms |
| Frame time | 0.511 ms | 0.502 – 0.513 ms |
| Binary size | 5,676,400 B | exact on every run |

On the **warm timings** this spread is small — under 3% on every latency and
frame figure — and zero on binary size. The RSS figures and the two startup
figures scatter wider, and are named as such in the bands below rather than
folded into one number:

- **Harness RSS has a low mode.** Most runs sit near 3.2–3.5 MB, but eight of the
  twenty recorded invocations land between 2.5 and 2.9 MB, with all five samples
  inside a run agreeing exactly. The disagreement is between processes: macOS not
  faulting every page in. It does not threaten the budget, which fails only on an
  *increase* and this scatter is downward — but a rule that failed in either
  direction would be reporting the page-in schedule.
- **The first launch of a run is cold.** It pages the binary in from disk, and
  what that costs depends on the page cache: this recording read 1.93 ms cold
  against 0.778 ms warm, while an earlier baseline taken with a cold cache read
  12.7 ms against 0.728 ms. That is why the figure is the median of launches 2–5
  and the cold one is recorded separately as `cold_ms`.

The frame-time and harness-latency figures are the stable ones. **Binary size is
stable to the byte** — clean release builds from scratch produce identical sizes
— which is why it carries the tightest band in the budget.

## The regression budget

The declared targets above are the contract and are unchanged. The budget is a
separate, tighter guard: how far each measured figure may rise above the stored
baseline before CI fails. It lives in
[`scripts/memory/thresholds.json`](../scripts/memory/thresholds.json) and is
enforced by `scripts/memory/check.py` (`make measure-check`), which reads the
report `make measure` wrote, the stored baseline, and the thresholds, and
writes a metric / measured / baseline / delta table to the job summary.

A band reads as `allowed = max(relative × baseline, absolute)`, and the check
fails when `measured > baseline + allowed`. **Only an increase is a
regression** — a figure that shrinks is never a failure, which is why a run
whose RSS lands low passes instead of firing.

| Metric | Baseline | Allowed growth | Threshold |
| :--- | ---: | ---: | ---: |
| RSS@idle, harness (60 chats) | 3,211,264 B | +1.05 MB | 4.26 MB |
| RSS@idle, shipped binary | 7.078 MB | +3.0 MB | 10.08 MB |
| Startup, first frame | 0.778 ms | +2.5 ms | 3.28 ms |
| Startup, populated load | 1.058 ms | +2.5 ms | 3.56 ms |
| Input latency, harness | 0.509 ms | +0.102 ms | 0.611 ms |
| Input latency, shipped binary | 0.063 ms | +0.15 ms | 0.213 ms |
| Binary size | 5,676,400 B | +56,764 B | 5,733,164 B |

Three of these are as tight as the noise allows and one is not, and the
difference is the point:

- **Binary size** is reproducible to the byte — three clean release builds gave
  identical sizes — so its band is 1%, about what a small new dependency costs.
  This is the budget's sharpest instrument.
- **Input latency (harness)** has a ~3% spread, so a 20% band is six times the
  noise and still under a quarter of the declared 16 ms budget.
- **The two startup figures** carry a spread well above their medians, so their
  bands are wide in both terms. They catch a genuine order-of-magnitude
  regression in the launch path and nothing finer.
- **The shipped binary's RSS** is the loosest in relative terms (+3.0 MB). It was
  calibrated against a *connected* binary, where the spread was what the network
  half happened to hold resident; the unauthorised binary is steadier, but the
  band was not tightened on the strength of five runs, so it stays loose.

### Calibration

The bands were not asserted; each was checked by injecting a real regression,
rebuilding, and watching the check fail on the figure it moved. These trials were
**re-run after the two corrections above**, because the first set was calibrated
against a baseline the audit had shown was a signed-in session:

| Injected | Measured | Baseline | Delta | Threshold | Result |
| :--- | ---: | ---: | ---: | ---: | :--- |
| harness retains an extra 6 MB | 9,945,024 B | 2,965,504 B | **+235.4%** | 4,014,080 B | **fails** |
| 5 ms of work before the first draw | 7.857 ms | 1.273 ms | **+517.2%** | 3.773 ms | **fails** |
| 512 KB `#[used]` static in the binary | 6,138,704 B | 5,610,320 B | **+9.4%** | 5,666,423 B | **fails** |

(The first two rows were measured against an intermediate three-run baseline
taken before the five-run re-record; the injections and the verdicts are
unaffected by the later change of anchor, and both figures are named above so
the comparison is not mistaken for one against the current baseline.)

Each failure message names the metric, the measured value, the baseline, the
delta, and the threshold. Reverted after the trial: `grep` confirms no injection
marker remained, and the binary returned to exactly 5,610,320 bytes.

Against an unregressed tree, **nine consecutive `make measure` +
`make measure-check` cycles passed and none failed**, with margins well clear of
the thresholds *of the baseline in force then*: first frame 0.55–0.81 ms against a
3.23 ms threshold, populated load 1.27–1.48 ms against 3.91 ms, binary RSS
7.91–8.28 MB against 11.25 MB, harness latency 0.511–0.540 ms against 0.618 ms, and
binary size exact every time. Those thresholds are the ones the table above has
since moved with its baseline; the bands behind them are unchanged.

### What the re-baseline invalidated

Stage 2's bands were anchored to figures the audit invalidated. The trials above
were re-run against the corrected baseline, and these bands were left in place
rather than re-tuned:

- **`rss_idle_binary_bytes` fell from 12.02 MB to 8.25 MB**, and the CUR-28
  re-record put it at 7.078 MB — the connected client was nearly a third of the
  old figure. Its band (+3.0 MB) now yields a 10.08 MB threshold, still loose.
- **`startup_first_frame_ms` fell from 2.403 ms to 0.728 ms** on the warm median.
  Its +2.5 ms floor was calibrated against a *cold* figure and is now several
  times the warm value; that is deliberate, because the cold launch still happens
  once per session and a band that tight would catch the page cache, not the code.
- **`input_latency_ms` rose from 0.030 ms to 0.066 ms**, because the unauthorised
  loop is not also doing MTProto work between the keypress and the draw.
- **Harness RSS** moved from 3.41 MB to 3.41 MB — unchanged, and worth noting,
  since the audit's correction was to a *different* process.

No band was tightened on this evidence. Each remains the widest that passes the
trials, and the cold-launch effect is the reason the two startup bands stay loose.

### What the budget cannot see

It is a change detector, not a memory model. It cannot tell a leak that grows
slowly from a leak that grows quickly — a 1 MB/year drift sits inside every band
here — and it only measures on the platform it was recorded on: see below.

### One baseline per host

A baseline is only comparable to a run on the platform it was taken on, and CI
runs `ubuntu-latest` while this baseline was recorded on `darwin-arm64`. The
stored baseline names its `platform`, and the check enforces only when that
matches the running host. On a host with no stored baseline it **reports and
does not fail**, because a threshold cannot be calibrated on a machine it was
never measured on; the run still writes its report, which a maintainer reads and
records with `check.py --record-baseline`. So CI's first run on a new host is a
recording run, and enforcement begins once that baseline is committed.

Each baseline is the **median across several `make measure` runs** (five in
`docs/memory-baseline.json`), not one. Since the budget only fails on an
increase, a baseline captured during a single unusually slow run would sit high
and silently disable the rule anchored to it — which is exactly what happened
while calibrating this one.

### What the harness does not measure

- **Allocation counts.** No counting global allocator is installed, so the
  report names bytes and frames rather than a count of `alloc` calls; installing
  one would be a change to what the harness measures rather than a reading of it.
- **Heap fragmentation.** RSS does not distinguish a fragmented heap from a
  large one.
- **Cache behaviour**, the history cache's included. Every run is a cold-cache
  launch: the sandbox holds no history file, and with no credentials nothing is
  ever written to one. A warm launch parses the file before its first frame and
  draws the cached chat list in it, so neither the parse nor that frame is in the
  startup figure, and the cache's resident cost is in no RSS figure — see
  [The history cache](#the-history-cache).
- **Anything inside `grammers`**, including MTProto decoding, which happens in
  an external crate this workspace does not build.
- **Windows.** There is no portable way to ask a process for its resident size,
  so a Windows run reports `null` rather than a number of the wrong kind. This
  is a platform gap, not a pass.

## What is in place

- `tokio` **current-thread** runtime: `Builder::new_current_thread()` in
  `app/src/runtime.rs`, explicitly, even though the `full` feature set is enabled.
- A bounded conversation window: `domain::history::ConversationWindow` caps at
  `CONVERSATION_WINDOW` messages, so full history is never held.
- A bounded history cache: at most `HISTORY_CACHE_DEPTH` (200) messages for each
  of at most `HISTORY_CACHE_PEERS` (32) peers, and `HISTORY_CACHE_CHATS` (500)
  chat-list rows, in memory and in the file beside the configuration — see
  [The history cache](#the-history-cache).
- Measurement-only `Instant` probes in the event loop, gated on
  `TELEVIM_MEASURE` and written to the log rather than the screen, so a launch's
  first frame and a keypress's latency have numbers behind them.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`.

### The session file's key derivation

A passphrase is stretched with Argon2id at `m` = 19 MiB, so a launch that uses
`TELEVIM_SESSION_PASSPHRASE` makes one transient allocation of about 19 MiB while
the key is derived, and frees it when the derivation returns. The key is kept for
the salt it was derived with, so a load followed by a save pays once per process,
not once per call; a `:retry` or a reconnect resolves a fresh provider and pays
again. The harness and the shipped-binary measurement run with no credentials, so
**this peak is not in any RSS figure above** and nothing here claims a number for
it; it is a declared allocation, not a measured one. A launch that uses the keyring
key instead derives nothing.

What the change does move is binary size, which the regression budget measures.
A stripped release build of the tree just before this change was 6,007,456 B and
of the tree with it 6,057,248 B: **+49,792 B (+0.83%)** for `aes-gcm-siv`, `argon2`
and what they pull in, inside the 1% band (about 57.6 KB). The stored baseline
(5,759,280 B) is older than both builds, and the tree had already drifted past its
band before this change (+248,176 B), so `make measure-check` reports a binary-size
breach that this change did not cause. The baseline was not re-recorded here; that
is a maintainer decision.

### The media cache

The media cache (`app/src/media_cache.rs`) is bounded on disk, not in memory:
`MEDIA_CACHE_MAX_BYTES` is 1 GiB and `MEDIA_CACHE_MAX_ENTRIES` is 256 files. Resident,
it holds one index entry per cached file — a message key, a path, a size and a
modification time, about 150 bytes — so about 40 KB at the entry cap. A download's
bytes are held in memory as they were before the cache, bounded by `MEDIA_LIMIT`
(16 MiB), and written off the loop. The disk worst case is the byte cap plus one file
of up to 16 MiB in flight during a write.

These are **declared, not measured**, and the RSS budget is unaffected by the disk bound.

### The history cache

The cache (`app/src/history_store.rs`) is resident for the whole run and copied
for every write, and it is the first structure in the program whose size is set
by what the reader has read rather than by what is on screen. Its bounds are rows,
not bytes: 200 messages × 32 peers is **6,400 rows at most**, plus 500 chat-list
rows. Placeholders, presence and media bytes are never held; the media kind is.

| | Typical | Worst case |
| :--- | ---: | ---: |
| Messages, in memory | about 1 MB | about 80 MB |
| Messages, on disk | about 1 MB | about 160 MB |
| Chat list, in memory and on disk | about 100 KB | about 6 MB |
| A write, briefly | about 3× the cache | about 3× the cache, more for heavily escaped text |

These are **declared, not measured**. The typical figure is 6,400 rows of an
ordinary message's size. The worst case is every cached message at Telegram's
4,096-character limit in three-byte UTF-8 — about 12 KB a message — and on disk,
text made entirely of control characters, which JSON escapes to six bytes each.
The chat list's worst case is 500 previews of that same length. A write holds the
cache, the clone the encoder takes of it and the serialised bytes at once, the
clone dropped once the bytes exist; a launch holds the file's bytes and the
parsed payload together while it reads. The feed's *seen* map
(`FeedMarks::seen`) is outside the peer bound: 16 bytes of payload, plus the
map's node overhead, for each peer that received an arrival in the session, and
never evicted until the process ends.

**The worst case is past the 50 MB budget, and that is a known gap, not a
claim that it fits.** At about 80 MB resident and about three times that during
a write, a reader whose 32 most recent conversations were all maximum-length
messages would take the process well over the ceiling. The typical case adds
about a megabyte resident and a few megabytes briefly per write, inside the
margin the measured figures leave. The upgrade path is a byte budget beside the
row bounds — evicting by size as well as by count — and it is named in
[`known-gaps.md`](./known-gaps.md) rather than built.

What the change moves that the harness does measure is binary size. Measured on
this host (Darwin 27.0.0, `arm64`, the pinned `rustc 1.98.1`), two `make measure`
runs each of this tree's `main` and of the tree with the cache, release profile as
above:

| Metric | Before | With the cache | Delta |
| :--- | ---: | ---: | ---: |
| Binary size, stripped release | 6,073,840 B | 6,189,696 B | **+115,856 B (+1.91%)** |
| RSS at idle, shipped binary, unauthorised | 8.48 / 8.50 MB | 6.97 / 7.00 MB | no increase |
| RSS at idle, harness at 60 chats | 3.95 / 3.98 MB | 1.74 / 3.98 MB | no increase |
| Startup, first frame | 1.343 / 1.561 ms | 1.653 / 1.704 ms | +0.14 to +0.36 ms |
| Input latency, harness | 0.770 / 0.760 ms | 0.758 / 0.759 ms | none |

Binary size is exact on every run, and the **+115,856 B is past the budget's 1%
band** (about 57.6 KB of the stored baseline) on its own: the store's logic and
`serde`'s derived code for its three row types, with no new dependency. The RSS
figures are a credential-free, cold-cache launch that never builds a cache, so
they could not show its cost; the binary's lower reading is the host's bimodal
RSS (recorded above as 6.97–8.23 MB), not something this change saved. The first
frame now also asks for a file that is not there, which is a single failed
`open`; the difference is inside the startup band's +2.5 ms and inside the spread
both trees showed. Every timing on this host reads above the stored baseline on
**both** trees — the host moved from Darwin 25.5.0 to 27.0.0 since that baseline
— so they compare the two trees to each other, not to the baseline.

`make measure-check` against the stored baseline reports two breaches, and
neither is this change's alone: harness input latency (0.758 ms against a
0.611 ms threshold), which `main` breaches by the same amount on this host, and
binary size (6,189,696 B against 5,816,873 B), which `main` was already past by
257 KB. The binary-size anchor was then re-recorded to this tree's exact
6,189,696 B, a maintainer decision taken with this change; the timing and RSS
anchors were left where they were, because what moved them is the host, not the
tree, and re-anchoring them here would hide that. With the binary anchor moved, the
check's remaining breaches are both input latency: the harness figure reads
0.745–0.765 ms on both trees, and the shipped binary's 0.145–0.228 ms on `main`
against 0.212–0.225 ms here, so either tree lands on either side of its
0.213 ms threshold from one run to the next.

### No arena, by measurement

No arena allocator is used anywhere, and the baseline above is why rather than a
preference. The render pass's allocations are transient — dropped at the end of
the frame that made them — so they cannot move either RSS figure, which the
long-lived window and the terminal buffers carry. A frame costs **0.510 ms**
against the 16 ms budget, so there is no time to recover either. And the arena
cannot be threaded through `App::row_layout(&self) -> Vec<RowSpan>`
(`tui/src/app.rs:2231`) or `conversation::render(..., &[RowSpan])`
(`tui/src/widgets/conversation.rs:52`) without pushing a lifetime into the
public widget API. The candidate sites it was evaluated against — the outer
`Vec<RowSpan>` and one `Vec<Range<usize>>` per windowed message
(`tui/src/app.rs:2231-2295`), the per-frame `Vec<ListItem>` and title strings,
`rows::reply_prefix`, called by the layout (`tui/src/rows.rs:680`) and again by the painter (`tui/src/widgets/conversation.rs:502`), and the
fetch/translate triple `Vec<Message>` path (`telegram-framework/src/history.rs:172`,
`proto/src/history.rs:151-164`, `domain/src/history.rs:182-192`) — are all
outside an arena that cannot be handed one without that bleed. MTProto decoding
is inside `grammers`. Recorded in [`decisions.md`](./decisions.md); `bumpalo` is
not a dependency. One drift left deliberately: the README's first project goal
still names an arena among the means of staying under the ceiling. That line is
a stated goal rather than a claim about the tree, and rewriting a goal to match
a measurement would be moving the target — so this paragraph is the record of
what was actually found.

### The global allocator is `std::alloc::System`

Chosen against the baseline above, and the alternatives were `tikv-jemallocator`
and `mimalloc`. The tree is already inside both ceilings with two to three times
of margin — **5,759,280 B** stripped against < 15 MB, and **3.06 MB** / **7.08 MB**
RSS@idle against < 50 MB — so a candidate would have to give some of it back to
be worth adopting. The workload is not the one those allocators target: a
single-threaded current-thread `tokio` plus one reader thread, not many threads
against a fragmenting heap. The choice could not be measured on this host either
— macOS RSS moves by roughly 2x between runs with no code change, so a
candidate's delta is inside the noise rather than a result — and
`std::alloc::System` is the one answer that is the same on macOS, Linux and
Windows and needs no C toolchain on any of them. Windows is not exercised in CI
and is named as a platform gap.

`#[global_allocator]` does not exist, and if it ever did it would be declared in
`crates/app/src/main.rs` — the composition root — because an allocator is a
property of the binary and a library that chose one would choose it for every
dependent. `tikv-jemallocator` was in `[workspace.dependencies]` until this
decision: declared, with no dependents and no `Cargo.lock` entry, so a name in a
manifest for a decision nobody had made. Removing it changed nothing in the build
— `Cargo.lock` is byte-identical. Recorded in
[`decisions.md`](./decisions.md).

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
5. **Two RSS figures, deliberately.** A single number would have to claim to be
   the program under load, and neither candidate process is. The gap between
   them is the honest state of the measurement. It is now closable rather than a
   product question: the chat-list fetch retries and `:retry` re-runs the bring-up,
   so a launch holding a session can populate the list — `make measure` still
   weighs a binary launched with every credential name dropped, which is what makes
   the figure reproducible, so closing the gap means a second launch that keeps
   one. That has not been taken.
6. **No arena, and it was measured rather than skipped.** Bump allocation is the
   usual answer to long-tail fragmentation, so the candidate was evaluated against
   the baseline above instead of being waved off. Every render-pass allocation is
   dropped at the end of the frame that made it, so none of them is what the RSS
   figures are made of, and the frame already costs 0.510 ms of a 16 ms budget.
   The API would have to change to hand one over. A profile that later shows a
   fragmenting heap is what would reopen this.
