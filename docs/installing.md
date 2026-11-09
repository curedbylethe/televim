# Installing

Install a prebuilt Televim binary on macOS, Linux, or Windows without Rust. To
build from source, see [`README.md`](../README.md#building).

## Supported targets

| OS | Architecture | Target | Archive | Minimum / status |
| :--- | :--- | :--- | :--- | :--- |
| macOS | Apple silicon | `aarch64-apple-darwin` | `.tar.gz` | macOS 11.0 |
| macOS | Intel | `x86_64-apple-darwin` | `.tar.gz` | macOS 10.12 |
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+ |
| Linux | arm64 | `aarch64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+ |
| Windows | x86_64 | `x86_64-pc-windows-msvc` | `.zip` | Windows 10+; experimental |

Windows remains experimental until the manual TUI, Credential Manager, and
file-opener check passes. The release workflow builds and CLI-smoke-tests
Windows; those checks do not cover the TUI, Credential Manager,
or file opener. Windows arm64, Linux musl/Alpine, and 32-bit targets are
unsupported.

Release binaries are unsigned and not notarized. `SHA256SUMS` checks that an
archive matches its published checksum; it does not prove who built the archive.
On macOS an unsigned build has an ad-hoc code identity that changes with every
build, so the keychain treats a rebuild as a new program; see [If the keychain
keeps asking](#if-the-keychain-keeps-asking).

## Release names and assets

Each release tag is `v<version>`. Unix archives are named
`televim-<version>-<target>.tar.gz`; the Windows archive is
`televim-<version>-x86_64-pc-windows-msvc.zip`. The version in the filename has
no leading `v`. One `SHA256SUMS` file beside the archives covers all five
archives.

Use `uname -s` and `uname -m` on macOS or Linux to choose the target. On Windows,
check `$env:PROCESSOR_ARCHITECTURE`; only `AMD64` is supported.

## One-command install

On macOS or Linux, inspect the script first if you want to review what it does.
It downloads the selected archive and checksum over HTTPS, verifies the
checksum, installs to `~/.local/bin`, and adds that directory to the appropriate
shell profile when needed.

```console
$ curl -fsSL https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.sh | sh
```

To pin a version, set `TELEVIM_VERSION` before `sh`. Replace `<version>` with a
published version.

```console
$ curl -fsSL https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.sh | TELEVIM_VERSION='<version>' sh
```

Pass `--no-modify-path` to install without changing a profile. Set
`TELEVIM_INSTALL_DIR` to choose another directory. Add that directory to `PATH`
yourself when you skip profile changes, then restart the shell.

```console
$ curl -fsSL https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.sh | sh -s -- --no-modify-path
```

On Windows, review `install.ps1` before piping it to PowerShell. The script
verifies the archive against `SHA256SUMS`, installs to
`%LOCALAPPDATA%\televim\bin`, and adds that directory to the User `Path`.

```powershell
PS> irm https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.ps1 | iex
```

For a pinned version, set `TELEVIM_VERSION` first. Replace `<version>` with a
published version.

```powershell
PS> $env:TELEVIM_VERSION = '<version>'
PS> irm https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.ps1 | iex
```

Restart the terminal after installation, then verify with `televim --version`.
To review the Windows script and pass installer options, download it and run
it directly:

```powershell
PS> Invoke-WebRequest https://raw.githubusercontent.com/curedbylethe/televim/main/scripts/install.ps1 -OutFile .\install.ps1
PS> Get-Content .\install.ps1
PS> .\install.ps1 -NoModifyPath -InstallDir "$env:LOCALAPPDATA\televim\bin"
```

## Manual install

Download the archive for your target and `SHA256SUMS` from the same [GitHub
release](https://github.com/curedbylethe/televim/releases). Replace `<version>`
with the release version and `<target>` with one of the targets above.

On macOS or Linux, check only the archive you downloaded. `SHA256SUMS` lists all
release archives, so checking the whole file fails when other archives are not
present.

```console
$ VERSION='<version>'
$ TARGET=aarch64-apple-darwin
$ ARCHIVE="televim-$VERSION-$TARGET.tar.gz"
$ RELEASE_URL="https://github.com/curedbylethe/televim/releases/download/v$VERSION"
$ curl -fLO "$RELEASE_URL/$ARCHIVE"
$ curl -fLO "$RELEASE_URL/SHA256SUMS"
$ CHECKSUM_ENTRY=$(awk -v asset="$ARCHIVE" '$2 == asset || $2 == "*" asset' SHA256SUMS)
$ test -n "$CHECKSUM_ENTRY"
$ printf '%s\n' "$CHECKSUM_ENTRY" | shasum -a 256 -c -   # macOS
$ printf '%s\n' "$CHECKSUM_ENTRY" | sha256sum -c -       # Linux
$ tar -xzf "$ARCHIVE"
$ mkdir -p ~/.local/bin
$ install -m 755 televim ~/.local/bin/televim
```

Do not use `--ignore-missing`: it can pass when the selected archive is absent.
To use the executable by name, add this line to your shell profile, then
restart the shell:

```console
$ export PATH="$HOME/.local/bin:$PATH"
```

On Windows, replace `<version>` with the release version. This example checks
only the selected archive before extracting it.

```powershell
PS> $version = '<version>'
PS> $archive = "televim-$version-x86_64-pc-windows-msvc.zip"
PS> $releaseUrl = "https://github.com/curedbylethe/televim/releases/download/v$version"
PS> Invoke-WebRequest "$releaseUrl/$archive" -OutFile $archive
PS> Invoke-WebRequest "$releaseUrl/SHA256SUMS" -OutFile SHA256SUMS
PS> $line = Select-String -Path .\SHA256SUMS -Pattern ("^([0-9a-fA-F]{64})\s+\*?" + [regex]::Escape($archive) + "$")
PS> if (-not $line) { throw 'Checksum entry missing' }
PS> $expected = $line.Matches[0].Groups[1].Value
PS> $actual = (Get-FileHash -Algorithm SHA256 $archive).Hash
PS> if ($actual -ne $expected) { throw 'Checksum mismatch' }
PS> Expand-Archive $archive -DestinationPath .
PS> $installDir = Join-Path $env:LOCALAPPDATA 'televim\bin'
PS> New-Item -ItemType Directory -Force $installDir
PS> Move-Item .\televim.exe (Join-Path $installDir 'televim.exe') -Force
```

`certutil -hashfile <archive> SHA256` can also calculate a Windows checksum.
Add `%LOCALAPPDATA%\televim\bin` to the User `Path`, then restart the terminal.

## First run

```console
$ televim
$ televim --version
```

The release binary includes the maintainer's Telegram application credentials;
you do not need to register an application to sign in. To use your own
credentials, follow [`README.md`'s credentials section](../README.md#credentials).

The default config path is `./televim.toml`. Use `--config <path>` to select
another path. Drafts, history, the accounts index, and log use that path as
their base; media can use `media_cache_dir`. The log is written to a file,
never to the terminal.

By default, Televim stores its session in the OS credential store. On Linux, the
credential store needs a D-Bus session and a Secret Service provider such as
GNOME Keyring or KWallet. For a headless system, configure an encrypted session
file with `session_path` and a non-empty `session_passphrase`, or with
`session_path` and `session_key_file` to avoid the credential store entirely;
see [`known-gaps.md`](./known-gaps.md) for the lost-passphrase limit and the
key-file exposure.

## macOS

Choose the Apple silicon or Intel target from the table. The release binaries
are unsigned and not notarized, so Gatekeeper may block a downloaded binary.
Remove the quarantine attribute from the extracted executable only after you
trust the download:

```console
$ xattr -d com.apple.quarantine ./televim
```

You can also use Finder's Open action and approve the app in System Settings
when macOS offers that option. Do not disable Gatekeeper globally.

### If the keychain keeps asking

By default Televim keeps the session in an encrypted file beside the
configuration, with its random 32-byte key in `session.key` next to it, and does
not call the keychain. The prompts below only apply when you set `session_path`
alone, or when an older version left the session in the keychain: sign in once
and the new file takes over.

The keyring is the macOS login keychain. On the first launch, expect one prompt
per keychain item: the session item, plus the history `file-key` item when a
history file exists and no passphrase or key file supplies the history key — so
one or two. Choose **Always Allow** for each. Later launches of the same binary
show none. An unsigned or ad-hoc build — every release binary, and a source build
without a stable signing identity — changes its code identity when rebuilt, so
macOS treats a rebuild as a new program and repeats the prompts per item. A
signed and notarized build keeps a stable identity and does not repeat them.

While running, the session is written back to the keychain at most once every
30 seconds, plus once on a clean exit, and the history key is read once per
launch. After a rebuild on an unsigned build, expect the first-launch prompts
again; they do not repeat within the run. Set `log_level` to `debug` to see each
keychain call, logged with its service, account and operation.

To keep the session out of the keychain, configure an encrypted file and its key
file together:

```toml
session_path = "/Users/you/.local/share/televim/session"
session_key_file = "/Users/you/.local/share/televim/session.key"
```

The environment variables do the same:

```console
$ export TELEVIM_SESSION_PATH="$HOME/.local/share/televim/session"
$ export TELEVIM_SESSION_KEY_FILE="$HOME/.local/share/televim/session.key"
```

On first use Televim creates the key file with 32 random bytes and mode `0600`,
and never overwrites it. Both settings are needed for zero keychain calls:
`session_key_file` alone only keys the history cache and leaves the session in
the keyring. Do not also set `session_passphrase`; the launch refuses with a
sentence naming both.

The expected dialogs are:

| Build | First launch | Later launch |
| :--- | :--- | :--- |
| Signed and notarized | one prompt per keychain item — the session, plus the history `file-key` when applicable; choose Always Allow for each | no prompts |
| Unsigned or ad-hoc | one prompt per item per build identity — the session, plus the history `file-key` when applicable; choose Always Allow for each | no prompts for the same binary; a rebuild resets access and repeats the prompts per item |

With `session_path` and `session_key_file` there are zero prompts in either row,
on either build: the key file is read directly and the keychain is not called.
Without the key file, the row for the build applies.

If a launch ends at `offline:` naming the key file, restore that file, or
delete the session file and the key file, then relaunch. A missing, short or
unreadable key file does not discard
the session and does not open a passphrase box; the session file is left as it
was. A key file that cannot be read because the profile denies access needs the
same recovery: restore access or restore the file.

An existing keyring-sealed session cannot open under a newly configured key file.
The first enable costs one sign-in. Move the key file and its session files as a
pair; a key file without the files it sealed opens nothing.

## Linux

The Linux binaries require glibc 2.35 or newer and GNU libc. musl and Alpine are
unsupported. They link to DBus for the credential store; install the runtime
package for your distribution:

- Debian or Ubuntu: `libdbus-1-3`
- Fedora: `dbus-libs`
- Arch Linux: `dbus`

Install a Secret Service provider, such as GNOME Keyring or KWallet, and make a
D-Bus session available. The archive preserves the executable bit. If a manual
copy lost it, run `chmod +x ~/.local/bin/televim`.

On a headless machine without a session bus or credential store, use the
encrypted `session_path` and `session_passphrase` configuration described above,
or `session_path` with `session_key_file` to avoid the credential store.

## Windows

Windows x86_64 is experimental. The Windows release workflow builds and
smoke-tests the executable, but the TUI, Credential Manager, and file
opener still need a manual check. The binary is unsigned. SmartScreen may show
“Windows protected your PC”; choose **More info** and **Run anyway** only if you
trust the download. Smart App Control may block unsigned code without a
per-app bypass. Do not turn either feature off globally.

The installer writes the User `Path` without administrator access. For a
manual install, add the install directory to the User `Path` with PowerShell:

```powershell
$installDir = Join-Path $env:LOCALAPPDATA 'televim\bin'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = @($userPath -split ';' | Where-Object { $_ })
$alreadyOnPath = $pathEntries | Where-Object {
    [string]::Equals(
        $_.TrimEnd([char[]]@('\')),
        $installDir.TrimEnd([char[]]@('\')),
        [StringComparison]::OrdinalIgnoreCase
    )
}
if (-not $alreadyOnPath) {
    $newUserPath = $installDir
    if (-not [string]::IsNullOrEmpty($userPath)) {
        $newUserPath = "$userPath;$installDir"
    }
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
}
```

Restart PowerShell or Command Prompt to use the updated `Path`. The session
uses Windows Credential Manager, or an encrypted file when `session_path` is set.
A `session_key_file` relies on the profile ACL: Windows has no `0600` mode, so
the same-owner guarantee is weaker than on Unix.

## Upgrading

Use `:update` for an in-app upgrade, then choose Restart to use the new version.
The update verifies the release digest and replaces only the executable. Config,
session, drafts, history, media, the key file, and keyring entries stay in place.
The current window keeps running its old version until restart.

For package-managed installs, use that manager's upgrade command. Otherwise,
rerun the installer or follow the [manual replacement steps](#manual-install).
Verify manual upgrades with `televim --version`.

Set `TELEVIM_VERSION` before running an installer to pin a version. Keep
existing `session_kdf_memory_kib`, `session_kdf_passes`, and `session_kdf_lanes`
values unchanged: the encrypted session file will not open with a different
key-derivation setting. See [`known-gaps.md`](./known-gaps.md).

## Uninstalling

Remove the executable from the install directory. Remove the marked Televim
block from the shell profile if the installer added one, or remove its
directory from the Windows User `Path`. Uninstalling does not delete app data.

Let `C` be the path passed to `--config`, or `./televim.toml` by default. Televim
does not create or delete the config file itself. Remove only the files you no
longer need:

| Data | Default location |
| :--- | :--- |
| Config | `C` (`televim.toml`) |
| Log | `C` with `.log` extension (`televim.log`); truncated on launch |
| Drafts | `C` with `.drafts.json` extension (`televim.drafts.json`) |
| History | `C` with `.history.json` extension (`televim.history.json`) |
| Media | `media_cache_dir`, or `C` with `.media` extension (`televim.media`) |
| Account drafts/history | `C` with `.<id>.drafts.json` and `.<id>.history.json` extensions |
| Account media | `C` with `.<id>.media` extension, or `media_cache_dir` with `.<id>` appended |
| Accounts index | `C` with `.accounts.json` extension (`televim.accounts.json`) |
| Session | Credential store service `televim`, account `session` or `session-<id>`; otherwise configured `session_path` |
| Key file | `session_key_file` (`TELEVIM_SESSION_KEY_FILE`); created on first use, `0600` and never overwritten |
| File key (keyring) | Credential store service `televim`, account `file-key` (shared), when no passphrase or key file supplies the key |
| Media fallback | User cache directory under `televim/televim.media-<label>` |

Per-account session files add `.<id>` before the configured file extension.
Environment variables `TELEVIM_MEDIA_CACHE_DIR`, `TELEVIM_SESSION_PATH` and
`TELEVIM_SESSION_KEY_FILE` can move media, session and key files. A user-created
`.env` may also contain Telegram credentials or session settings.

Signing out deletes the `session` credential-store entry, but leaves the shared
`file-key`, a configured key file, log, and accounts index. It is not a complete
uninstall. Media cache files are plaintext, and backups may retain them. Remove
backups separately if you need to delete that data. A lost session passphrase
requires manually removing the encrypted session file; Televim cannot recover it.

## Troubleshooting

- **`GLIBC_2.xx not found`:** use a system with glibc 2.35 or newer. musl and
  Alpine are unsupported.
- **`libdbus-1.so.3: cannot open shared object file`:** install your distribution's
  DBus runtime package from the Linux section.
- **`Bad CPU type` or wrong architecture:** download the archive matching your
  OS and architecture.
- **macOS blocks the binary:** confirm you trust the download, then use the
  narrow quarantine steps in the macOS section.
- **SmartScreen or Smart App Control blocks Windows:** the binary is unsigned;
  Smart App Control may not offer a per-app bypass.
- **`televim: command not found`:** add the install directory to `PATH` and
  restart the terminal.
- **Checksum mismatch:** download the archive and `SHA256SUMS` again from the
  same release. Do not continue with a mismatched archive.
- **Keyring or Secret Service error:** start a D-Bus session and a Secret
  Service provider, or configure an encrypted session file.
- **Key file missing, short or unreadable:** restore the file, or delete the
  session file and the key file, then relaunch. The session file is left as it
  was and no passphrase box opens; the `offline:` sentence names the key file.
- **Wrong version or missing asset:** check that the archive version matches
  the release tag and that the target name matches your system.

## Building from source

See [`README.md`'s building section](../README.md#building). Use Rust 1.98.1.
On Linux, install `libdbus-1-dev` and `pkg-config` before building. Run
`make build-release` or `cargo build --release -p televim`. A source build has no
compiled Telegram application credentials; configure your own as described in
[`README.md`'s credentials section](../README.md#credentials).
