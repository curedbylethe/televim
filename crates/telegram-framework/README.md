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
│   └── raw.rs        # Client::invoke                              (`live`)
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
of misbehaving against Telegram. If the code never arrives, or is mistyped,
request a fresh one — `request_login_code` is rate limited per phone number, and
a second call within a minute is logged at `warn`.

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
through the `grammers` session adapter, which is what actually has to hold a
real authorisation key. They run everywhere, including CI, and need nothing but
`cargo test`.

The keyring round-trip is `#[ignore]`d, because a headless machine has no
credential store:

```console
$ cargo test -p telegram-framework --all-features -- --ignored
```

The integration tests in `tests/auth_integration.rs` talk to a real datacenter
and are opt-in:

```console
$ TELEVIM_TEST_DC=1 \
  TELEVIM_API_ID=… TELEVIM_API_HASH=… \
  TELEVIM_TEST_PHONE=+15551234567 TELEVIM_TEST_CODE=12345 \
  cargo test -p telegram-framework --all-features --test auth_integration
```

Without `TELEVIM_TEST_DC=1` each test reports that it was skipped and returns,
so CI stays green without credentials. Add `TELEVIM_TEST_PASSWORD` to exercise
the two-factor branch. Note that each login test requests its own code and
Telegram throttles that aggressively.

## Not here yet

This crate stops at the authentication boundary. Fetching chats and messages,
sending and editing, resolving `InputPeer`s, filtering the update stream, and
the `bumpalo`/`jemalloc` memory work all live outside it — as does writing the
session back after a datacenter migration, which currently needs an explicit
`Client::persist_session`.
