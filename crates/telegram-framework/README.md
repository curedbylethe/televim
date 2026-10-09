# telegram-framework

The first-party wrapper over [`grammers`][grammers], and the **only** crate in
this workspace allowed to depend on `grammers-*`. It owns three things:

- **Session persistence** — a serialisable snapshot of the account's
  authorisation state, plus pluggable backends to store it.
- **The login flow** — phone number, login code, and two-factor password.
- **A raw escape hatch** — invoke any MTProto method without waiting for a
  hand-written wrapper.

Everything `grammers` hands back is translated into a type defined here before
it leaves the crate, so `proto`, `domain` and `tui` never name a `grammers`
type.

[grammers]: https://codeberg.org/Lonami/grammers

## The `live` feature

The `grammers` dependency is optional and **off by default**:

```console
$ cargo tree -p proto  | grep grammers   # no output
$ cargo tree -p domain | grep grammers   # no output
```

That is not a stylistic choice — it turns the architectural rule "only
`telegram-framework` touches `grammers`" into something a machine checks. The
session-storage half of the crate has no `grammers` dependency at all and is
always compiled; the client half needs `--features live`:

```console
$ cargo build --features live
$ cargo test  --all-features
```

CI builds with `--all-features`, so the client is compiled, linted and tested on
every change.

## Layout

```
crates/telegram-framework/
├── src/
│   ├── lib.rs        # Public surface, and the reasoning behind `live`
│   ├── error.rs      # FrameworkError, AuthError, SessionError, RequestError
│   ├── session.rs    # SessionData, SessionStore, and the three backends
│   ├── client.rs     # ClientBuilder, Client, and the login flow  (`live`)
│   ├── auth.rs       # LoginToken, PasswordToken, SignInResult     (`live`)
│   ├── raw.rs        # Client::invoke                              (`live`)
│   ├── search.rs     # Client::search_messages                     (`live`)
│   ├── media.rs      # MediaKind, and Client::download_media        (`live`)
│   └── testing.rs    # Fixtures shared by the unit tests   (`test` + `live`)
└── tests/
    └── auth_integration.rs   # Opt-in tests against a real datacenter
```

Data flows in one direction. `Client` owns a `grammers` client, which owns a
`StoreSession` — an adapter that implements `grammers`' `Session` trait on top of
a `SessionStore`. Nothing flows back out except this crate's own types.

```text
   ClientBuilder ──▶ Client ──▶ grammers client ──▶ StoreSession ──▶ SessionStore
                        ▲                                                 │
                        └────────────── SessionData ◀────────────────────┘
```

## The `grammers` version, and what it constrains

Pinned to `0.10.0` from crates.io. Upstream stopped tagging after `v0.8.0`, so
no `tag =` can name 0.8.1, 0.9.0 or 0.10.0; see the dependency notes in
`AGENTS.md` for the evidence. `grammers-tl-types` generates from **TL layer 227**.

Three things about that release shape this crate, and are worth knowing before
changing any of it:

- **`PeerMap` has no public constructor.** It can only be obtained from a
  `grammers` response. `fetch_history` builds its own `GetHistory` request,
  because the typed iterator cannot scroll newer, so there is no response to
  take one from — which is why `history.rs` reads the raw message rather than
  going through `grammers`' `Message`. Every field it reads is the same read
  `grammers` makes off the same value.
- **A `Session` method can fail, and the setters are `async`.** Every method this
  crate implements touches the in-memory mirror, so `Session::Error` is
  `Infallible` and saying so is the point: the credential store is written
  separately, precisely so that a keyring write never lands in the request path.
- **The update position is not written on drop.** Asking for it is `async` and a
  destructor cannot await, so `UpdateSubscription::finish` does it and the
  update pump calls it. See **Not here yet** below.

`Client::subscribe_updates` and `Client::send_message` are `async` where they
were not before, for the same reason: `grammers` made the first one `async`, and
`send_message`'s future is large enough that boxing it keeps the stack small.

## Logging in

```text
   phone ──▶ request_login_code ──▶ LoginToken
                                      │
                                      ▼
                                    sign_in ◀── code
                                      │
                     ┌────────────────┴────────────────┐
                     ▼                                 ▼
          SignInResult::Success         SignInResult::PasswordRequired(PasswordToken)
          (session persisted)                        │
                                                     ▼
                                      check_password ◀── password
                                                     │
                                                     ▼
                                          session persisted
```

