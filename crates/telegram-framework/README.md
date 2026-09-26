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
- **`bumpalo`/`jemalloc` tuning lives at the composition root.** This crate
  allocates like any other; the arena and allocator choices belong to the binary.

Two things are known gaps rather than deliberate scope cuts:

- **The update stream is drained and counted until it is taken, not buffered.**
  `ClientBuilder` captures the pool's update receiver and a task discards what
  arrives until `subscribe_updates` hands the feed to a caller, warning once so
  the discard is never silent. The channel therefore cannot grow without bound,
  and nothing is lost either: the session's update position only advances while a
  feed is running, so `catch_up` replays from wherever it stopped. What the feed
  does not model — reactions, pins, typing indicators, read receipts — is
  discarded by the same filter.
- **`check_password` takes the password as `&str`.** Its bytes stay in memory for
  as long as the caller's buffer does. Zeroising our own copy would not help —
  `grammers` holds the value across the SRP exchange — so this needs a decision
  about who owns the buffer, not a one-line `zeroize`.
