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
  selection: "#d7dce5"
  caret-insert: "#d7dce5"
  caret-normal: "#d7dce5"
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
  caret-insert-light: "#1d2128"
  caret-normal-light: "#1d2128"
  match-light: "#8a6100"
  mode-normal-light: "#6d9dd0"
  mode-insert-light: "#4f9d55"
  mode-visual-light: "#b98d2e"
  mode-confirm-light: "#d67b83"
  on-mode-light: "#0d1117"
---

# Televim

> Category: Developer Tools

> Surface: terminal

> **Status: proposed.** `crates/tui/src/theme.rs` still carries the 16 named ANSI
> colours. The palette below lands in `02-theme-palette.md`. Until that commit,
> the `baseline` column is what the binary draws and the `v2` column is the
> target; after it, `v2` is what it draws.

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
the colour a frame is drawn on and nothing else.

`surface` is the inside of a bordered panel. It is specified, and it is also not
painted: a panel is a border and the text inside it, and painting the inside
would be a second background where the reader expects one. It exists so a
frame has a name for the region rather than reaching for `background`.

## Colour Palette

Fifteen roles, because the TUI has fifteen. A generic forty-token web palette
would be fiction, and the ones this program does not have are the ones a
designer must not invent either.

Two columns: **baseline** is what a standard 16-colour terminal shows for the
ANSI name in the third column, and is what the binary draws today. **v2** is
the proposal. The light column is v2 re-specified for a reader on a light
terminal. Only the dark column ships: the light column's numbers are decided
and tested, `Theme` has one set of values, and a second palette is a later
change.

| Role | ANSI today | baseline (dark) | v2 (dark) | v2 (light) | Why it holds this colour |
| :--- | :--------- | :-------------- | :-------- | :--------- | :----------------------- |
| `background` | — | terminal's own | `#0b0d10` | terminal's own | never painted; a frame's ground only |
| `surface` | — | terminal's own | `#141821` | terminal's own | a panel's inside, also never painted |
| `border` | `DarkGray` | `#545862` | `#3a4150` | `#b0b5c0` | present, not attention-drawing |
| `border-focused` | `Cyan` | `#00afaf` | `#5ad4e6` | `#1a7f8c` | the *only* thing saying where keys go |
| `text` | `White` | `#e5e5e5` | `#d7dce5` | `#1d2128` | body |
| `text-dim` | `Gray` | `#8a8a8a` | `#7b8496` | `#6b7280` | decoration, hints, an unfocused draft |
| `selection` | `REVERSED` | reverse video | reverse video | reverse video | the row the cursor is on, in a read-only surface; owned by it alone |
| `caret-insert` | — | terminal's own | `#d7dce5` | `#1d2128` | the bar, while the line is composed; on the bar's plain ground |
| `caret-normal` | — | terminal's own | `#d7dce5` | `#1d2128` | the hollow; `text` on a plain ground, `background` on the reversed row |
| `match` | `Yellow`+`BOLD` | `#87af00` | `#e5c07b` | `#8a6100` | a search hit; colour **and** bold |
| `selection-bg` | `Magenta` | `#af005f` | `#6b4bab` | `#6b4bab` | a Visual selection's background |
| `mode-normal` | `Blue`/`Black` | `#0000af` | `#4a7fb5` | `#6d9dd0` | the mode label |
| `mode-insert` | `Green`/`Black` | `#00af00` | `#6fbf73` | `#4f9d55` | ditto |
| `mode-visual` | `Yellow`/`Black` | `#afaf00` | `#e5c07b` | `#b98d2e` | ditto |
| `mode-confirm` | `Red`/`Black` | `#af0000` | `#e06c75` | `#d67b83` | ditto |

All eight mode labels, the four dark-column and the four light-column, are
`on-mode-light` (`#0d1117`). One label ink, in both columns, and no split. On a
dark terminal that is 4.50:1 on `mode-normal`, 8.45:1 on `mode-insert`, 10.96:1
on `mode-visual` and 5.92:1 on `mode-confirm`. On a light terminal it is 6.65:1
on `mode-normal-light` (`#6d9dd0`), 5.67:1 on `mode-insert-light`, 6.23:1 on
`mode-visual-light` and 6.33:1 on `mode-confirm-light` (`#d67b83`). The old
split put four of the eight under the body-text floor: `#d7dce5` was 3.06:1 on
`mode-normal` and 2.32:1 on `mode-confirm`, and 4.12:1 and 3.63:1 on the old
light fills `#3a6a99` and `#c0454e`. Those fills were also 3.34:1 and 3.79:1
against `#0d1117`, which is why they moved. Naming the foreground makes the
label's contrast a property of the theme rather than of the reader's terminal,
which is the only reason it is stated at all.

The two carets are the body-text value in both columns, so they clear the
body-text floor by the same margin `text` does: `#d7dce5` on the dark ground is
14.14:1, and `#1d2128` on a white light ground is 16.15:1. On the reversed row
their ink is `background`, the reverse of that same pair, which is the only ink
that survives the row's own swap. The values are listed twice because every role
is specified twice; the two carets differ in shape, never in ink, and the rule
that fixes which of the two inks applies is below.

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
- **Text on a selection follows the selection, not the terminal.** `selection-bg`
  is `#6b4bab` in both columns, so the text on it is always `#d7dce5`, and that
  pair is 4.74:1. It passes. The light column's own text ink `#1d2128` on
  `#6b4bab` would be 2.48:1. The same rule covers a search match under a
  selection: it takes the pale ink too, because the light match `#8a6100` on
  `#6b4bab` is 1.18:1. Neither is reversed.

