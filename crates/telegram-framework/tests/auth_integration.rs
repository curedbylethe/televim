//! Integration tests against a real Telegram datacenter.
//!
//! They talk to Telegram for real, so they are opt-in: set `TELEVIM_TEST_DC=1`
//! and the credentials below, and they will run. Without it every test reports
//! that it was skipped and returns, which keeps CI — where there is no account
//! to log into — green.
//!
//! | Variable               | Required | Meaning                                              |
//! | :--------------------- | :------- | :--------------------------------------------------- |
//! | `TELEVIM_TEST_DC`      | yes      | Opt in. Must be `1`.                                 |
//! | `TELEVIM_API_ID`       | yes      | Application identifier from <https://my.telegram.org>. |
//! | `TELEVIM_API_HASH`     | yes      | Application hash matching `TELEVIM_API_ID`.          |
//! | `TELEVIM_TEST_PHONE`   | login    | Phone number of the test account, in `+…` form.      |
//! | `TELEVIM_TEST_CODE`    | login    | The login code Telegram delivers for that account.   |
//! | `TELEVIM_TEST_PASSWORD`| 2FA only | Two-factor password, for an account that has one.    |
//!
//! Each login test requests its own code, and Telegram throttles that hard, so
//! run them sparingly. To exercise a test datacenter rather than the production
//! one, pre-seed the session file with a test-DC home datacenter before the
//! first run; the client then stays on it.

#![cfg(feature = "live")]

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use telegram_framework::session::FileStore;
use telegram_framework::{
    AuthError, Client, ClientBuilder, FileKey, KeyProvider, LoginToken, SessionError, SignInResult,
    tl,
};

/// Credentials and configuration for the opt-in tests.
struct TestDc {
    api_id: i32,
    api_hash: String,
    phone: Option<String>,
    code: Option<String>,
    password: Option<String>,
}

impl TestDc {
    /// Reads the configuration, or returns `None` when the tests are not
    /// enabled.
    fn from_env() -> Option<Self> {
        if env::var("TELEVIM_TEST_DC").as_deref() != Ok("1") {
            return None;
        }

        Some(Self {
            api_id: env::var("TELEVIM_API_ID").ok()?.parse().ok()?,
            api_hash: env::var("TELEVIM_API_HASH").ok()?,
            phone: env::var("TELEVIM_TEST_PHONE").ok(),
            code: env::var("TELEVIM_TEST_CODE").ok(),
            password: env::var("TELEVIM_TEST_PASSWORD").ok(),
        })
    }

    /// The phone number and login code, or `None` when the test needs to skip.
    fn login_credentials(&self) -> Option<(&str, &str)> {
        Some((self.phone.as_deref()?, self.code.as_deref()?))
    }
}

/// A fixed file key, so a test never reaches the OS credential store.
#[derive(Debug)]
struct TestKey;

impl KeyProvider for TestKey {
    fn sealing_key(&self) -> Result<([u8; 16], FileKey), SessionError> {
        Ok(([1; 16], FileKey::from_bytes([1; 32])))
    }

    fn opening_key(&self, _salt: &[u8; 16]) -> Result<FileKey, SessionError> {
        Ok(FileKey::from_bytes([1; 32]))
    }
}

/// Builds a client backed by the session file at `path`.
async fn build_client(dc: &TestDc, path: &Path) -> Client {
    ClientBuilder::new(dc.api_id, dc.api_hash.clone())
        .session_store(Box::new(FileStore::with_key_provider(
            path,
            Arc::new(TestKey),
        )))
        .build()
        .await
        .expect("the client builds")
}

/// A scratch session path, so the tests never touch the developer's own session.
fn session_path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temporary directory is created");
    let path = dir.path().join("session.json");
    (dir, path)
}

