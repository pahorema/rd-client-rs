//! One shared DSM login for every caller using the same credentials.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::{Client, DEFAULT_TIMEOUT, Error, Session};

/// DSM address and account a session is opened for.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// DSM URL, for example `http://nas:5000`.
    pub url: String,
    /// Account name.
    pub account: String,
    /// Account password.
    pub password: String,
}

impl Credentials {
    /// Credentials for `account` on the DSM at `url`.
    pub fn new(
        url: impl Into<String>,
        account: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            account: account.into(),
            password: password.into(),
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("url", &self.url)
            .field("account", &self.account)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Hands out one shared, logged-in [`Session`] for a set of [`Credentials`].
///
/// DSM invalidates older sessions when the same account logs in many times concurrently,
/// so every caller shares a single login: concurrent first calls log in once, later calls
/// reuse the session while the credentials stay the same, and changed credentials log in
/// again.
///
/// Cheap to clone; clones share the cached session.
#[derive(Debug, Clone)]
pub struct SessionProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    timeout: Duration,
    current: Mutex<Option<Shared>>,
}

#[derive(Debug)]
struct Shared {
    credentials: Credentials,
    session: Session,
}

impl Default for SessionProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionProvider {
    /// A provider whose clients use [`DEFAULT_TIMEOUT`].
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// A provider whose clients apply `timeout` to every request.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                timeout,
                current: Mutex::new(None),
            }),
        }
    }

    /// The shared session for `credentials`, logging in when there is none yet or when the
    /// credentials changed.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of [`Client::connect_with_timeout`] or [`Client::login`]; nothing is
    /// cached then, so the next call tries again.
    pub async fn session(&self, credentials: &Credentials) -> Result<Session, Error> {
        let mut current = self.inner.current.lock().await;
        if let Some(shared) = current.as_ref()
            && shared.credentials == *credentials
        {
            return Ok(shared.session.clone());
        }
        let client = Client::connect_with_timeout(&credentials.url, self.inner.timeout).await?;
        let session = client
            .login(&credentials.account, &credentials.password)
            .await?;
        *current = Some(Shared {
            credentials: credentials.clone(),
            session: session.clone(),
        });
        Ok(session)
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SessionProvider>();
};