### Two rules the palette cannot change

Both are asserted by tests today, and a palette that broke them would be a
palette that made the screen lie.

1. **Reverse video belongs to the cursor alone.** `selection-bg` and `match`
   never use `REVERSED`. A second user of it leaves the reader unable to tell
   which row the cursor is on.
2. **`selection-bg` is a background.** A cell that is also a match keeps that
   background and takes the pale ink (`#d7dce5`), not the match colour, which is
   what keeps it legible. Neither is reversed. Verified by
   `a_match_under_a_selection` in `conversation.rs`, which asserts one cell
   carries both.

### The three names, and the caret rule

Three visuals, three names, and no synonyms. The engine's flag legend, the
stylesheet's classes and this document use the same three words, and the table is
the port list: the field each one becomes in `crates/tui/src/theme.rs`.

| The visual | What it is | Field | Flag | Class |
| :--------- | :--------- | :---- | :--- | :---- |
| **`selection`** | the row the cursor is on, in a read-only surface: reverse video on the row itself | `Theme::selection` (kept) | `r` | `.rv` |
| **`caret-insert`** | a two-column bar: the line, while it is being composed | `Theme::caret_insert` (added) | `c` | `.ci` |
| **`caret-normal`** | a hollow cell: the line's own Normal mode, and a card's inline position | `Theme::caret_normal` (added) | `n` | `.cn` |

**The rule, in three parts.**

1. **A read-only surface marks its cursor with reverse video on the row itself,
   and nothing else is reversed.** This is the rule above, unchanged, and
   `selection-bg` still never uses `REVERSED`: a second user of it leaves the
   reader unable to tell which row the cursor is on.
2. **A caret drawn inside a reversed row is the ground colour**, because that row
   has already taken `selection` — which is `text` — as its background. A caret
   in `text` there would be `text` on `text`. The ground is the only ink that
   survives the swap, and that is the whole reason there are two ink values and
   not one.
3. **A surface that is not reversed draws its caret in `text`.** The bar is the
   common case and it is always plain: an insert caret while the line is
   composed, and a normal caret in the line's own Normal mode. The card's inline
   position is the opposite case, and it is the only one: it is drawn inside the
   reversed row and takes the ground colour. The plain case is met once more on a
   card, where a charwise selection makes the row the `selection-bg` background
   rather than reverse video; the hollow takes `text` there too, by the same
   rule.

**The repo asks the terminal for its caret today**, in `input_bar.rs`, and it
must stop and paint instead. A terminal cursor has one shape, and the two names
above are exactly the two shapes it cannot be at once: the bar while Insert runs
and the hollow in the line's own Normal mode. Asking for a block gives a filled
block in both, so the state the reader is most often in — the line's Normal mode —
is the one the artifact and the repo disagree about. Painting the carets also
makes them the same kind of object the card's inline position already is: a cell
`TestBackend` can assert, rather than a request the backend does not model.

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
  label and its gap, which leaves 71. `ALL_HINTS` is the array of all fourteen,
  and a test iterates it, so a hint left out of the array is a hint the check
  never sees. **A hint that does not fit fails the build; the fix is a shorter
  sentence, not a wider budget.**
- **One cell is one character, and a character may be two cells wide.**
  `wrap::columns` is the table. A row is never cut inside a grapheme cluster —
  `grapheme::cluster_start` / `cluster_end` are what make that true, and a
  delete removes one cluster rather than one code point.
- **`·` marks a cell that holds something the screen would otherwise show as
  nothing, in three places, and only those three.** In a draft being composed a
  space is a cell that paints nothing, and the insert caret is a two-column bar
  on a blank cell, so the key that typed one looked like the key that did
  nothing; the dot occupies the space's own cell, which is what keeps the caret
  and the wrap in step. `·` also joins a panel's title to a note (`· 2 selected`).
  And it is a card row's cue: one column, present, dim, on the row's first line,
  saying the row carries something. **The conversation gets no dots** — a message
  is read as prose, and a sentence with its spaces dotted reads as something else.
- **A continuation row of a wrapped draft has no prefix and no indent.** The
  `:` or `/` goes on the first row only: it names the whole line, and a reader
  who typed two of them has a question rather than a command.
- **The dots apply to every prompt the reader is typing in**, `:` and `/`
  included. The condition is not *what kind of prompt this is*, it is *is the
  caret in here right now* — the reason the dot exists is that a caret is
  standing on a blank cell, and that is true of a search query too.

### The fourteen hint rows

`ALL_HINTS` is the array of all fourteen and a test iterates it. These are the
fourteen, verbatim, with the width each one measures. The width is
`chars().count()`, the same count the test makes, and the array is the authority:
the numbers below were re-measured from it.