```rust
use telegram_framework::session::KeyringStore;
use telegram_framework::{ClientBuilder, SignInResult};

let client = ClientBuilder::new(api_id, api_hash)
    .session_store(Box::new(KeyringStore::default()))
    .build()
    .await?;

if !client.is_authorized().await? {
    let token = client.request_login_code("+15551234567").await?;
    let code = read_the_code_telegram_sent();

    match client.sign_in(&token, &code).await? {
        SignInResult::Success => {}
        SignInResult::PasswordRequired(password_token) => {
            let password = ask_the_user(password_token.hint());
            client.check_password(password_token, &password).await?;
        }
    }
}
```

`LoginToken` is **single use**. The flag is set the first time the token reaches
`sign_in`, so a second attempt fails with `AuthError::TokenAlreadyUsed` instead
of misbehaving against Telegram. That check runs before any request, which is
what the integration test pins down with a deliberately wrong code. The token is
also deliberately not `Clone` — a copy would be a second handle to a hash
Telegram only redeems once — and a compile-time assertion keeps it that way. If
the code never arrives, or is mistyped, request a fresh one — `request_login_code`
is rate limited per phone number, and a second call within a minute is logged at
`warn`.

## Session storage

`SessionStore` is a three-method trait — `load`, `save`, `clear` — over
`SessionData`: a plain snapshot of the home datacenter, the known datacenters
and their authorisation keys, the cached peers, and the update counters. It
contains no `grammers` type, so a backend needs to know nothing about MTProto.

| Backend        | Where the session lives                          | Use it for                          |
| :------------- | :----------------------------------------------- | :---------------------------------- |
| `KeyringStore` | macOS Keychain, Windows Credential Manager, Linux Secret Service | **Production.** The default. |
| `FileStore`    | A JSON file                                      | Machines with no keyring: headless Linux, containers, CI. |
| `MemoryStore`  | Process memory                                   | Tests, and sessions that must not outlive the process. |

```rust
use telegram_framework::session::{FileStore, SessionStore};

let store = FileStore::new("/var/lib/televim/session.json");

// Load, inspect, and put it back.
if let Some(session) = store.load()? {
    println!("home datacenter: {:?}", session.home_dc_id);
    store.save(&session)?;
}

store.clear()?;
```

### Writing the session back

The login methods write the session on success. Between logins, a few requests
change it too — a datacenter migration, a peer learned from a response, a moved
update counter — and those are what `StoreSession` tracks with a dirty flag:

- `set_home_dc_id`, `set_dc_option`, `cache_peer` and `set_update_state` raise
  the flag.
- `Client::invoke` and `Client::is_authorized` flush it once the request that
  raised it has finished, logging a failed write at `warn` rather than reporting
  it as a failed request — the request itself succeeded.
- `Client::persist_session` forces a write when a caller knows something changed.

Flushing on a flag rather than on every mutation is deliberate: `grammers`
consults the session on every request, and mirroring each of those to the OS
credential store would put a keyring write in the request path. What this
prevents is a mystery logout — a session that migrated datacenters but was never
written back has no authorisation key for the datacenter it moved to.

### Security

- **`KeyringStore` is the default, and the one to use.** The authorisation key
  is a permanent credential: whoever holds it can act as the account until the
  session is revoked from another client. The OS credential store protects it
  with the platform's own access control.
- **`FileStore` writes that key in plaintext.** It is a fallback, not a
  default. On Unix the file is created `0600`, which keeps other users out but
  does not protect against a compromised account or a backup that sweeps up the
  home directory.
- **No secret is ever logged.** Auth steps log at `debug` with no phone number,
  code or password; `AuthKey` and the token types have `Debug` implementations
  that print a placeholder instead of their contents; the client's `Debug`
  prints no API hash.
- **A failed save is never silent.** If a login succeeds but the session cannot
  be written, the client logs at `warn` — the login stands, and the user finds
  out why they will have to sign in again rather than being logged out for no
  apparent reason.
