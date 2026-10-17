# Changelog

All notable changes to televim are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The versioning policy
(when the version is bumped, the tag scheme, what major/minor/patch mean) is in
[`docs/decisions.md`](./docs/decisions.md).

## [0.6.0] - 2026-10-17

### Added

- **config:** Per-kind viewer commands
- **media:** Carry the media kind to the viewer
- **runtime:** Wait on a configured viewer command

### Fixed

- **runtime:** Wait for the viewer on Windows by default
- **media:** A late commit leaves a viewer's file alone
- **media:** Sweep a download temp only when it is ours or dead

## [0.5.1] - 2026-10-16

### Changed

- **app:** Bound the history cache by JSON-encoded text bytes

## [0.4.0] - 2026-10-16

### Added

- **app:** Persist peer read positions across restarts
- **app:** Recover missed peer read positions from the chat list
- **tui:** Take an explicit UTC offset in date rendering
- **app:** Show message times and day breaks in local time
- **tui:** Let the cursor rest on the open-draft row
- **app:** Fall back to the platform cache dir for media
- **app:** Persist the media id in the history file
- **app:** Prove cross-chat media hit survives restart
- **app:** Verify a media cache entry's checksum on lookup
- **telegram-framework:** Stream a download instead of buffering it
- **proto:** Forward the streaming download
- **app:** Write a media download to a temp file as it streams
- **telegram-framework:** Read a document's embedded thumbnail without a request
- **proto:** Carry the embedded thumbnail to the app
- **app:** Cache a message's thumbnail and invalidate it with its parent
- **dialogs:** Keep the server's pin order for pinned chats

### Fixed

- **tui:** Keep the last character in place when o opens a line
- **app:** Let a failed open's retry hold only its own conversation
- **tui:** Keep a peer's read position with the time it last moved
- **app:** Keep read-mark recency across restarts
- **domain:** Add a prefix reset to VimState
- **tui:** Clear pending input latches in one App method
- **app:** Clear pending input and re-push key flags on focus change
- **proto:** Keep global search hits in Telegram's newest-first order
- **domain:** Keep the newest global search matches under the cap
- **app:** Treat global search answers as newest first
- **telegram-framework:** Treat a zero global search date as no time
- **app:** Run the viewer wait off the loop thread
- **app:** Gate the key reader across the viewer
- **app:** Report the opener's exit when the opener returns early
- **domain:** Treat a ZWNJ as a word boundary
- **tui:** Break a row after a zero-width non-joiner
- **telegram-framework:** Classify an unreadable session file apart from a wrong key
- **app:** Word an unreadable session file for its permissions, not its key
- **app:** Leave input focus alone when ready lands on an open conversation
- **tui:** Draw Terminal-mode lam-alef words in full
- **tui:** Draw a right-to-left profile card value in visual order
- **tui:** Draw global search snippets in conversation RTL order
- **tui:** Draw chat list titles right-to-left under bidi visual
- **tui:** Draw forward picker titles and previews right-to-left
- **tui:** Draw user list names right-to-left under bidi visual
- **tui:** Draw Terminal rows as stored, not split per cluster
- **tui:** RTL caret placement and words bidi mode
- **tui:** Anchor the emoji popup on the drawn caret cell in every bidi mode
- **tui:** Place the Terminal caret word by word, as kitty reverses runs
- **tui:** Keep the last letter of a word with a lam-alef in a message row

## [0.3.0] - 2026-10-09

### Added

- **app:** Name cached media by the hash of its bytes
- **app:** Refresh a cached file's recency on hit
- **app:** Drop legacy message-named media files on open
- **telegram-framework:** Carry the media's stable id on messages
- **domain:** Add the media id to messages
- **proto:** Pass the media id from the framework to the domain
- **app:** Cache media under its Telegram id as well as its message
- **app:** Config key media_cache_max_bytes, overridable by env
- **app:** Drop a cached message's media on edit and delete
- **domain:** Add grouped global search state
- **telegram-framework:** Search all chats with users_only global search
- **proto:** Map global search answers to snippet DTOs
- **app:** Run cross-chat search as one action and land grouped results
- **tui:** Add :search command and ? binding for cross-chat search
- **telegram-framework:** Carry send time and direction on search hits
- **tui:** Show cross-chat results grouped by chat, open the chat on Enter
- **telegram-framework:** Carry the media kind on global search hits
- **telegram-framework:** Seed the peer cache from global search answers
- **app:** Open a cached global search hit without a fetch
- **telegram-framework:** List the top correspondents by server rating
- **proto:** Expose the top correspondents as user candidates
- **app:** List top correspondents on an empty lookup
- **tui:** Look up the top people on an empty new-chat submit
- **tui:** Pin the top people on an empty new-chat query
- **tui:** Format the empty new-chat submit test

## [0.2.0] - 2026-10-09

### Added

- **app:** Sign in with credentials compiled into the release build

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
- The expanded AES key schedule is not zeroized in every path.
- The 50 MB memory ceiling is not measured under load.
- Visual mode's `r` refuses rather than replying.
- Groups and channels are out of scope by design; no cache exists for them.
