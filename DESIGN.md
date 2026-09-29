---
name: Televim
category: Developer Tools
surface: terminal
colors:
  background: "#0b0d10"
  surface: "#141821"
  border: "#3a4150"
  border-focused: "#5ad4e6"
  text: "#d7dce5"
  text-dim: "#7b8496"
  cursor: "#d7dce5"
  match: "#e5c07b"
  selection-bg: "#6b4bab"
  mode-normal: "#4a7fb5"
  mode-insert: "#6fbf73"
  mode-visual: "#e5c07b"
  mode-confirm: "#e06c75"
  border-light: "#b0b5c0"
  border-focused-light: "#1a7f8c"
  text-light: "#1d2128"
  text-dim-light: "#6b7280"
  match-light: "#8a6100"
  mode-normal-light: "#3a6a99"
  mode-insert-light: "#4f9d55"
  mode-visual-light: "#b98d2e"
  mode-confirm-light: "#c0454e"
  on-mode-light: "#0d1117"
  on-mode-dark: "#d7dce5"
---

# Televim

> Category: Developer Tools

> Surface: terminal

> **Status: proposed.** `crates/tui/src/theme.rs` still carries the 16 named ANSI
> colours. The `v2` column below is a proposal and not yet what the binary draws;
> the `baseline` column is. A reader who opens this file and then opens
> `theme.rs` will find two different answers, and this file is the one that looks
> authoritative — so the header has to be flipped to match the code in the commit
> that adopts the palette, not in a later one.

*A private room, read at a glance, typed at without the mouse.*

A Telegram client that shows only the people you talk to — no channels, no
groups, no bots — inside a terminal, driven by Vim keys. The name is Telegram
and Vim run together, and that is the whole proposition: the reading is the
same shape as every other terminal program, and the typing is the same shape as
every other editor.

## The canvas

The terminal's own background is `background`, and the program never paints it.
A client that paints its own background is a client that fights the reader's
colour scheme, and that is the reader's decision. In this document `#0b0d10` is
the colour a mockup is drawn on and nothing else.

`surface` is the inside of a bordered panel. It is specified, and it is also not
painted: a panel is a border and the text inside it, and painting the inside
would be a second background where the reader expects one. It exists so a
mockup has a name for the region rather than reaching for `background`.

## Colour Palette

Twelve roles, because the TUI has twelve. A generic forty-token web palette
would be fiction, and the ones this program does not have are the ones a
designer must not invent either.

Two columns: **baseline** is what a standard 16-colour terminal shows for the
ANSI name in the third column, and is what the binary draws today. **v2** is
the proposal. The light column is v2 re-specified for a reader on a light
terminal.

| Role | ANSI today | baseline (dark) | v2 (dark) | v2 (light) | Why it holds this colour |
| :--- | :--------- | :-------------- | :-------- | :--------- | :----------------------- |
| `background` | — | terminal's own | `#0b0d10` | terminal's own | never painted; a mockup's ground only |
| `surface` | — | terminal's own | `#141821` | terminal's own | a panel's inside, also never painted |
| `border` | `DarkGray` | `#545862` | `#3a4150` | `#b0b5c0` | present, not attention-drawing |
| `border-focused` | `Cyan` | `#00afaf` | `#5ad4e6` | `#1a7f8c` | the *only* thing saying where keys go |
| `text` | `White` | `#e5e5e5` | `#d7dce5` | `#1d2128` | body |
| `text-dim` | `Gray` | `#8a8a8a` | `#7b8496` | `#6b7280` | decoration, hints, an unfocused draft |
| `cursor` | `REVERSED` | reverse video | reverse video | reverse video | owned by the cursor, alone |
| `match` | `Yellow`+`BOLD` | `#87af00` | `#e5c07b` | `#8a6100` | a search hit; colour **and** bold |
| `selection-bg` | `Magenta` | `#af005f` | `#6b4bab` | `#6b4bab` | a Visual selection's background |
| `mode-normal` | `Blue`/`White` | `#0000af` | `#4a7fb5` | `#3a6a99` | the mode label |
| `mode-insert` | `Green`/`Black` | `#00af00` | `#6fbf73` | `#4f9d55` | ditto |
| `mode-visual` | `Yellow`/`Black` | `#afaf00` | `#e5c07b` | `#b98d2e` | ditto |
| `mode-confirm` | `Red`/`White` | `#af0000` | `#e06c75` | `#c0454e` | ditto |

