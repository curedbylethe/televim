//! The raw API escape hatch.
//!
//! `grammers` covers the calls a chat client needs, but not all of them, and
//! `televim` must not have to fork or wait upstream to reach the rest.
//! [`Client::invoke`] forwards a request straight to Telegram and hands back
//! whatever came out, typed as the response the request declares.
//!
//! The escape hatch is intentionally generic: there is no hand-written wrapper
//! per method, because that is exactly the surface this avoids.

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};

impl Client {
    /// Invokes an `MTProto` method and returns its raw response.
    ///
    /// `R` is any request from [`tl::functions`](crate::tl::functions). Its
    /// [`RemoteCall::Return`](crate::tl::RemoteCall::Return) associated type
    /// decides the response type, so the answer is typed without the caller
    /// naming it.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram rejects the request,
    /// when the connection fails, or when the response cannot be decoded.
    /// [`RequestError::Rpc`] keeps the error code and name, so callers can still
    /// react to a specific failure.
    ///
    /// # Caveats
    ///
    /// The `grammers` request types are only guaranteed to match the layer
    /// `grammers` was generated for. A request built for a different layer may
    /// be rejected or answered with something this build cannot decode.
    ///
    /// The request may also move the session — a datacenter migration, a peer
    /// cached from the response — in which case the session is written back
    /// before this returns. A failed write is logged at `warn` rather than
    /// reported as a failed request, because the request itself succeeded.
    ///
    /// # Examples
    ///
    /// Ping Telegram and read the `Pong` it answers with:
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::{ClientBuilder, tl};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// let pong = client.invoke(&tl::functions::Ping { ping_id: 42 }).await?;
    /// println!("telegram answered with {pong:?}");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Ask for the update state — the same call
    /// [`Client::is_authorized`](crate::Client::is_authorized) makes
    /// internally:
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::{ClientBuilder, tl};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let client = ClientBuilder::new(1234, "api-hash")
    /// #     .session_store(Box::new(MemoryStore::new()))
    /// #     .build()
    /// #     .await?;
    /// let state = client.invoke(&tl::functions::updates::GetState {}).await?;
    /// println!("update state: {state:?}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn invoke<R: crate::tl::RemoteCall>(
        &self,
        request: &R,
    ) -> Result<R::Return, FrameworkError> {
        let response = self
            .inner()
            .invoke(request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        // A successful call can migrate the datacenter or cache a peer, and
        // that only reaches the store if it is written back here.
        self.flush_session();

        Ok(response)
    }
}