- **`FileStore` writes through a temp and renames.** The bytes go to a sibling
  `<file_name>.<pid>.tmp` in the same directory, restricted to `0600` before a
  byte is in it, and the temp is renamed over the target. A kill, a full disk or
  a closed terminal mid-write therefore leaves the previous session intact
  rather than a truncated or 0-byte one, and the target only ever appears as a
  whole file. The temp has to be a sibling because a cross-filesystem rename is
  not atomic, and renaming is also what keeps the `0600`: the target inherits
  the temp's inode. A failed write removes the temp. `std` only — no dependency.
- **The store reports corruption; it does not reset it.** `load` returns
  `SessionError::Corrupt` for bytes it cannot parse and leaves the file alone —
  `a_truncated_snapshot_is_reported_rather_than_reset` is the test, because a
  store that silently dropped what it could not parse would be a store claiming
  a session is gone when it is not. Recovery is the caller's decision: `app`
  resolves the store once at bring-up, discards a corrupt session, and tells the
  reader on the status line.

## The raw escape hatch

`Client::invoke` forwards any request straight to Telegram:

```rust
use telegram_framework::session::MemoryStore;
use telegram_framework::{ClientBuilder, tl};

let client = ClientBuilder::new(api_id, api_hash)
    .session_store(Box::new(MemoryStore::new()))
    .build()
    .await?;

let pong = client.invoke(&tl::functions::Ping { ping_id: 42 }).await?;
println!("{pong:?}");

let state = client.invoke(&tl::functions::updates::GetState {}).await?;
```

The response type comes from the request's `RemoteCall::Return`, so it is typed
without the caller naming it. `tl` is re-exported from the `grammers` build this
crate was compiled against, which means callers can build requests without
adding `grammers` to their own manifest and without risking a version mismatch.

The escape hatch is deliberately generic: there is no hand-written wrapper per
method, because a hand-written wrapper per method is the surface this avoids.

## Testing

Unit tests cover the three backends, the snapshot encoding, the error `Display`
implementations, the hex codec, and — most importantly — a full round trip
through the `grammers` session adapter, which is what actually has to hold a real
authorisation key. They also cover the parts that would otherwise only ever run
against a live datacenter: the single-use compare-and-swap on `LoginToken`
(including under concurrent claims), the cooldown decision behind the login-code
warning, and the whole `grammers` error mapping. They run everywhere, including
CI, and need nothing but `cargo test`. The fixtures that stand in for what
`grammers` would have built live in `src/testing.rs`.

The architectural rule this crate exists to enforce is asserted, not assumed:

```console
$ make boundary
```

It fails if `domain`, `proto`, `tui` or a default-featured `telegram-framework`
picks up a `grammers` crate, and CI runs it on every change.

Two checks need hardware CI does not have, so they live in the manual
`Desktop checks` workflow. To run them by hand:

```console
$ cargo test -p telegram-framework --all-features -- --ignored

$ TELEVIM_TEST_DC=1 \
  TELEVIM_API_ID=… TELEVIM_API_HASH=… \
  TELEVIM_TEST_PHONE=+15551234567 TELEVIM_TEST_CODE=12345 \
  cargo test -p telegram-framework --all-features --test auth_integration
```

The first is the keyring round trip, `#[ignore]`d because a headless machine has
no credential store. The second is the opt-in datacenter run: without
`TELEVIM_TEST_DC=1` each test reports that it was skipped and returns, so CI
stays green without credentials. Add `TELEVIM_TEST_PASSWORD` to exercise the
two-factor branch. Note that each login test requests its own code and Telegram
throttles that aggressively.

## Sending, editing and deleting

`Client::send_message(peer_id, text, reply_to)` sends text, optionally as a reply
to another message of the same conversation, and returns the message as the
server now has it — the identifier is the one Telegram assigned, not a local
placeholder. `Client::edit_message(peer_id, message_id, text)` replaces the text
of a message the account wrote; Telegram enforces the direction, so an attempt to
edit another account's message is refused by the server rather than by a check
here. It returns nothing: `grammers` discards the `Updates` an edit answers with,
so the new text reaches a caller as a `UpdateKind::MessageEdited` event and
nowhere else. `Client::delete_messages(peer_id, ids)` deletes for both sides.

All three take the peer's *bare* identifier, resolve it through the session's
peer cache, and report `FrameworkError::UnknownPeer` when it is not there — the
same rule `fetch_history` follows.