| Shown when | Text | Columns |
| :--------- | :--- | ------: |
| conversation, Normal, bar empty | ` i:ins  r:rep  e:edit  dd:del  v:vis  /:find  ::cmd  A:card  S:acct` | 67 |
| chat list has the focus | ` j/k: chat  Enter: open  Tab: pane  h: conversation  A:card  S:you` | 66 |
| conversation, Visual | ` d: delete  y: yank  r: reply  Esc: cancel` | 42 |
| a deletion is confirmed | ` y: delete  n/Esc: cancel` | 24 |
| the bar holds a draft, not focused | ` ⏎ draft — i to continue, ^J/⏎ to discard` | 41 |
| the line is being typed in | ` ⏎: send  ^J: newline  shift+⏎: newline where supported` | 55 |
| a `:shortcode` is being completed | ` ⇥/⏎: pick  ↑/↓: choose  Esc: close` | 35 |
| the line's own Normal mode | ` i/a: ins  w/b/e  x: del  dw/cc  p: paste  gg/G: ends  ⏎: send  Esc` | 67 |
| a selection inside the line | ` y: yank  d: cut  Esc: back` | 27 |
| the editable profile has the focus | ` e/Enter: edit  j/k: field  Esc/Tab/Ctrl+w: leave` | 49 |
| the self card has the focus | ` j/k: row  h/l: within  v: vis  y: yank  d: act  Esc: back` | 58 |
| a contact's card has the focus | ` j/k: row  h/l: within  v: vis  y/yy: yank  Esc: back` | 53 |
| the session is not signed in | ` ::signin  q:quit` | 17 |
| nothing has been read yet | ` q:quit` | 7 |

Notes, because they are the answers to questions a reader will have:

- **The subtraction starts from the binary's 67, not the 70 this document once
  quoted.** The row it used to print carried `D:dismiss` and did not carry
  `S:acct`. `D:dismiss` is gone — dismissing a failed send is a *refusal*, and a
  refusal is the status line's to say, not the hint's, which has no room for why —
  and `S:acct` is present. Eleven columns left the row and eight arrived, so the
  row the arithmetic starts from is three columns shorter than this document
  assumed.
- **`A:card` costs eight columns and `q:quit` pays for it: 67 − 8 + 8 = 67, four
  columns spare.** A hint is the only place a reader learns a key the rest of the
  screen is silent about, and `A` is one. `q` is the only one of the three keys
  the reader uses most — `e:edit`, `v:vis`, `q:quit` — that has a second route to
  the same action: `:q` and `:quit` reach it, and `::cmd` is named on the same
  row. `e` and `v` have no other mention anywhere on screen, so they keep theirs.
  The cost is real and it is visible: a reader who has not learned `q` now has to
  learn `:q`.
- **What is tighter is the pair of keys, not the row.** The binary's row is three
  columns shorter than this document assumed, so it is the *easier* row to name a
  key on, and the corrected arithmetic says so: four columns spare, not one. The
  part that is genuinely tighter than before is that `A:card` and `S:acct` must
  both be named, `S` has no second route either, and neither may be dropped for the
  other — two keys where the old note counted one.
- **`S:acct` is on the conversation's row and `S:you` is on the chat list's, and
  that one key has two spellings is a rule rather than a slip.** `S` opens your
  own profile from either pane, and, like `A`, only a hint names it. The chat list
  spells it among the people, where you are one of them; the conversation spells
  it for the account the card is. A reader who meets the second spelling has to
  reconcile it with the first, so both are named deliberately and neither is
  renamed to match. The list's row is 66 of 71 with `A:card` and `S:you` in it.
- **The two card rows differ because their rows differ.** The self card names
  `d`, because it has two rows that act, and names `y` but not `yy`. A contact's
  card names `y/yy`, because that card is read and `yy` is the verb worth
  teaching, and it does not name `d` at all. Each names only keys its own card
  answers: a key named on a card that does not answer it tells the reader the key
  is wrong rather than that it is not shown.
- **`gg` and `G` are in the line's hint because the editor has neither.** A key
  the line answers with nothing else on screen naming it is a key the hint exists
  for.
