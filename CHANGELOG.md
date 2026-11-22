# Changelog

All notable changes to televim are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The versioning policy
(when the version is bumped, the tag scheme, what major/minor/patch mean) is in
[`docs/decisions.md`](./docs/decisions.md).

## [0.17.0] - 2026-11-22

### Added

- **app:** Store messages in a sealed per-record archive
- **app:** Record and fetch only the ranges a conversation lacks
- **app:** Bound the archive without deleting what is stored
- **app:** Resume an open conversation from the store instead of reloading it

### Fixed

- **app:** Keep every stored message when the window saves
- **app:** Refuse to write over an archive that could not be read
- **app:** Re-seal the archive only when every row moved
- **app:** Re-seal the archive key instead of rewriting every row
- **app:** Archive fetched pages, keep deletions and bound the load
- **app:** Keep the archive read, edited and recoverable
- **app:** Leave the gap below a stored range to the wire
- **app:** Let a page read pick the range that holds its rows
- **app:** Refuse a history write failure from another account
- **app:** Keep the queued pages of a failed final write
- **app:** Write pages and the window in one transaction
- **app:** Serve stored pages before honoring the full-archive refusal
- **telegram-framework:** Flush a session file before renaming it into place
- **app:** Keep the wire cursor open after a short stored page
- **app:** Retry and report a history write that fails at shutdown
- **app:** Keep the new passphrase when only the session's directory sync failed
- **app:** Load only the newest recorded stretch of a chat on restart
- **app:** Archive the page a jump downloads
- **app:** Read the archive-full answer again after a write lands
- **app:** Seal only the window rows a save changed
- **app:** Preserve unreadable history archives
- **app:** Reload history after unlocking
- **app:** Count archived text during writes
- **app:** Resume older paging after reconnect
- **app:** Keep archive pages on their own cursor
- **design:** Model the archive full status
- **app:** Preserve window range in older archives
- **app:** Recover interrupted archive creation
- **app:** Update archive status on transitions
- **app:** Archive pages after changing chats
- **app:** Read older archive pages off the loop
- **app:** Distinguish session durability failures
- **app:** Avoid rewriting saved archive updates
- **app:** Preserve adjacent ledgerless records
- **app:** Join older pages, miss failed reads, guard jump answers
- **app:** Sweep legacy media temp files at launch
- **app:** Ask for a stored run's missing tail wherever the reader is
- **app:** Serve the stored forward stretch before asking the wire
- **app:** Hold live arrivals over an unconfirmed stored window
- **app:** Send the read marker a resumed open owes

## [0.16.0] - 2026-11-20

### Added

- **app:** Make automatic read receipts configurable

### Fixed

- **app:** Take a key event as proof of focus after a lost gain
- **app:** Drop the no-op keyboard flag re-push on key focus recovery
- **tui:** Hand a focused Arabic draft to the terminal as stored
- **tui:** Mirror the focused RTL caret and selection onto emitted cells
- **app:** Stop waiting on the macOS platform opener

## [0.15.0] - 2026-11-17

### Added

- **app:** Add a pure-Rust Ogg/Opus player
- **app:** Play voice messages in the app without a viewer
- **tui:** Show voice playback progress on the message row
- **tui:** Give the body split a hidden chat list
- **tui:** Hide and show the chat list with Ctrl-b
- **tui:** Keep the chat list while a sign-in flow runs
- **app:** Remember a hidden chat list in the configuration

### Changed

- **app:** Read only the tail of a voice note to find its length

### Fixed

- **app:** Keep a play download out of the viewer and stop the note on teardown
- **deps:** Report major releases and decouple the classifier test
- **deps:** Report breaking 0.x releases as major

### ci

- Add a dependency update report split by patch, minor and major
- Run the dependency report on the nightly schedule

## [0.14.1] - 2026-11-16

### Fixed

- **tui:** Ask to quit on q from the chat list
- **app:** Withhold the read marker while the terminal is unfocused

## [0.14.0] - 2026-11-14

### Added

- **domain:** Restore persisted send placeholders
- **app:** Persist queued text sends across disconnects
- **tui:** Re-queue failed text sends on retry
- **app:** Add the reconnect command

### Fixed

- **app:** Move legacy keychain session migration off the launch path
- **app:** Name the failing file in the shared write warning
- **web-demo:** Answer :reconnect in the offline browser demo

## [0.13.0] - 2026-11-14

### Added

- **telegram-framework:** Surface contact signup messages
- **tui:** Render service messages as hint rows

### Fixed

- **domain:** Reorder chats on message arrival
- **tui:** Preserve highlight when chats reorder
- **app:** Refresh chats for unknown arrivals
- **app:** Avoid stale chat refresh behavior
- **app:** Preserve chat-list state during refresh
- **domain:** Keep tied chats stable on arrival
- **app:** Reconcile chat-list refresh safely
- **app:** Preserve reads during chat-list refresh
- **proto:** Preserve history paging across service rows
- **tui:** Isolate service hints from message groups