Text is checked before the request, so no round trip is spent on a message
Telegram would reject. `validate_text` is that check, and its two refusals are
the whole of the contract:

- `FrameworkError::TextEmpty` — the text is empty or nothing but whitespace.
- `FrameworkError::TextTooLong { chars, limit }` — the text is longer than
  `TEXT_LIMIT` characters. The count is in characters, not bytes, because that is
  the unit Telegram limits on: a message of emoji is measured the same way the
  server measures it.

Two things are deliberately absent rather than half-present:

- **Deleting for this side alone.** `grammers` hard-codes `revoke: true`, and the
  other scope would need a raw request together with a way to ask the reader
  which they meant. Shipping the capability under the one confirmation every
  delete already uses would make each delete silently "for both".
- **Cancelling a send that is already on its way.** That needs task-abort
  machinery this crate does not have.

## Marking read

`Client::mark_read(peer_id, max_id)` asks Telegram to mark a conversation read up
to and including `max_id`, and returns once the server has accepted it. The peer
is resolved through the session's peer cache like the calls above, and reports
`FrameworkError::UnknownPeer` when it is not there. It does not clear anything on
its own: the caller decides what an accepted marker means for the screen. The
application sends it when a conversation is opened and clears the unread count
only on success, so a refused marker is not a state the screen has to undo.

## Searching

`Client::search_messages(peer_id, SearchArgs)` searches one conversation and
returns **places, not messages**: a `SearchResults` holding the matching message
identifiers and how many matches there are in all. That shape is the point —
a match is only ever used to be gone to, and the page is fetched there by
`fetch_history` with `HistoryArgs::around`. Building a message for every match
would allocate and immediately drop every one of their bodies, which is the copy
a search does not need.

```rust
use telegram_framework::{HistoryArgs, SearchArgs};

let found = client
    .search_messages(dialog.peer_id, SearchArgs::first("televim", 100))
    .await?;

println!("{} of {} match(es)", found.ids.len(), found.total);

// Go to one: a page centred on it, oldest first.
if let Some(&id) = found.ids.first() {
    let page = client
        .fetch_history(dialog.peer_id, HistoryArgs::around(id, 50))
        .await?;
}
```

`SearchArgs::first` takes the newest page; `SearchArgs::after` anchors a later
one at a match, for callers that page through results. The page size is clamped
to `SEARCH_LIMIT`, which is the same wire bound as `HISTORY_LIMIT`.

Two facts about the answer are load-bearing:

- **It is newest first**, the same as a history page — Telegram's order, left as
  it arrives. A caller that walks a match list oldest first reverses it once.
- **`total` is a number for every response variant.** A sliced answer carries
  `count`, the total number of matches; an unsliced one, returned when every
  match fits one page, has no `count` field at all, so its total is the number
  of messages it holds. Reading `count` unconditionally would report zero on
  exactly the small conversations a search is most used in.

This is the same reasoning as history's, and it is why the crate invokes
`messages.search` itself: `grammers`' own `search_messages` builder cannot set
`add_offset`, `min_id` or `max_id`, and it buffers whole messages.

## Media

Every message description carries **what** the message holds, if anything:
`MessageInfo.media` is `Option<telegram_framework::MediaKind>` — `Photo`, `Video`,
`Gif`, `Voice`, `Sticker`, or `File`. `None` means Telegram said there is no media at all;
`Sticker` is a static sticker — an animated one stays `File`, and the raw path
answers a sticker that moves as a `Gif` by attribute order — and `File` is the
catch-all, so a contact, a poll, and any kind a newer
Telegram invents all arrive as *something this message carries* rather than as
nothing. Two classifiers produce it — `classify_raw` for the hand-built
`GetHistory` path and `classify_typed` for the `grammers` update feed — and they
are tested on every job because the decision can be, unlike the paths that reach
it.

The kind names a thing; it does not locate one. `Client::download_media(peer_id,
message_id)` is the locator: it re-fetches the message by identifier through the
same raw `GetHistory` request `fetch_history` builds, and streams the bytes out
through `iter_download`. `grammers`' own `download_media` is behind an `fs`
feature this crate never enables, and it takes a filesystem path, which is not a
shape a terminal client wants.