/// Logs in and leaves the account signed in, whichever branch the account takes.
///
/// Returns the login token so a caller can prove it cannot be redeemed twice.
/// `None` means Telegram answered with a step this build does not know: the
/// result enum is `#[non_exhaustive]`, so a future Telegram may add one, and the
/// caller should skip rather than fail on a step it was never written for.
async fn log_in(client: &Client, dc: &TestDc) -> Option<LoginToken> {
    let (phone, code) = dc
        .login_credentials()
        .expect("TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE are set");

    let token = client
        .request_login_code(phone)
        .await
        .expect("telegram sends a login code");

    match client
        .sign_in(&token, code)
        .await
        .expect("the login code is accepted")
    {
        SignInResult::Success => {}
        SignInResult::PasswordRequired(password_token) => {
            let password = dc
                .password
                .as_deref()
                .expect("this account has two-factor authentication; set TELEVIM_TEST_PASSWORD");
            client
                .check_password(password_token, password)
                .await
                .expect("the two-factor password is accepted");
        }
        _ => {
            eprintln!("skipped: telegram answered with a sign-in step this build does not know");
            return None;
        }
    }

    Some(token)
}

#[tokio::test]
async fn login_round_trip_persists_the_session() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };
    if dc.login_credentials().is_none() {
        eprintln!("skipped: set TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE");
        return;
    }

    let (_dir, path) = session_path();

    let client = build_client(&dc, &path).await;
    let Some(token) = log_in(&client, &dc).await else {
        return;
    };
    assert!(
        client
            .is_authorized()
            .await
            .expect("authorization is checked"),
        "the client should be authorized after logging in"
    );

    // The token is single use, and the refusal has to happen locally. The
    // deliberately wrong code is the proof: if the guard ever moved behind the
    // request, Telegram would answer `InvalidCode` instead, and this would fail
    // rather than pass for the wrong reason.
    let error = client
        .sign_in(&token, "00000")
        .await
        .expect_err("a spent login token must be refused");
    assert!(
        matches!(error, AuthError::TokenAlreadyUsed),
        "expected the spent token to be refused, got {error:?}"
    );

    // Persist explicitly, then drop the client and rebuild from the same file:
    // the session must survive the restart on its own.
    client.persist_session().expect("the session is persisted");
    drop(client);

    assert!(path.exists(), "the session file should have been written");

    let restored = build_client(&dc, &path).await;
    assert!(
        restored
            .is_authorized()
            .await
            .expect("authorization is checked"),
        "a client rebuilt from the stored session should be authorized"
    );
}

#[tokio::test]
async fn two_factor_login_reports_password_required() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };
    let Some((phone, code)) = dc.login_credentials() else {
        eprintln!("skipped: set TELEVIM_TEST_PHONE and TELEVIM_TEST_CODE");
        return;
    };
    let Some(password) = dc.password.as_deref() else {
        eprintln!("skipped: set TELEVIM_TEST_PASSWORD to exercise the two-factor branch");
        return;
    };

    let (_dir, path) = session_path();
    let client = build_client(&dc, &path).await;

    let token = client
        .request_login_code(phone)
        .await
        .expect("telegram sends a login code");

    let SignInResult::PasswordRequired(password_token) = client
        .sign_in(&token, code)
        .await
        .expect("the login code is accepted")
    else {
        panic!("the test account is expected to have two-factor authentication enabled");
    };

    // Nothing is persisted until the password step completes.
    assert!(
        !path.exists(),
        "a half-finished login must not be written to the store"
    );

    client
        .check_password(password_token, password)
        .await
        .expect("the two-factor password is accepted");
    assert!(
        client
            .is_authorized()
            .await
            .expect("authorization is checked"),
        "the client should be authorized once the password is accepted"
    );
    assert!(path.exists(), "the completed session should be persisted");
}

#[tokio::test]
async fn raw_invoke_returns_a_pong() {
    let Some(dc) = TestDc::from_env() else {
        eprintln!("skipped: set TELEVIM_TEST_DC=1 to run against a real datacenter");
        return;
    };

    let (_dir, path) = session_path();
    let client = build_client(&dc, &path).await;

    // `ping` needs an encrypted connection but not a signed-in account, so this
    // exercises the escape hatch without spending a login code.
    let pong = client
        .invoke(&tl::functions::Ping { ping_id: 42 })
        .await
        .expect("telegram answers the ping");

    match pong {
        tl::enums::Pong::Pong(pong) => assert_eq!(pong.ping_id, 42),
    }
}
