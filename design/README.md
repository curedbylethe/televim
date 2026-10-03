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
stale — silently, because nothing measured them. The engine has 63 scenes, and
`design-system/build-specimen.js` regenerates the specimen from them so a frame
cannot disagree with the model that produced it.

It can also represent **text direction**, which is the other thing a frame that
was drawn by hand cannot do: `baseDir` answers which way a message reads, and
`visualCells` answers in what order one wrapped row's pieces reach the grid. The
`rtl` scene group is one conversation in Hebrew shown in both modes, reached with
the same keys as every other chat — a design model that could only show
right-to-left text through a private entry point would not be showing the program.

It is a model, and the rules it does not implement are named rather than left to
be discovered: it lays out one row at a time, and it does not run the algorithm's
BD16 bracket-pair pass, which matches brackets across a whole paragraph. That is
5 of 4000 generated cases, all adversarial bracket sequences rather than message
text, and `check-model.js` carries the two as a comment saying so.
`DESIGN.md`'s "Text direction" clause is the spec; `crates/tui/src/bidi.rs` is the
implementation, and where this model and that file disagree, that file is right.

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

`build-specimen.js` resolves the engine as `../televim-engine.js`, which is why the
layout is mirrored rather than flattened. Editing a vendored file here and not
there is the one way to break the sync, and `make design-check` catches it.

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
63/63 scenes are 24 rows x 80 cells
```