Labels take `on-mode-dark` (`#d7dce5`) on the three dark-enough backgrounds and
`on-mode-light` (`#0d1117`) on the two light ones (`mode-insert`, `mode-visual`).
Naming the foreground makes the label's contrast a property of the theme rather
than of the reader's terminal, which is the only reason it is stated at all.

### Why the refresh

`baseline`'s four mode labels are the raw ANSI primaries. `#0000af` on a dark
terminal is legible; on a light one it is a smear, and the mode label is the one
row that has to be readable in every state. The v2 set is desaturated so it
survives a terminal approximating it — which also means it survives a terminal
that is genuinely 16-colour, because `Color::Rgb` becomes a truecolour escape
that such a terminal maps to whatever it has.

Two costs, stated rather than hidden:

- **Truecolour is required.** On a 16- or 256-colour terminal v2 degrades to the
  terminal's own approximation, which the desaturated values are chosen to
  survive. It degrades; it does not break. `Color::Indexed` was rejected: it
  never looks right on the truecolour terminals that are the majority.
- **`selection-bg` fails a body-text contrast ratio on a light terminal** —
  `#d7dce5` on `#6b4bab` is near 3.3:1, under the 4.5:1 floor. It is kept
  deliberately. A selection is a block, not small text: the reader's job is to
  see its extent, and it fails a text ratio while passing the thing it is for.
  Do not "fix" this by darkening the value; that stops it being a selection
  colour on the dark terminals where this program is actually used.

### Two rules the palette cannot change

Both are asserted by tests today, and a palette that broke them would be a
palette that made the screen lie.

1. **Reverse video belongs to the cursor alone.** `selection-bg` and `match`
   never use `REVERSED`. A second user of it leaves the reader unable to tell
   which row the cursor is on.
2. **`selection-bg` is a background and `match` is a foreground.** A cell that
   is both is then legible rather than one of them winning outright. Verified by
   `a_match_under_a_selection` in `conversation.rs`, which asserts one cell
   carries both.

## Typography

The terminal's own. One monospace family, three weights, no sizes — a terminal
has one cell height and the reader chose the cell.

- **Weight is the only hierarchy.** Regular for body, `bold` for a search match
  and nothing else, `dim` for `text-dim`. No second size and no italic: both are
  unreliable across terminals, so the spec must not depend on them.
- **The column budget is a hard constraint, and it is tested.**
  `MODE_LABEL_WIDTH = 9` — the widest label is `NORMAL`, and a `Confirm` never
  shares the row with a hint, so it is one number rather than a case per label.
  `ASSUMED_WIDTH = 80` — every hint must fit an 80-column terminal less the
  label and its gap, which leaves 70. `ALL_HINTS` is an array of all nine, and a
  test iterates it, so a tenth hint cannot be added and forgotten. **A hint that
  does not fit fails the build; the fix is a shorter sentence, not a wider
  budget.**
- **One cell is one character, and a character may be two cells wide.**
  `wrap::columns` is the table. A row is never cut inside a grapheme cluster —
  `grapheme::cluster_start` / `cluster_end` are what make that true, and a
  delete removes one cluster rather than one code point.
- **`·` stands in for a space in a draft being composed, and only then.** A space
  is a cell that paints nothing, and the caret the terminal draws is a bar on a
  blank cell, so the key that typed one looked like the key that did nothing.
  The dot occupies the space's own cell, which is what keeps the caret and the
  wrap in step. **The conversation gets no dots** — a message is read as prose,
  and a sentence with its spaces dotted reads as something else.
- **A continuation row of a wrapped draft has no prefix and no indent.** The
  `:` or `/` goes on the first row only: it names the whole line, and a reader
  who typed two of them has a question rather than a command.
- **The dots apply to every prompt the reader is typing in**, `:` and `/`
  included. The condition is not *what kind of prompt this is*, it is *is the
  caret in here right now* — the reason the dot exists is that a caret is
  standing on a blank cell, and that is true of a search query too.

### The nine hint rows

`ALL_HINTS` is an array of all nine and a test iterates it, so a tenth cannot be
added and forgotten. These are the nine, verbatim. The column count is
`chars().count()`, which is the number the test asserts against.