## [0.12.2] - 2026-11-10

### Fixed

- **app:** Stabilize default profile path
- **app:** Migrate session keys to files
- **app:** Migrate legacy keychain sessions
- **app:** Migrate every indexed account session
- **app:** Use active account session for key migration

## [0.12.1] - 2026-11-10

### Fixed

- **tui:** Reverse every row of the cursor message

## [0.12.0] - 2026-11-09

### Added

- **framework:** Count and log every keychain operation
- **app:** Keep the session in a file with its own key by default

### Fixed

- **framework:** Write the session back at most once every 30 seconds
- **app:** Bound the automatic reconnect and share one key provider
- **app:** Save a changed session when the program exits

## [0.11.0] - 2026-11-09

### Added

- **framework:** Add a key-file session key provider
- **framework:** Preload the stored session once per launch
- **app:** Add the session key-file source
- **app:** Share one key provider across a launch
- **app:** Measure conversation open latency

### Fixed

- Correct the key-file recovery guidance
- Make the key-file recovery work offline
- **app:** Keep the open fetch mark until its own page lands
- **tui:** Preserve the open conversation when reselecting it
- **app:** Restore paging after reconnect

## [0.10.0] - 2026-11-09

### Added

- **tui:** Prepare domain and tui for wasm
- **demo-support:** Add deterministic fictional demo world
- **web-demo:** Add detached wasm feasibility crate
- **demo-support:** Add deterministic action simulator
- **web-demo:** Drive the offline demo app
- **web-demo:** Add browser facilities and offline session state
- **web-demo:** Add core parity checks and quit routing
- **demo-support:** Cover secondary parity flows
- **web-demo:** Page offline conversation history
- **web-demo:** Integrate lazy portfolio shell
- **app:** Add desktop notification delivery seam
- Add per-chat notification muting
- Add manual unread mark command
- Add mark-all-read command
- **app:** Notify for fresh arrivals outside the focused chat

### Fixed

- **tui:** Support wasm key modifier assignment
- **web-demo:** Handle mobile IME composition
- **web-demo:** Connect browser lifecycle and local actions
- **web-demo:** Align simulator queues and reset behavior
- **web-demo:** Start the selected WASM library
- **web-demo:** Harden offline boundaries and browser interactions
- **web-demo:** Report offline retries and remove dead upload progress
- **ci:** Create Pages size report directory
- Address notification and unread review findings

### ci

- Publish audited web demo via GitHub Pages

## [0.9.1] - 2026-11-07

### Fixed

- **app:** Resolve the default bidi mode to words on kitty

## [0.9.0] - 2026-11-06

### Added

- **app:** Check stable releases in background
- **tui:** Add stable update offer
- **app:** Stage verified update downloads
- **app:** Replace verified updates and confirm restart

### Fixed

- **release:** Run tag builds after release-please skip
- **release:** Run Linux packaging with Bash
- **tui:** Align update progress with design
- **app:** Keep update offer across launches and tidy failed updates

## [0.8.0] - 2026-11-06

### Added

- **telegram-framework:** Name the account in a stored session
- **app:** Keep an accounts index beside the configuration
- **app:** Tag caches with the slot that owns them
- **tui:** Ask for a second account instead of refusing
- **app:** Add a second account without signing the first one out
- **app:** Keep the reader's place across an account switch
- **tui:** List the known accounts on the self card
- **app:** Switch to a listed account from the self card
- **telegram-framework:** Upload attachments with progress and cancel
- **proto:** Send attachments through the client
- **app:** Orchestrate attachment sends and exports
- **tui:** Attach, open and save media by command
- Preserve draft reply and edit subjects
- **tui:** Confirm before discarding drafts
- **app:** Protect newer draft file versions
- **install:** Add the POSIX installer
- **install:** Add the Windows installer

### Fixed

- **tui:** Tell the network when a sign-in is abandoned at the phone step
- **app:** Keep the picker truthful and the reader's place on a failed switch
- **telegram-framework:** Classify live media from raw messages
- **app:** Settle synthetic upload failures
- **media:** Align attachment paths, progress, and copy behavior
- **tui:** Preserve drafts while opening prompts
- **tui:** Keep placeholder jump marks alive
- **tui:** Keep read marks when discarding drafts
- **tui:** Preserve parked drafts across compose actions
- **release:** Harden release and installer paths
- **release:** Harden legacy tags and PATH updates
- **release:** Enforce platform and installer checks

### ci

- Run the memory budget off ordinary pull requests
- Split the workflow into a fast job and a single-build release job
- Drop --locked from the release build
- Cancel superseded runs and skip the Rust jobs on docs-only changes
- Test the docs-only classifier and stop extra cancellations
- **release:** Build and verify release matrix
- **install:** Exercise installers on three platforms

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
