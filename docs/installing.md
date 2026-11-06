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
file with `session_path` and a non-empty `session_passphrase`; see
[`known-gaps.md`](./known-gaps.md) for the lost-passphrase limit.

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
encrypted `session_path` and `session_passphrase` configuration described above.

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
uses Windows Credential Manager. Windows file permissions do not provide the
Unix owner-only mode guarantee.

## Upgrading

Use `:update` for an in-app upgrade, then choose Restart to use the new version.
The update verifies the release digest and replaces only the executable. Config,
session, drafts, history, media, and keyring entries stay in place. The current
window keeps running its old version until restart.

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
| File key | Credential store service `televim`, account `file-key` (shared) |
| Media fallback | User cache directory under `televim/televim.media-<label>` |

Per-account session files add `.<id>` before the configured file extension.
Environment variables `TELEVIM_MEDIA_CACHE_DIR` and `TELEVIM_SESSION_PATH` can
move media and session files. A user-created `.env` may also contain Telegram
credentials or session settings.

Signing out deletes the `session` credential-store entry, but leaves the shared
`file-key`, log, and accounts index. It is not a complete uninstall. Media cache
files are plaintext, and backups may retain them. Remove backups separately if
you need to delete that data. A lost session passphrase requires manually
removing the encrypted session file; Televim cannot recover it.

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
- **Wrong version or missing asset:** check that the archive version matches
  the release tag and that the target name matches your system.

## Building from source

See [`README.md`'s building section](../README.md#building). Use Rust 1.98.1.
On Linux, install `libdbus-1-dev` and `pkg-config` before building. Run
`make build-release` or `cargo build --release -p televim`. A source build has no
compiled Telegram application credentials; configure your own as described in
[`README.md`'s credentials section](../README.md#credentials).