| Shown when | Text | Columns |
| :--------- | :--- | ------: |
| conversation, Normal, bar empty | ` i:ins  r:rep  e:edit  dd:del  D:dismiss  v:vis  /:find  ::cmd  q:quit` | 70 |
| chat list has the focus | ` j/k: chat  Enter: open  Tab: pane  h: conversation` | 51 |
| conversation, Visual | ` d: delete  y: yank  r: reply  Esc: cancel` | 42 |
| a deletion is confirmed | ` y: delete  n/Esc: cancel` | 25 |
| the bar holds a draft, not focused | ` ⏎ draft — i to continue, ^J/⏎ to discard` | 41 |
| the line is being typed in | ` ⏎: send  ^J: newline  shift+⏎: newline where supported` | 55 |
| a `:shortcode` is being completed | ` ⇥/⏎: pick  ↑/↓: choose  Esc: close` | 35 |
| the line's own Normal mode | ` i/a: ins  w/b/e  x: del  dw/cc  p: paste  gg/G: ends  ⏎: send  Esc` | 67 |
| a selection inside the line | ` y: yank  d: cut  Esc: back` | 27 |

**A code point is not a cell, and the budget is counted in code points.** `⏎`
`⇥` `↑` `↓` `…` are East Asian *Ambiguous*: one cell in most terminals, two on
one configured for a CJK locale. The test counts code points, so it
under-counts exactly the hints that use them — the worst case is
`COMPLETION_HINT` at 35 code points and 39 cells. That is still inside 71, so
it is not a bug today, and it is stated here rather than left to be discovered
by a reader whose hints are clipped. **If a hint ever needs the room, count
cells, not code points** — `wrap::columns` is the function that does it
correctly, and it is already a dependency.

**One of the nine is unreachable.** `CONFIRM_HINT` cannot be displayed: a
confirmation outranks every hint, so `status_text` returns the confirmation's
sentence before it ever reaches `hint()`. The string is width-checked and shown
by nothing, which makes `ALL_HINTS`'s count a weaker claim than the array exists
to make. Deleting it belongs to the change that adds a tenth hint — the settings
panel — because that is when `ALL_HINTS` is resized anyway. Until then, nine is
the number of *constants*, not the number of hints a reader can see.

Three notes, because they are the answers to questions a reader will have:

- **`NORMAL_HINT` at 70 and `LINE_NORMAL_HINT` at 67** are the two longest, one
  and three columns under the 71 available. They took that room deliberately:
  reply, edit, delete and `gg`/`G` all landed by shortening the mode keys, never
  by letting the row run past the bar and be clipped. **`gg` and `G` are in the
  line's hint because the editor has neither.** A key the line answers with
  nothing else on screen naming it is a key the hint exists for.
- **The line's Normal mode has no `j`/`k`, and that is not an oversight.** The
  editor behind it makes those history navigation on a one-line buffer, and
  history is not built. The caret moves between the lines of a message from
  Insert mode, where the arrows are motions.
- **`^J` before `shift+⏎`**, because `^J` works in every terminal and needs no
  protocol, and a shifted `Enter` is only distinguishable where the terminal
  volunteers it.

## Layout

```
┌─ 30% ────────┬─ 70% ─────────────────────────────────────┐
│  Chats (12)  │  Conversation (3/48) · 2 selected          │  Constraint::Min(3)
│              │                                           │
│  border-     │  [them] a message wraps at the panel's    │
│  focused when│         width — at whitespace where there  │
│  it has focus│         is whitespace, and at the edge     │
│              │         where there is not                 │
├──────────────┴───────────────────────────────────────────┤
│ ┌ Message to Ada Lovelace ┐                              │  Length(2 + rows)
│ │ draft text, · for spaces, real caret                   │  capped at 6
│ └───────────────────────────────────────────────────────┘
│  INSERT   ⏎: send  ^J: newline  shift+⏎: newline where…   │  Length(1)
└──────────────────────────────────────────────────────────┘
```

- **30 / 70 by percentage, three rows.** The top region is `Min(3)`, the bar is
  `Length(2 + rows)`, the status line is `Length(1)`.
