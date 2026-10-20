# Fuzzing the session decoders

The two parsers here that consume bytes the client did not write are
`SessionData::from_bytes` (a persisted snapshot) and `sealed::open` (the
encrypted session envelope). This workspace drives both through one libFuzzer
target. It is a local tool, not part of `make ci`.

## Requirements

Nightly and `cargo-fuzz`, neither of which CI installs:

```console
$ rustup toolchain install nightly
$ cargo install cargo-fuzz
```

## Running

From this directory:

```console
$ cargo fuzz build
$ cargo fuzz run session -- -runs=10000
```

The first byte of an input selects the decoder: `0` is `from_bytes`, `1` is
`open`, anything else returns. A panic is the finding; libFuzzer writes the
reproducer to `artifacts/session/` and prints its path.

## Why this is a separate workspace

`fuzz/Cargo.toml` declares its own `[workspace]` and its own
`[workspace.dependencies]`, and `rust-toolchain.toml` pins nightly here rather
than at the root. So the root pin (`1.98.1`), the root lockfile and the root
manifests are untouched, and `make lint`, `make test` and `make boundary` never
see this directory.

## Corpus and artifacts

`corpus/session/` holds a few tiny hand-written seeds and is committed, so a run
starts from something valid: `empty`, `snapshot-empty` (`0` then `{}`),
`snapshot-valid` (`0` then a minimal snapshot) and `envelope-empty` (`1`). The
first byte is the selector above. Inputs libFuzzer adds to this directory are
ignored, as are crash reproducers under `artifacts/`: they are facts about a run
rather than fixtures of the suite.

The deterministic equivalent — every byte of a valid snapshot and envelope
flipped with a few masks — lives in the `telegram-framework` test module, so the
same "malformed input is an error, not a panic" property is proven on stable in
CI, where nightly and `cargo-fuzz` are absent.
