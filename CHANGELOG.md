# Changelog

All notable changes to televim are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The versioning policy
(when the version is bumped, the tag scheme, what major/minor/patch mean) is in
[`docs/decisions.md`](./docs/decisions.md).

## [Unreleased]

### Added

### Changed

### Fixed

## [0.1.5]

Baseline reconstructed from `git log` and the README feature set. No release
notes or tags existed before this section, so it is best-effort: it says what the
tree does at 0.1.5, not what each commit changed.

### Added

- Authentication: MTProto sign-in with the login code and two-factor password asked
  for in the interface; the session file is sealed with a passphrase.
- Chat list: private user-to-user chats only, with bots, groups and channels filtered
  out; cached on disk so a launch draws without waiting for the network.
- Starting a conversation with someone new: `/` on the chat list searches contacts.
- Conversation view: `j`/`k` by message, `g`/`G`, `Ctrl+d`/`Ctrl+u` scrolling, and
  search within the loaded window that then asks the server.
- Message composition: `i`/`a` to compose, `Enter` to send, `Esc` to leave Insert.
- Send, edit and delete one message with `d`, or a selection of messages.
- Forward: `f` in Normal forwards the message under the cursor, or the current selection.
- Media: download progress with cancel on `Esc`; downloads cached on disk in a
  configurable directory.
- Text direction: right-to-left rows in the composer and conversation, with a
  bidi module.
- Typing indicator and online / last-seen status for the open peer.
- Yank and paste from Visual mode into registers.
- Commands: `:q`/`:quit`, `:chat <id>`, `:new <query>`, `:settings`.
- Profile card for the signed-in account and for a contact.
- Read marker sent when a conversation is opened, and kept read on arrival while open.
- Reconnect: when the update feed stops, the client rebuilds itself.
- History cache: messages are cached per account so an open shows the last window at once.

### Changed

- Workspace is one version, set in the root `Cargo.toml`, inherited by every crate.

### Fixed

- Media store generation guard: a late store no longer removes a newer store's file.
- Command prefix no longer hides the first keystroke.
- Deleted accounts are left out of the forward destinations.

### Known gaps for v1

These are real, named, and tracked in [`docs/known-gaps.md`](./docs/known-gaps.md).
The areas still behind for v1:

- Live-terminal proof is missing for the kitty graphics path and for forwarding; both
  are covered by byte-level or unit tests only.
- Sixel images are not implemented.
- Arabic and Persian letters are not contextually joined; default-mode RTL renders
  wrong in xterm and alacritty.
- The history file and the expanded AES key schedule are not encrypted or zeroized at
  rest in every path.
- The 50 MB memory ceiling is not measured under load.
- Visual mode's `r` refuses rather than replying.
- Groups and channels are out of scope by design; no cache exists for them.