- **The bar is always a draft, never absent.** It holds what the reader last
  typed, or a hint. It grows to six rows (`INPUT_MAX_ROWS`) because a draft is
  the reader's own words and the bar is the only place they can be read back —
  but past six, the conversation is what the reader is reading.
- **The focused pane's border is the only thing on screen that says where a
  keystroke goes.** Two panes drawn alike are two panes the reader has to guess
  between. `lit_borders` in `conversation.rs` reads the three corners and asserts
  exactly one is `border_focused` for each of the three focus values.
- **Titles are space-padded.** ` Chats (12) `, ` Conversation (3/48) `, ` Input `,
  ` draft `, ` Message to Ada Lovelace `, ` Reply `, ` Edit `, ` Command `,
  ` Find `.
- **The scrollbar is a column the body gives up**, and only above
  `MIN_BODY_WIDTH = 8`. A narrow panel has no room to give: the bar would cost
  more than it tells the reader.
- **Nothing is cached between frames.** A message is as tall as its text needs at
  the width the panel gave it, and the viewport, the scrollbar, `Ctrl+d`/`Ctrl+u`
  and the fetch triggers all count those rows. A cache is a second thing to keep
  in step with the window, which is the failure this arrangement exists to
  prevent.
- **A real caret, drawn by the terminal.** A `█` painted as a character is one
  the reader cannot tell from a real one, and it cannot go backwards through what
  is already on screen. `TestBackend` does not model it — the arithmetic is
  tested, the cursor is checked by hand.

## Vocabulary

This is the section a designer is most likely to get wrong, because a chat UI
has strong conventions this one deliberately does not follow. Verbatim from the
code.

| Written | Means | Source |
| :------ | :---- | :----- |
| `[you]` `[them]` | which side a message is from, on its first row | `rows.rs` |
| `> quoted ‖ body` | a reply: quote and body **on one row**, `‖` between | `rows::reply_prefix` |
| `> [message not loaded] ‖ body` | a reply whose target the window does not hold | asserted in `rows.rs` |
| `[sending…]` | on the message's **last** row, which is the one with room | asserted |
| `[failed: no route]` | ditto, with the reason, truncated to the room | `rows::status_suffix` |
| `· 2 selected` | joined onto a panel title by `·` | `selection_note` |
| `· 3 match(es)` | a search's count, in the title | `search_note` |
| `·` | a space in a draft being composed; a title-note joiner | two meanings, both deliberate |
| `Loading…` `Loading older…` `Loading newer…` | a fetch in flight | `FetchDirection::label` |
| `Jumping to first unread…` | a jump the window could not answer | `JUMP_LABEL` |
| `televim` | the resting status line, when there is nothing to say | `IDLE_STATUS` |
| `Quit televim? (y/n)` | a screen-wide confirmation | `QUIT_PROMPT` |
| `Delete your message from both sides? (y/n)` | ditto, naming **which side** | `DELETE_OUTGOING_PROMPT` |
| `Delete 3 of your messages? (y/n)` | a count, and pluralisation | `delete_yours_prompt` |
| `NORMAL` `INSERT` `VISUAL` `CONFIRM` | upper case, in a filled label, reverse-video cursor | `status_bar.rs` |
| ` i:ins  r:rep  e:edit ` | hints: `key:word`, two spaces between pairs | the nine rows above |
| `: ` `/ ` | a prompt's prefix, on its first row only | `App::prompt_prefix` |
| `3 message(s) selected — Esc clears` | the status line's sentence for a selection, **stating the unit** | `selection_note` |
| `3 character(s) selected — Esc clears` | ditto for a selection inside one message | ditto |
| `/query — match 2 of 5` | a search: the query, then where the walk is | `SearchState::label` |
| `/query — 2 loaded` | a **local** search, which is never an answer | `local_position` |
| `/query — 2 loaded — searching…` | a local search with the server still working | ditto |
| `/query — match 1 of 5 (first 2)` | a server walk that is still capped | `server_position` |

**No emoji in the chrome.** The emoji catalog is for the reader's *own text*, and
`emojis` costs 0.5 MB of binary and 2 MB of RSS. A `👤` in a settings title is
the one place that cost would be spent for nothing.

## Components

Six, with their states, and the `TestBackend` assertion that would catch each
regressing.

