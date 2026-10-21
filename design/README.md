# The design project

A drivable model of every screen, and the specimen that documents them. It is the
repo's copy of the OpenDesign project `televim-app`; the two are kept in step by
`scripts/design-sync.sh`, and `make design-check` fails if they are not.

Open `design/index.html` in a browser, pick a screen, and press the keys. Every
frame is exactly 24 rows by 80 **cells**, and the engine says so on every one. A
cell is not a character — an emoji is two cells and a combining mark is none — so
the model measures `cells`, the same count `wrap::columns` makes in the Rust.

## What it is, and what it is not

**It is a design model.** It renders what `DESIGN.md` specifies. It is the
*visual* source of truth for a screen.

**It is not a reference implementation.** `televim-engine.js` is a third answer to
"how tall is this message", beside `crates/tui/src/rows.rs` and
`crates/tui/src/wrap.rs`, and nothing keeps the three in step. When the engine and
the Rust disagree, **the Rust is right** and the engine is the thing to fix.

**A green engine run proves nothing about the binary.** The behavioural source of
truth is the Rust's own `TestBackend` assertions. If you need to know what the
program does, run the program.

The engine earns its place by being *drivable*: eight hand-written frames could
only ever have shown eight states, and the three profile frames had already gone
stale — silently, because nothing measured them. The engine has 20 scenes and 123
variants, and `design-system/build-specimen.js` regenerates the specimen from them
so a frame cannot disagree with the model that produced it.

It can also represent **text direction**, which is the other thing a frame that
was drawn by hand cannot do: `baseDir` answers which way a message reads, and
`visualCells` answers in what order one wrapped row's pieces reach the grid. The
`rtl` scene group is one conversation in Hebrew shown in both modes, reached with
the same keys as every other chat, and its last variant is a right-to-left draft
in the **input bar** as well as in the conversation: the bar is a row like any
other, so it is permuted by those same two functions and its caret is put on the
column it is drawn at rather than the one it was typed in. A design model that
could only show right-to-left text through a private entry point would not be
showing the program, and `check-model.js` puts the bar through `visualCells` and
compares the frame against it cell for cell, so "the bar follows the same rules as
a message" is a check rather than a claim.

It is a model, and the rules it does not implement are named rather than left to
be discovered: it lays out one row at a time, the way `crates/tui/src/bidi.rs`
hands each wrapped row to `ParagraphBidiInfo`, and it does not mirror glyphs
(rule L4) or shape. It does run the BD16 bracket-pair pass and rule N0, per row,
after W7: two adversarial bracket sequences captured from `unicode-bidi` 0.3.18
pin it in `check-model.js`. `DESIGN.md`'s "Text direction" clause is the spec;
`crates/tui/src/bidi.rs` is the implementation, and where this model and that file
disagree, that file is right.

## Layout

The structure mirrors the OpenDesign project exactly, so a file is copied rather
than edited and the sync is a path mapping. `DESIGN.md` is the one exception: it
lives at the repository root, because that is where the whole repo already looks
for it.

| This repo | OpenDesign `televim-app` |
| :-------- | :----------------------- |
| `../DESIGN.md` | `design-system/DESIGN.md` |
| `index.html` | `index.html` |
| `televim-engine.js` | `televim-engine.js` |
| `televim-ui.js` | `televim-ui.js` |
| `televim.css` | `televim.css` |
| `design-system/tokens.css` | `design-system/tokens.css` |
| `design-system/components.html` | `design-system/components.html` |
| `design-system/build-specimen.js` | `design-system/build-specimen.js` |
| `logo/televim.svg` | `logo/televim.svg` |
| `logo/televim-light.svg` | `logo/televim-light.svg` |
| `logo/favicon.svg` | `logo/favicon.svg` |
| `logo/favicon.ico` | `logo/favicon.ico` |
| `logo/televim-social.png` | `logo/televim-social.png` |

`build-specimen.js` resolves the engine as `../televim-engine.js`, which is why the
layout is mirrored rather than flattened. Editing a vendored file here and not
there is the one way to break the sync, and `make design-check` catches it.

