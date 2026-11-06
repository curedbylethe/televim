<picture>
  <source media="(prefers-color-scheme: dark)" srcset="design/logo/televim.svg">
  <source media="(prefers-color-scheme: light)" srcset="design/logo/televim-light.svg">
  <img src="design/logo/televim-light.svg" alt="televim" width="320">
</picture>

# televim

A cross-platform Telegram client for the terminal, written in Rust. `televim` is
a portmanteau of "Telegram" and "Vim," signalling its terminal-first,
keyboard-driven identity: the UI is driven by Vim keybindings (Normal and
Insert today; Visual mode is a stub).

It displays **only private user-to-user chats** — no channels, no bots, no
groups. The primary constraint is **memory**: the client must stay under 50 MB
RSS at idle after loading 50+ chats.

## Documentation

| File | What is in it |
| :--- | :------------ |
| [`AGENTS.md`](./AGENTS.md) | The hard constraints for agents and contributors |
| [`docs/installing.md`](./docs/installing.md) | Install, upgrade and remove televim on macOS, Linux and Windows |
| [`docs/product.md`](./docs/product.md) | Goals, acceptance criteria, and the feature set as it ships |
| [`docs/architecture.md`](./docs/architecture.md) | Workspace layout, the dependency rule, per-crate design |
| [`docs/dependencies.md`](./docs/dependencies.md) | Why each dependency is chosen or pinned |
| [`docs/decisions.md`](./docs/decisions.md) | Key Decisions — the ADRs |
| [`docs/testing.md`](./docs/testing.md) | Quality harness, `Makefile`, CI, the test layers |
| [`docs/memory.md`](./docs/memory.md) | The memory budget: measured baseline, declared work, and what the harness does not measure |
| [`docs/known-gaps.md`](./docs/known-gaps.md) | Known gaps and v2 hooks |
| [`DESIGN.md`](./DESIGN.md) | The design system |
| [`design/logo/`](./design/logo/) | The mark: the dark and light lockups, the favicon, and the social-preview card |
| [`CHANGELOG.md`](./CHANGELOG.md) | Release history, generated per release from the commits |

Goals, acceptance criteria, and the feature set as it ships live in [`docs/product.md`](./docs/product.md).

## Installing

Release archives are on [GitHub Releases](https://github.com/curedbylethe/televim/releases).
Installers put `televim` on your user `PATH` and verify the archive checksum.

| OS | Architecture | Target | Archive | Minimum / status |
| :--- | :--- | :--- | :--- | :--- |
| macOS | Apple silicon | `aarch64-apple-darwin` | `.tar.gz` | macOS 11+ |
| macOS | Intel | `x86_64-apple-darwin` | `.tar.gz` | macOS 10.12+ |
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+ |
| Linux | arm64 | `aarch64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+ |
| Windows | x86_64 | `x86_64-pc-windows-msvc` | `.zip` | Windows 10+; experimental |

macOS/Linux:

```console
$ curl -fsSL https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.sh | sh
```

Windows PowerShell:

```powershell
PS> irm https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.ps1 | iex
```

For a pinned release, set `TELEVIM_VERSION` before running the installer. Use
`--no-modify-path` on Unix or `-NoModifyPath` on Windows to manage `PATH`
yourself. See [`docs/installing.md`](./docs/installing.md) for manual downloads,
checksum commands, system requirements, upgrade, uninstall and troubleshooting.
Inspect an installer before piping it to a shell. `SHA256SUMS` checks download
integrity; it does not identify the publisher.

### Credentials

televim signs in as a Telegram application. The release binaries are built with
the maintainer's `api_id` and `api_hash`, so they run with no setup. The
credentials are not secret from someone who looks inside the binary, so you can
use your own instead. Register an application at
[my.telegram.org](https://my.telegram.org) and set either:

```console
$ export TELEVIM_API_ID=1234567
$ export TELEVIM_API_HASH=0123456789abcdef0123456789abcdef
```

or, in a `televim.toml` in the directory you run it from:

```toml
api_id = 1234567
api_hash = "0123456789abcdef0123456789abcdef"
```

Either overrides the built-in pair. Pass another config path with
`televim --config path/to/televim.toml`. The drafts and history files are written
beside the config file. The login session goes to the OS credential store unless
`session_path` is set.

### Upgrading

Run the installer again to replace the binary. Config, drafts, history, media
and keyring entries stay in place. See [`docs/installing.md`](./docs/installing.md#upgrading).

### Uninstalling

Remove the binary and, if desired, the installer's `PATH` entry. Remove app data
separately. See [`docs/installing.md`](./docs/installing.md#uninstalling) before
deleting config, session or media files.

## Building

```console
$ make ci      # fmt-check lint boundary test design-check build-release
$ make build-release
$ make run     # the debug binary
$ make watch   # the same, under cargo-watch
```

See [`docs/installing.md`](./docs/installing.md#building-from-source) to build
and run from source.

The planned shape of what is left here is worked out in `~/.opencode/plan/`.