```rust
match client.download_media(peer_id, message_id).await {
    Ok(bytes) => { /* the attachment, whole */ }
    Err(FrameworkError::MediaUnavailable { .. }) => { /* no media, or not this message */ }
    Err(FrameworkError::MediaTooLarge { size, limit, .. }) => { /* size over MEDIA_LIMIT */ }
}
```

`MEDIA_LIMIT` is 16 MiB, checked against the size Telegram declares and again
against what actually arrives, so an oversized attachment is **reported**
(`MediaTooLarge`) rather than truncated or streamed to the end first. The bytes
come back owned: this is a `Vec<u8>` for now, and the streaming-and-cache shape
that a viewer needs is CUR-9/CUR-10's.

## Not here yet

Three things stay outside the crate on purpose, and are worth knowing before
reaching for it:

- **The chat list is unfiltered.** `fetch_dialogs` returns groups, channels and
  bots alongside people. Classifying them is the framework's job — Telegram is
  the only source of the answer — and deciding what to display is the caller's;
  `proto` makes that decision through the domain's own rule.
- **Paging is expressed, not performed.** `fetch_history` sends the page it is
  given and hands it back. Where the loaded part of a conversation ends, and
  therefore what the next request should ask for, is the caller's to keep.
- **Arena and allocator tuning live at the composition root.** This crate
  allocates like any other, on the system allocator; an arena, or a
  `#[global_allocator]`, would belong to the binary, because a crate that chose
  one would choose it for every crate that linked it.

Three things are known gaps rather than deliberate scope cuts:

- **A feed that is dropped without `finish` persists a stale update position.**
  `grammers` no longer records the position when the stream is dropped — asking
  for it is `async`, and a destructor cannot await — so
  `UpdateSubscription::finish` does it and the update pump calls it once the
  feed has been read to its end. A feed torn down without that call, which is
  what a shutdown mid-run looks like, persists a position behind the one it
  reached, and the next launch resolves the gap by replaying updates the reader
  has already seen. Recording it periodically instead would bound that to a
  fixed window; it is not done because nothing has been hurt by it yet.
- **The update stream is drained and counted until it is taken, not buffered.**
  `ClientBuilder` captures the pool's update receiver and a task discards what
  arrives until `subscribe_updates` hands the feed to a caller, warning once so
  the discard is never silent. The channel therefore cannot grow without bound,
  and nothing is lost either: the session's update position only advances while a
  feed is running, so `catch_up` replays from wherever it stopped. What the feed
  does not model — reactions, pins — is discarded by the same filter, and what it
  does read out of that bucket it reads in one place: see the two entries below.
- **A peer's typing is read out of the raw bucket, and is momentary.**
  `grammers` models no named update for `updateUserTyping` either, so it arrives
  in `Update::Raw` and is matched alongside the read acknowledgement. A composing
  action becomes `UpdateKind::PeerTyping { typing: true }` and a cancelled one
  `typing: false`, with the `user_id` standing for the conversation. Nothing in
  this crate expires the signal: the peer may have stopped, or typed nothing
  else, and only the update that says so clears it. A group and a channel report
  typing through their own updates, which are not read, and an action that is
  neither composing nor cancelled is dropped.
- **Read receipts are read out of the raw bucket, and are best-effort.**
  `grammers` models no named update for `updateReadHistoryOutbox`, so it arrives
  in `Update::Raw` and is matched there: a per-conversation watermark saying every
  outgoing message up to `max_id` has been read, which is what
  `UpdateKind::ReadReceipt` carries. Its `pts`/`pts_count` are dropped — that is
  gap tracking this crate does not do, the update position already being in the
  session. The receipt is best-effort by nature and the client never claims more
  than it was told: Telegram sends each update to one randomly chosen active
  session, `catch_up` is off by default, and the update queue can drop one under
  load. A watermark therefore only ever moves forward, and a message whose read
  acknowledgement never arrived is shown as not read rather than as lost.
- **`check_password` takes the password as `&str`.** Its bytes stay in memory for
  as long as the caller's buffer does. Zeroising our own copy would not help —
  `grammers` holds the value across the SRP exchange — so this needs a decision
  about who owns the buffer, not a one-line `zeroize`.