1. **Panel** — `Chats`, `Conversation`, `Input`. States: focused, unfocused,
   empty, titled with a count, titled with notes.
2. **Message row** — states: incoming, outgoing, reply, sending, failed, search
   match, char-selected, message-selected, wrapped across rows.
3. **Mode label** — the four above, plus the line's own `VISUAL` and `NORMAL`,
   which are *different modes that share a word*. The label belongs to whichever
   mode the keys about to be pressed will mean, and a confirmation is a question
   about the whole screen.
4. **Hint row** — nine variants, one per state of the screen, all under
   `ASSUMED_WIDTH - MODE_LABEL_WIDTH`. Three keys mean something else while a
   `:shortcode` completion is up, and the status line names them for as long as
   it is.
5. **Prompt** — `Command`, `Find`, `Reply`, `Edit`, and the bar's own title
   switching between `Message to …` and ` draft `.
6. **Status line** — the ranking, which is the whole of it: a **confirmation**
   outranks a **selection** outranks a **search**, and a `flash` is not a state
   at all. A refusal written while any of the three is up is a line the reader
   never sees. That is why an operation that finishes in Visual leaves Visual,
   and why a prompt carries its counts rather than flashing them. Below the three
   sit a jump in flight, the full reason a failed message failed, and then
   whatever was last written to the status.

   **Above all of them is a keystroke inside the line.** A key being pressed
   cannot be answered by a sentence about a state the reader is in the middle of
   changing, so the hint wins. The full order, highest first:

   | Rank | What it says | Example |
   | ---: | :----------- | :------ |
   | 1 | a key inside the line — the line's own hint | ` i/a: ins  w/b/e  …` |
   | 2 | a confirmation | `Quit televim? (y/n)` |
   | 3 | a selection, **with its unit** | `2 message(s) selected — Esc clears` |
   | 4 | an active search | `/bench — match 1 of 2` |
   | 5 | a jump in flight | `Jumping to first unread…` |
   | 6 | why the message under the cursor failed | the send's own reason |
   | 7 | a `flash` — a refusal, a status worth reading | anything just written |
   | 8 | the hint, or `televim` | the resting state |

   The unit in rank 3 is not decoration: three characters and three messages are
   both "3", and a reader who has just pressed `v` has to be able to tell which
   of the two they are holding.

## Principles

Six, each argued for in a code comment or an `AGENTS.md` section. Not
aspirational: each is a decision the code makes and a test asserts.

1. **Refuse rather than answer a different question.** A key that answered a
   different question than the one asked, while the screen said something else,
   is worse than a refusal. `r` in Visual says no, in two distinct wordings,
   because its two reasons are genuinely different.
2. **Nothing typed is ever lost to an `Esc`.** Two stages: stop typing, then
   leave. `Ctrl+w` steps back out of the line from any of its modes without
   throwing anything away.
3. **A refusal says why, and an unhandled key is *dropped*, not refused.** A
   dropped key is silence, and silence is the one bug this rule exists to
   prevent: `gg` and `G` were dropped by the editor before they were
   implemented, and they arrived as nothing happening.
4. **A screen that cannot draw cannot ask.** `Ctrl-C` does **not** confirm — it
   is the way out when the program is wedged, and a terminal that is not
   answering cannot draw the question either.
5. **The honest fallback beats the pretty one.** No cache, no second source of
   truth, no layout computed twice. A frame is built from the current window or
   it is not built.
6. **The lightest version that works.** The runtime is current-thread, chosen
   explicitly even though `tokio`'s `full` features are on. `jemalloc` is
   declared and not installed. Declared-but-unused is named as a gap, never
   counted as a feature.

## Motion

There is none, deliberately.

A frame is drawn from a shared reference and the layout is rebuilt per frame, so
an animation is a second thing to keep in step with the window. The only
temporal elements in the whole interface are the terminal's caret, the
scrollbar's thumb, and the cursor row's reverse video — all three the
terminal's, not ours. A spinner or a countdown would be the first thing in this
programme that repainted on a schedule rather than on a change, and it is not
worth it.

## What this document is for

It is the spec the rest of the design work implements, and it is tracked beside
`AGENTS.md` for the same reason: a design system that lives only inside a tool
drifts from the code with nothing to catch it. When a change makes a section
here wrong, the change is incomplete until this file is updated in the same
commit.