`scripts/check-model.js` is the exception that is not mapped: it is
**repository-only**, so it has no project copy and no row in the table above. It is
the invariant-bearing checker — every scene at 24 rows by 80 cells, the `HINT_W`
budget in cells and code points, the RTL permutations — and it runs from here, in
`make design-check`, resolving the engine beside it. The pair map cannot see a file
with no counterpart, and a project copy once existed and was replaced by a design
run with a scenario script that dropped those checks, so `design-sync.sh check`
fails if the project ever grows `scripts/check-model.js` again. If it does, delete
it: the repository copy is the one that runs.

## The logo

`logo/` holds the mark and is hand-drawn SVG, not generated. `televim.svg` is the
`teleVim` lockup for a dark ground (letters are `currentColor`, default `#E8EAED`),
`televim-light.svg` is the same for a light ground, and `favicon.svg` is the icon
alone. All three are plain shapes with no text, fonts or strokes, so they scale
without a font installed. `favicon.ico` carries 16, 32 and 48 px renders of
`favicon.svg` on a transparent square; regenerate it from the SVG if the SVG changes.
The cyan box is `5BC0DE`, the stem `1E1E1E` and the fold `216883` (`1F7394` on the
light ground); none is a `DESIGN.md` role, since the palette there is the TUI's.

`televim-social.png` is the 1280×640 social-preview card: the dark lockup on a
`#0b0d10` ground, 1040 px wide, which is a ground for the card rather than a logo
colour. It is rendered from the SVG rather than drawn, so regenerate it from the
commands below if the SVG changes. The `sed` is not decoration: this host's
ImageMagick renders SVG with only its internal `MSVG` engine and no librsvg
delegate, so it does not resolve `currentColor` and the letters would come out
invisible.

```console
$ tmp=$(mktemp -d)
$ sed 's/fill="currentColor"/fill="#E8EAED"/g' logo/televim.svg > "$tmp/dark.svg"
$ magick -background none -density 300 "$tmp/dark.svg" -resize 1040x -depth 8 "$tmp/mark.png"
$ magick "$tmp/mark.png" -background '#0b0d10' -gravity center -extent 1280x640 \
    -depth 8 -strip logo/televim-social.png
```

## The specimen is half hand-written

`design-system/components.html` is a **document**, not a generated file. Its
headings, ledes, captions and notes are written by hand; the frame *bodies* inside
it are written by the generator. So it is tracked — the prose is the reason — and
`build-specimen.js` exists to keep the frames honest.

Run it after any change to the engine:

```console
$ node design-system/build-specimen.js
```

It is idempotent, so running it and getting no diff means the committed specimen is
already what the engine produces. It `throw`s if a frame it expects is missing, so
a hand-edit that deletes one fails loudly rather than quietly dropping a screen.

## Keeping the two in step

```console
$ make design-check     # diff both ways; fails if they differ
$ make design-pull      # OpenDesign → this repo   (after a design run)
$ make design-push      # this repo → OpenDesign   (after editing here)
```

The common direction is `pull`, because the work happens in OpenDesign: a design
run edits the project there, and `pull` is how that reaches git. `push` is for the
other direction — an edit made here, in the repo, that the design surface should
also show.

**This repository is the versioned record.** OpenDesign keeps its own opaque
version store (`UUID`-named files and a manifest) which is an undo history, not a
reviewable one: no branches, no diff on a screen, no pull request. Anything worth
keeping belongs here.

`make design-check` runs as part of `make ci`, and **skips loudly** when the
OpenDesign project is not on this machine — a green run says it was skipped rather
than passing silently. Set `OPEN_DESIGN_PROJECT` to point at a project somewhere
else.

## Checking the model without a browser

```console
$ node -e "const TV=require('./televim-engine.js');let n=0,b=0;\
TV.SCENES.forEach((s,i)=>s.variants.forEach((v,j)=>{const g=TV.render(TV.scene(i,j));\
n++;const w=[...new Set(g.map(r=>r.reduce((m,c)=>m+TV.cells(c[0]),0)))];\
if(g.length!==24||w.length!==1||w[0]!==80)b++}));\
console.log((n-b)+'/'+n+' scenes are 24 rows x 80 cells')"
123/123 scenes are 24 rows x 80 cells
```