- **The line's Normal mode has no `j`/`k`, and that is not an oversight.** The
  editor behind it makes those history navigation on a one-line buffer, and
  history is not built. The insert caret moves between the lines of a message from
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
│ │ draft text, · for spaces, insert caret                 │  capped at 6
│ └───────────────────────────────────────────────────────┘
│  INSERT   ⏎: send  ^J: newline  shift+⏎: newline where…   │  Length(1)
└──────────────────────────────────────────────────────────┘
```

- **30 / 70 by percentage, three rows.** The top region is `Min(3)`, the bar is
  `Length(2 + rows)`, the status line is `Length(1)`.
- **The right column holds two contents.** The conversation and a profile card
  are the same region: `l` (or `A`) swaps the card in, `Esc` swaps it back, and
  the chat list gives up nothing. `Tab` and `Ctrl-w h`/`Ctrl-w l` walk the two
  panes; inside a card `h`/`l` are inline, which is why the pane keys carry the
  `Ctrl-w` prefix there. A card is not a stack and is not remembered: opening it
  again lands on its first row, where the conversation keeps the reader's place.
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
- **The carets are painted, not asked for.** The three names, the rule they obey
  and the port list are above. A `█` the program paints is one the reader cannot
  tell from the terminal's own, and the terminal's own cannot be a bar in Insert
  and a hollow in Normal. Painting them makes both the same kind of cell the
  card's inline position already is, so `TestBackend` asserts them and section 09
  of the specimen shows them.

### Groups, day separators and read state

A message is still as many rows as its text needs, the sender label still sits
on its first row and a status still sits on its last. Three treatments sit on
top of that and add no row kind a message already had. Every rule below is
conditional on a message having a time: **a message with no time never groups,
never gets a separator and never shows a timestamp**, so a row drawn before these
treatments existed reads exactly as it did.

**Grouping.** Consecutive messages join one visual group when every one of these
holds against the message before: the same side, the same calendar day, no more
than five minutes (`GROUP_MIN`) between the two (the gap is to the *previous*
message, not to the group's first), and the later one is not a reply. A change
of direction, a reply, a day boundary or a longer gap always starts a new group.
A reply starts a group and may be followed into it.

- **The sender label, `[you]` or `[them]`, is on the group's first row only.**
  A later message in the group has a blank 7-column tag and its text begins in
  the same column, so the group reads as one turn. A reply's quoted target stays
  on the reply's first row, which is always a group's first row.
- **The time is on the group's last row only**, right-aligned in the status
  column: `21:04`. It is not repeated per message.
- **The cursor still selects a message.** The row fill and the selection follow
  `mi`, so the cursor can sit on any message of a group and the group does not
  select as one.
- **A message's own status is never hidden by grouping.** `[sending…]` and
  `[failed: …]` stay on the last row of the message that carries them, mid-group
  or not. Only the read state and the time are the group's.

**Read state (outgoing only).** An outgoing message carries what the peer has said
about it: nothing yet, `delivered` (acknowledged, not read) or `read`. It is drawn
**once per group, on the last row of the group's newest message**, left of the
time: `[delivered] 21:16`, `[read] 21:04`. A message with a status, `sending…` or
`failed: …`, shows that status in the same place **instead**, and a failed message
never claims delivered or read, however the peer's last answer stood. An
outgoing group still sending shows `[sending…] 21:32`; the earlier messages'
receipts are not shown. An incoming message has no read state, only a time. If
the state and the time do not fit beside the last line of text, they take a row of
their own, as a status always has.

| Last row of a group | Draws |
| :------------------ | :---- |
| outgoing, peer has read the newest | `[read] 21:04` |
| outgoing, acknowledged, not read | `[delivered] 21:16` |
| outgoing, newest is sending | `[sending…] 21:32` |
| outgoing, newest failed | `[failed: no route] 21:38` |
| outgoing, no answer from the peer yet | `21:40` |
| incoming | `20:14` |

**Day separators.** The first message of each calendar day is preceded by one row
naming the day, sitting exactly between the last message of the day before and the
first of the next, and above the first message of a chat's history. The label is
`Today`, `Yesterday`, the weekday name for the five days before that, and a full
date past the week: `Sep 20, 2026`.

- **It is a row of its own, the full width of the text** (52 columns, the width a
  message's tag and body occupy together): a rule of `─` in the border colour with
  the label set into its middle in the dim colour, padded by one space each side.
  It has no tag column, no fill, no sender label, no reply target and no status. It
  is not a bubble and not a message row.
- **It is not a message.** In the flat row list it is an entry with `mi: -1`, the
  way a `Loading…` row is, so the cursor, a selection and a search never land on
  it; it counts as one row for the viewport and the scrollbar. When the cursor is
  on the first message of a day, the separator above it is kept in view.
- **Group boundaries and separators agree.** A separator is always a group break,
  because a day boundary is.

## Vocabulary

This is the section a designer is most likely to get wrong, because a chat UI
has strong conventions this one deliberately does not follow. Verbatim from the
code.

| Written | Means | Source |
| :------ | :---- | :----- |
| `[you]` `[them]` | which side a message is from, on its first row; in a group, on the group's first row only | `rows.rs` |
| `> quoted ‖ body` | a reply: quote and body **on one row**, `‖` between | `rows::reply_prefix` |
| `> [message not loaded] ‖ body` | a reply whose target the window does not hold | asserted in `rows.rs` |
| `[sending…]` | on the message's **last** row, which is the one with room | asserted |
| `[failed: no route]` | ditto, with the reason, truncated to the room | `rows::status_suffix` |
| `[delivered]` | an outgoing group the peer's client has acknowledged and not read; the group's **last** row, beside any time | specified here; not yet in `rows.rs` |
| `[read]` | an outgoing group the peer has read; same row, same place | ditto |
| `21:04` | a group's time, `HH:MM`, once, on the group's **last** row, right-aligned after any state | specified here; not yet in `rows.rs` |
| `──── Today ────` | a day separator: a rule the width of the text with the day set into it | specified here; not yet in `rows.rs` |
| `· 2 selected` | joined onto a panel title by `·` | `selection_note` |
| `· 3 match(es)` | a search's count, in the title | `search_note` |
| `·` | a space in a draft being composed; a title-note joiner; a card row's cue | three meanings, all deliberate |
| `Loading…` `Loading older…` `Loading newer…` | a fetch in flight | `FetchDirection::label` |
| `Jumping to first unread…` | a jump the window could not answer | `JUMP_LABEL` |
| `televim` | the resting status line, when there is nothing to say | `IDLE_STATUS` |
| `Quit televim? (y/n)` | a screen-wide confirmation | `QUIT_PROMPT` |
| `Delete your message from both sides? (y/n)` | ditto, naming **which side** | `DELETE_OUTGOING_PROMPT` |
| `Delete 3 of your messages? (y/n)` | a count, and pluralisation | `delete_yours_prompt` |
| `NORMAL` `INSERT` `VISUAL` `CONFIRM` | upper case, in a filled label | `status_bar.rs` |
| ` i:ins  r:rep  e:edit ` | hints: `key:word`, two spaces between pairs | the fourteen rows above |
| `: ` `/ ` | a prompt's prefix, on its first row only | `App::prompt_prefix` |
| `3 message(s) selected — Esc clears` | the status line's sentence for a selection, **stating the unit** | `selection_note` |
| `3 character(s) selected — Esc clears` | ditto for a selection inside one message | ditto |
| `/query — match 2 of 5` | a search: the query, then where the walk is | `SearchState::label` |
| `/query — 2 loaded` | a **local** search, which is never an answer | `local_position` |
| `/query — 2 loaded — searching…` | a local search with the server still working | ditto |
| `/query — match 1 of 5 (first 2)` | a server walk that is still capped | `server_position` |
| `Profile · you` `Profile · Ada Lovelace` | a card's title; the `(n/m)` is the row the cursor is on | `cardTitle` |
| `Profile` | a card with no subject: not signed in, or nothing read yet | `cardTitle` |
| `· name        Noor Haddad` | a card row: the cue, the label, the value | `CUE` |
| `not signed in` | the shell card's first line, then the reason, wrapped | `NOT_SIGNED_IN` |
| `reading the session…` | the shell card **before anything has been read**; it does not borrow the other one's wording | `READING` |
| `Set the credentials again with :signin.` | where to put the credentials back | `SET_CREDENTIALS` |
| `signed out` | the status the screen rests on after the reader signs out, until the sign-in field arrives | the screen after a sign-out |
| `Sign out and forget this session? (y/n)` | a screen-wide confirmation, raised by the `logout` row; `y` signs out | the card |
| `not yet: this build cannot add an account` | the `add account` row's refusal | ditto |
| `Not yours: a contact card has no row you can act on.` | `d` on a contact's card | ditto |
| `Not a row that acts: add account and logout are the two.` | `d` on a value row of the self card | ditto |
| `Not editable here: :settings opens the editable profile.` | `e` on the self card: the one route to the editable screen | ditto |
| `p: a card has no buffer to paste into` | why `p` is unbound, written to the hint's own row | ditto |
| `1 row(s) yanked — register filled, OSC 52 offered` | `yy`: the register first, then the offer to the terminal | ditto |
| `3 row(s) selected — Esc clears` | a selection of rows, stating the unit | `cardSel` |
| `Profile · editable` | the other screen: it edits, the cards read | `drawSettings` |
| `Sign in to Telegram` | the sign-in view's first line | `drawSignin` |
| `Phone` `Login code` | the two rows every sign-in has, in order | ditto |
| `two-factor password (3 attempts left)` | the third row, drawn **only** when Telegram answered `SESSION_PASSWORD_NEEDED`; the count starts at three and drops on each refusal | ditto |
| `Password hint: …` | the account's own hint, drawn under the password step when it has one | ditto |
| `Checking…` | a request is in flight; `⏎` is refused and a second `⏎` does nothing | ditto |
| `still checking — the answer is on its way` | the one `⏎` a request in flight will answer | ditto |
| `[ok]` / `[wrong code]` | a row Telegram accepted, and one it refused | ditto |
| `[ ⏎: sign in again ]` | the row's offer after `AUTH_KEY_UNREGISTERED` | ditto |
| `cancelling discards the code Telegram sent; ⏎ asks for a new one` | `Esc` at the code step, naming what it costs | ditto |
| `the code did not survive; ⏎ asks for a new one` | back after `Tab`/`Ctrl+w`: the step is kept, the code is not | ditto |
| `televim has no application credentials. …` | no `api_id`/`api_hash`: a sentence, not a form | `drawNoCreds` |
| `that code is not the one Telegram sent` | `PHONE_CODE_INVALID` | `AUTH` |
| `that code has expired — ⏎ asks for a new one` | `PHONE_CODE_EXPIRED` ✱ | ditto |
| `that is not a phone number Telegram will accept` | `PHONE_NUMBER_INVALID` | ditto |
| `Telegram has banned that number` | `PHONE_NUMBER_BANNED` | ditto |
| `too many attempts — wait, then try again` | `PHONE_NUMBER_FLOOD` | ditto |
| `that password is not right (2 attempts left)` | `PASSWORD_HASH_INVALID`, with the row's own count put in | ditto |
| `this account has no two-factor password` | `PASSWORD_MISSING` | ditto |
| `this session was revoked — sign in again; and, for the log, the stored session is discarded` | `SESSION_REVOKED` ✱ | ditto |
| `the stored session is no longer valid — sign in again` | `AUTH_KEY_UNREGISTERED` ✱ | ditto |
| the raw error text | anything else, verbatim | ditto |

**`SESSION_PASSWORD_NEEDED` is not a refusal**: it is the answer that puts the password row up.
The three sentences marked ✱ are Telegram's own doing or the account's own doing, so they never
say "you"; nothing else in the sign-in view does either.

**No emoji in the chrome.** The emoji catalog is for the reader's *own text*, and
`emojis` costs 0.5 MB of binary and 2 MB of RSS. A `👤` in a settings title is
the one place that cost would be spent for nothing.

## Components

Eight, with their states, and the `TestBackend` assertion that would catch each
regressing.

1. **Panel** — `Chats`, `Conversation`, `Input`. States: focused, unfocused,
   empty, titled with a count, titled with notes.
2. **Message row** — states: incoming, outgoing, reply, sending, failed, search
   match, char-selected, message-selected, wrapped across rows. Also: first or
   later in a group (the label on the first only), last of a group (the time, and
   for an outgoing one `[delivered]` or `[read]` in place of nothing, `[sending…]`
   or `[failed: …]` in place of both), and the **day separator** beside it, a row
   that is not a message.
3. **Mode label** — the four above, plus the line's own `VISUAL` and `NORMAL`,
   which are *different modes that share a word*. The label belongs to whichever
   mode the keys about to be pressed will mean, and a confirmation is a question
   about the whole screen.
4. **Hint row** — fourteen variants, one per state of the screen, all under
   `ASSUMED_WIDTH - MODE_LABEL_WIDTH`. Three keys mean something else while a
   `:shortcode` completion is up, and the status line names them for as long as
   it is. The settings hint and the card hints lose to a confirmation and to an
   active search, the same way the other hints do.
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
   | 3 | a selection, **with its unit** | `2 message(s) selected — Esc clears`, or `3 row(s) selected — Esc clears` on a card |
   | 4 | an active search | `/bench — match 1 of 2` |
   | 5 | a jump in flight | `Jumping to first unread…` |
   | 6 | why the message under the cursor failed | the send's own reason |
   | 7 | a `flash` — a refusal, a status worth reading | anything just written |
   | 8 | the hint, or `televim` | the resting state. The editable profile and both cards use their own hint here, not `televim` |

   The unit in rank 3 is not decoration: three characters, three rows and three
   messages are all "3", and a reader who has just pressed `v` has to be able to
   tell which of the three they are holding.

7. **Profile card** — one widget over two subjects, drawn in the right column.
   States: the self card, a contact's card, the selection row, the inline position,
   a charwise selection, a selection of rows, a value wrapped over rows, a row
   the peer did not give (absent, not empty), a dim action row, a reserved slot
   drawn as nothing, and the two shell states (not signed in, nothing read yet).
   What would catch it regressing: the selection row is reverse video and the
   normal caret *on it* is the ground colour, so the reader can always see where
   the cursor is; and the title's `(n/m)` counts the rows the peer gave, so a
   row that disappears when a privacy setting hides it changes the count.

8. **Sign in** — the one view that is not the chat: a box over the right column, two rows, and a
   third only when Telegram asks for it. States: the phone (pre-filled from configuration when
   one is set), `Checking…` (a request in flight, `⏎` refused once and a second `⏎` doing
   nothing), the login code, a refused code (`[wrong code]`), the two-factor password with its
   `(n attempts left)` counter, a refused password, the cancelled code (`Esc`), the paused flow
   after `Tab`/`Ctrl+w` (the step survives and the code does not, and the return says so), and
   the stale session whose row offers `[ ⏎: sign in again ]`. A no-2FA account has two rows for
   the whole flow: the password row is never drawn speculatively. When the machine has no
   `api_id`/`api_hash` the view is not a form at all — it is a sentence and there is no phone
   row, because a field the reader cannot use is worse than no field. What would catch it
   regressing: the password row is absent at steps 0 and 1, and the `(n attempts left)` on it is
   the same three the refusals decrement.

## Profile

**A profile row is the same kind of object a message is.** That is the whole of
this screen, and it is not a new idea: it is the conversation's model,
generalised. The conversation already has a cursor on an item, a charwise
selection inside one item, `v` to select, `y` to yank, `j`/`k` between items,
`gg`/`G` the ends, and `d` to delete. The profile panel is that widget over
different items, so one interaction model serves a chat, a settings card and a
person.

What differs between "my profile" and "someone else's profile" is then a
**capability** question, not a layout question: *is this row yours to change?*
The self card has two rows that act and refuses inside both; a contact's card has
none, and says so. Nothing else about the two cards differs, and neither is a new
widget.

### The row

A row is one of three kinds. Every row carries the same one-column cue on its
first line — `·`, present, dim, and blank on a wrapped row's continuation lines.
It is the device the conversation already uses when it puts `[you]` or `[them]`
on a message's first row: a mark on the row it belongs to, in the row's own
columns. The conversation writes a word; a card writes one cell.

| Kind | Draws | Answers |
| :--- | :---- | :------ |
| value | the label in `text-dim`, the value in `text`, wrapped at the value column | `h`/`l` inline, `v`, `y`, `yy` |
| action | the label alone, dim | `d` on the self card; nothing on a contact's card |
| reserved | nothing | nothing: the colour slot, held and not drawn |

**A row exists only when the peer says something.** An empty bio, a missing
username and a birthday their privacy hides are all one case: the row is absent,
not empty. That is why a contact's birthday row is usually missing, and why the
panel must never draw a label with nothing beside it — an empty field and an
absent field would look the same, and only one of them is true.

**The bio wraps and is never truncated with an ellipsis.** A value that does not
fit the panel is the panel's problem, and an ellipsis on a value the reader came
to read is a lie about how much of it there is. The same rule governs the reason
on the signed-out card, which is a chain and is routinely longer than the panel.

**A row's inline position is visible, always.** The selection row is reverse video,
the way a message's is, and the normal caret on it is drawn in the
ground colour so that it can be seen against that reverse. A position that moves
inside a row and cannot be seen is one nobody knows.

### The self card

Rows, in order: `name`; `username`; `phone`; `bio`; `birthday`; then
`add account`, dim, which refuses, and `logout`, which signs out. No `id` and
no `session`: the card is what the reader came to read, and both were the
program's bookkeeping
rather than the person's.

- `username` and `phone` each take their own row, so the phone's own key
  (`phone`) names it, rather than the phone sharing `username`'s row and
  borrowing its label.
- `birthday` is written as the month, day and year, then the age in
  parentheses: `Oct 19, 2001 (24 years old)`. **Without the year when there is
  no year** — `10 December` — because a year is a disclosure the account was
  not required to make, and a birthday with no year has no age to state.
- `add account` refuses with `not yet: this build cannot add an account`.
- `logout` raises a screen-wide confirmation, `Sign out and forget this
  session? (y/n)`, and **confirms before it throws away the only secret the
  program holds** — a panel that only flashed would have taught the reader the
  wrong thing about that key. `y` queues the sign-out; the session is discarded,
  the list empties, and the sign-in field comes back for the phone. The
  confirmation outranks a transient status and both are the same row.

### A contact's card

Rows, in order: `name`; the reserved colour slot; `username`; `bio`; `birthday`,
each only when the peer gives one. **No phone**: a client cannot ask a contact
for their number. No `add account`, no `logout`, no birthday-forcing. The colour
slot sits immediately after `name` and not lower, because it is the name's own
property — the thing that backs the name in the chat list and inks `[them]` in a
conversation — so it belongs beside the row it qualifies, not under the rows that
merely describe the person.

Because a person cannot be replied to, searched for, or deleted *from their own
card* — all three already exist in the conversation, and duplicating them is
worse than not having them — the card's verbs are read. `y` yanks the character
selection; `yy` yanks the row's whole value into the register and then offers it
to the system clipboard by OSC 52, the same two-part rule the conversation's `y`
already follows. `d` and `e` refuse: the rows are not yours.

### The reserved colour row

The program will one day let the reader give each person a colour, shown as a
background behind their name in the chat list and as the indicator ink on
`[them]` inside a conversation, because there are no profile pictures and colour
is the only identity signal available. **The row is reserved, and it is not
designed here.** Its slot is held between the name and the username, it draws
nothing, and nothing sets it. The constraints it will have to satisfy, stated now
because they are constraints and not choices:

1. **A fixed set of named colours, never a free picker.** This is a sixteen-colour
   terminal, and a reader-chosen background behind body text is illegible for
   anything but near-black. A free picker is a contrast-bug generator, and a set
   this program names is the only form the set can be checked in: a picker has no
   floor to test against, and a named colour has one value to hold against it.
2. **A contrast floor against the body text, and the name stays in the body
   text.** Colour only ever reinforces the name, it never carries it. Colour alone
   is not an accessible identity channel, and a card whose name *is* the colour
   fails the moment the terminal cannot render it. That is also why
   `Color::Indexed` was rejected for the palette above: on the terminal this
   program hands a name to, the index is the one form that renders as something
   other than what was chosen, and a colour that carries meaning cannot be the form
   that degrades.
3. **One stated winner between the person's colour and the chat list selection
   row's background, and the winner is the selection row.** The selection row
   already carries a background, reverse video, and reverse video belongs to the
   cursor alone. If the person's colour took that row's background, the reader
   would no longer be able to tell which row the cursor is on. So the colour is
   not drawn on the selection row at all, and it returns the moment the cursor
   moves. The winner is decided here, once, rather than row by row.
4. **It is local state this program owns**, keyed by the peer's identifier and
   persisted in its own file. Telegram has no field for it.
5. **It arrives with a light value as well as a dark one, like every other role.**
   Every colour in the palette table above is specified twice, once per terminal,
   because a value that is right on one is often invisible on the other. A peer
   colour is the role likeliest to be chosen on a dark terminal and then read on a
   light one, so it needs the same pair or it becomes the single thing in the
   interface with no light-terminal answer.

### Entry and exit

Every key, with the reason beside it, judged against what Vim itself does.

| Key | Does | Reason |
| :-- | :--- | :----- |
| `A` | opens the *contact's* card, from the conversation and from the chat list | `A` is unbound everywhere else in this program, and it reads as "about". Vim's `A` is insert-at-end and has nothing to say about a card. |
| `l` | from the conversation, opens the contact's card | the right-hand column holds two contents, and `l` is the one that means "the other one" |
| `S` | opens **your own** profile, from either pane | the key the account panel always had; the panel changed shape, the key did not |
| `h` | goes back — **but `h` is also inline motion** | at a card row's first cell the inline position has nowhere left to move, so `h` takes its other meaning there. Vim leaves a motion at the buffer edge doing nothing; a card is not a buffer, and a reader with no way back has a stuck screen. Everywhere else on the card it is a motion. |
| `Esc` | the way back, and the only one that needs no argument | `<Esc>` leaves a mode in Vim; here it leaves the card |
| `Tab` | walks the panes in the order they are drawn, as it already does | unchanged |
| `Ctrl-w h` / `Ctrl-w l` | pane navigation | because `h`/`l` are inline. Bare `Ctrl-w` keeps its existing meaning, leaving the input line: a prefix key with a bare fallback, the pattern `g`/`gg` already uses in this program. |
| `j` `k` `gg` `G` | between rows, plus `{count}j` | counts cost nothing: the digits are otherwise unbound on a card |
| `h` `l` | **inline**, within the row's text | exactly as a message moves within its text. The value is one logical line, so these are single-cell motions and the wrap is only how it is drawn. |
| `v` | charwise selection, extended by `j`/`k` to become a selection of *rows* | a set, not a range of characters: the same distinction the conversation makes |
| `y` / `yy` | the selection, or the row's whole value | `y` is an operator and `yy` is linewise, which is what Vim means by them. With a selection up, `y` acts at once, as it does in Visual. |
| `d` | acts on the self card's two action rows, and refuses on a contact's | `d` is the conversation's act; here it acts on the row the cursor is on, and the refusal names which rows act |
| `e` | refuses | the editable profile is a separate screen, and the refusal names it: `:settings` |
| `p` | **unbound, and says why in the hint area** | there is no buffer in a card to paste into, and a bound key that cannot work is worse than an absent one. The reason is written to the status row, which is the row the hint is on. |
| `zz` `zt` `zb` | no-ops | a card cannot scroll, and pretending it can is a lie about state. The key is dropped rather than answered with motion that moves nothing. |

Two decisions about the screen's shape, stated beside the `h`-is-both rule because
they are the same kind of thing — what a key means changes when the screen changes
shape under it:

- **`Ctrl-w l` does nothing, and that is a statement about the screen.** A card is
  the last thing drawn in the right column: there is nothing to its right to move
  to. `Ctrl-w l` is therefore dropped rather than answered with motion that moves
  nothing, exactly as `zz`/`zt`/`zb` are on a card that cannot scroll. Bare
  `Ctrl-w` keeps the meaning it has everywhere else in this program — it leaves
  the input line — and it is a prefix with a bare fallback for the reason
  `g`/`gg` is one: `Ctrl-w h` walks the panes, and any other follower makes
  `Ctrl-w` mean what it always meant.
- **`/` on a card hands the search to the conversation.** A search whose scope
  depends on which pane the reader happens to be standing in is a search the
  reader has to stop and think about, and a card is a handful of rows — seven at
  its longest — with nothing in it worth a second scope. So `/` is the
  conversation's key wherever it is pressed: a card borrows the conversation's
  search rather than growing one of its own, and the hits are drawn where the
  search lives.

Two rules that are easy to get backwards:

- **The conversation keeps its message cursor across a round trip; the card starts
  at the top every time.** `l`, then back, then `l` must land the card on its
  first row. One is a document and the reader's place in it matters; the other is
  a card of fixed shape. The scenes `ljjjh` and `ljjjhl` show it: the first lands
  in the conversation at `(6/16)`, exactly where the reader left it, and the
  second lands back on the card's first row.
- **The `Esc` ladder is the same ladder as everywhere else, and the card is not a
  stack.** Nothing being edited: one press closes the card. A line open over it:
  four rungs, one `Esc` each — insert, the line's Normal, the card, closed. The
  card remembers nothing between openings, which is why it is not a stack.

### Transport

Both cards are the same request. `users.getFullUser` takes any `InputUser`: your
own is `UserSelf`, a contact's is `User(id, access_hash)`, and the
`access_hash` is already in the peer cache the chat list fills. The shared rule is
the row rule above — a row exists only when the peer says something — which is why
a contact's birthday row is absent whenever their privacy hides it, and why the
same rule governs an empty bio and a missing username. **The one write is the
escape hatch**, and it forwards straight to the protocol layer.

### The shell cards

Before either card there is the state a reader on a machine with no credentials
sees, and it is the easiest one to get wrong: an empty panel is indistinguishable
from a widget that has gone wrong. So the panel is never empty. `Profile` and
then, on its own line, `not signed in`, then the reason — wrapped, never clipped,
because a bring-up failure carries a chain and a reason cut at the edge is a
reason the reader cannot act on — then a line saying where to set the credentials:
`Set the credentials again with :signin.`

There is a third state, **before anything has been read**, and it must not borrow
the other one's wording: `reading the session…`, then `Nothing has been read yet,
so nothing is known either way.` It does not say the session is missing, because
nothing has established that yet. Its hint names `q` only, and the signed-out
card's hint names `::signin` and `q`: the shell cards are the whole program, so
there is nothing behind them to go back to and `Esc` has nowhere to land.

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
an animation is a second thing to keep in step with the window. Nothing repaints
on a schedule: the program asks the terminal for no blinking cursor, the
scrollbar's thumb and the selection row's reverse video change only when the
reader moves, and the carets are painted cells rather than the terminal's own. A
spinner or a countdown would be the first thing in this programme that repainted
on a schedule rather than on a change, and it is not worth it.

## What this document is for

It is the spec the rest of the design work implements, and it is tracked beside
`AGENTS.md` for the same reason: a design system that lives only inside a tool
drifts from the code with nothing to catch it. When a change makes a section
here wrong, the change is incomplete until this file is updated in the same
commit.
