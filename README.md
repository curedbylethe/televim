<picture>
  <source media="(prefers-color-scheme: dark)" srcset="design/logo/televim.svg">
  <source media="(prefers-color-scheme: light)" srcset="design/logo/televim-light.svg">
  <img src="design/logo/televim-light.svg" alt="televim" width="320">
</picture>

# televim

A keyboard-driven Telegram client for the terminal, with Vim-style Normal, Insert,
and Visual modes. It runs on macOS and Linux, with an experimental Windows
build; it shows private user-to-user chats only, with no groups, channels, or bots.

Pre-1.0: see [known gaps](docs/known-gaps.md).

![televim showing a chat list and a conversation](docs/images/televim.png)

*Fictional offline chats; the status row is cropped.*

## Browser demo

Televim's Pages deployment target is
[curedbylethe.github.io/televim](https://curedbylethe.github.io/televim/). It runs
the real Rust interface with fictional conversations and no Telegram
connection. The page shows the provenance label **Interactive Rust/WASM demo ·
fictional conversations · offline** before load.

Build locally with `make wasm-pages`. See the [browser demo guide](docs/web-demo.md),
[parity matrix](docs/web-demo-parity.md) and [portfolio embed contract](docs/web-demo-embed.md).

## Install

Release binaries are on [GitHub Releases](https://github.com/curedbylethe/televim/releases).

| Platform | Status |
| :--- | :--- |
| macOS, Apple silicon (11.0+) | Supported |
| macOS, Intel (10.12+) | Supported |
| Linux, x86_64 (glibc 2.35+) | Supported |
| Linux, arm64 (glibc 2.35+) | Supported |
| Windows, x86_64 | Experimental |

**macOS and Linux**

```console
curl -fsSL https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.sh | sh
```

**Windows PowerShell**

```powershell
irm https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.ps1 | iex
```

The installer uses `$HOME/.local/bin` on macOS and Linux, or
`%LOCALAPPDATA%\televim\bin` on Windows. It adds the directory to
`PATH`; open a new terminal after installation. Use `--no-modify-path` or
`-NoModifyPath` to skip the `PATH` change.

Manual archives and `SHA256SUMS` are on the [release page](https://github.com/curedbylethe/televim/releases).
The checksum detects a broken download, not a compromised release; see the
[manual install and checksum steps](docs/installing.md).

Upgrade with `:update`; installed updates require restarting televim. See [upgrading](docs/installing.md#upgrading).

On Linux, install DBus (`libdbus-1-3` on Debian or Ubuntu) and a Secret Service provider. See [platform details](docs/installing.md#linux).

## First run

<a id="credentials"></a>

Install, then start Televim:

```console
televim
```

Release binaries include Telegram application credentials, so no API setup is
needed. Sign in with your phone number and Telegram's code; enter your
two-factor password if asked. Type each value in the input bar and press
`Enter`. Both release and source builds require your Telegram login.

A source build has no application credentials. Set `TELEVIM_API_ID` and
`TELEVIM_API_HASH`, or add `api_id` and `api_hash` to `televim.toml`.

By default, Televim reads `televim.toml` under the operating system's local
data directory (`~/Library/Application Support/televim` on macOS,
`%LOCALAPPDATA%\televim` on Windows, or `$XDG_DATA_HOME/televim` on Linux;
usually `~/.local/share/televim`). Use `--config <path>` to select another
file. Logs, drafts, history, and the account list are stored beside it; the
media cache can use a separate directory. On first launch, an existing profile
in the current directory is adopted in place and remembered for later launches.
Stable release checks run in the background once per day. Set `updates = "off"`
or `TELEVIM_UPDATES=off` to disable them; the default is `auto`.

By default the login session is an encrypted file (`session`) beside the
configuration, and Televim creates a 32-byte `0600` key file (`session.key`) next
to it on first use, so the OS keychain is never asked. Set
`TELEVIM_SESSION_PASSPHRASE` or `session_passphrase` to use a passphrase instead,
or set `session_path` alone to keep the file's key in the OS credential store. See [session
storage](docs/product.md) and [the macOS keychain
recovery](docs/installing.md#if-the-keychain-keeps-asking).

Use `televim --chat <id>` to open a chat directly.

- Desktop notifications are on by default (`notifications = "off"` disables
  them). Televim suppresses them for the open or muted chat, outgoing messages,
  stale arrivals, and while a reported terminal has focus.

## Essential controls

**Anywhere**

| Key | Action |
| :--- | :--- |
| `?` / `:` | Search chats / open commands from a conversation or profile card |
| `Esc` / `Ctrl-C` | Cancel or step back / quit immediately |

**Chat list**

| Key | Action |
| :--- | :--- |
| `j` / `k` / `gg` / `G` | Move highlight; jump to first / last chat |
| `l` / `Enter` | Open chat / profile card |
| `/` / `p` | Start chat / pin or unpin the highlighted chat |

**Conversation**

| Key | Action |
| :--- | :--- |
| `j` / `k` | Previous / next message |
| `Ctrl-d` / `Ctrl-u` | Page down / up |
| `gg` / `G` | First unread / newest message |
| `i` / `a` | Write a message |
| `v` / `V` / `y` | Select text or messages; copy with `y` in Visual mode |
| `d` / `r` / `e` / `f` | Delete (confirms) / reply / edit / forward |
| `/` / `n` / `N` | Search this chat / next / previous match |
| `h` / `q` | Return to list / quit (confirms) |

**Composer**

| Key | Action |
| :--- | :--- |
| `Enter` / `Ctrl-J` | Send / insert a new line |
| `Esc` | Return to Normal mode; press again to leave the line |
| Arrow keys | Move the caret |

Type `:` in a conversation or profile card for `:chat`, `:new`, `:search`, `:attach`, `:save`, `:update`, and `:q`; see the full [command list](docs/product.md#feature-set).

## Troubleshooting

- **“televim has no application credentials”**: see [source-build credentials](#credentials).
- **Linux keyring errors**: install your distribution's DBus runtime package
  and a Secret Service provider such as GNOME Keyring or KWallet. For headless
  use, configure `session_path` and a passphrase. See [Linux details](docs/installing.md#linux).
- **macOS blocks launch**: binaries are unsigned. If you trust the download,
  allow it in System Settings → Privacy & Security.
- **Windows warning**: Windows is experimental and the binary is unsigned; SmartScreen may warn.
- **Logs**: `televim.log` is beside the config file. Include it when reporting a
  problem in the [issue tracker](https://github.com/curedbylethe/televim/issues).

## Documentation

| Guide | Covers |
| :--- | :--- |
| [Installing](docs/installing.md) | Install, upgrade, uninstall, and platform details |
| [Product](docs/product.md) | Goals, acceptance criteria, features, and commands |
| [Known gaps](docs/known-gaps.md) | Current limitations |
| [Architecture](docs/architecture.md) | System design |
| [Decisions](docs/decisions.md) | Key project decisions |
| [Testing](docs/testing.md) | Build and verification commands |
| [Browser demo](docs/web-demo.md) | Architecture, build, limits, hosting and checks |
| [Browser parity](docs/web-demo-parity.md) | TUI flow outcomes and evidence |
| [Portfolio embed](docs/web-demo-embed.md) | Lazy iframe contract for the portfolio repo |
| [Dependencies](docs/dependencies.md) | Libraries, versions, licenses and rationale |
| [Design system](DESIGN.md) | Interface design rules |
| [Changelog](CHANGELOG.md) | Release history |

<a id="building"></a>

## Building from source

Run `make build-release` for a release binary or `make run` for a debug build.
A source build needs the credentials listed above. See the [build guide](docs/testing.md).

## License

MIT OR Apache-2.0.
