//! A logged-in DSM session.

use std::fmt;
use std::sync::Arc;

use serde::de::{DeserializeOwned, IgnoredAny};

use crate::api::{Api, Apis};
use crate::{Client, Error};

/// A logged-in DSM session, created by [`Client::login`].
///
/// Cheap to clone; clones are handles to the same login and compare equal, while two
/// separate logins never do, even with the same credentials.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    sid: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("client", &self.inner.client)
            .field("sid", &"<redacted>")
            .finish()
    }
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Session {}

impl Session {
    pub(crate) fn new(client: Client, sid: String) -> Self {
        Self {
            inner: Arc::new(Inner { client, sid }),
        }
    }

    /// Ends the session on DSM. The session (and its clones) must not be used afterwards.
    ///
    /// # Errors
    ///
    /// Any [`Error`] of the round trip; an already expired session usually reports a
    /// session error (see [`Error::is_session_error`]).
    pub async fn logout(&self) -> Result<(), Error> {
        let client = &self.inner.client;
        let _: IgnoredAny = client
            .get(
                &client.apis().auth,
                "logout",
                &[("_sid", &self.inner.sid)],
                None,
            )
            .await?;
        tracing::debug!("logged out of Synology");
        Ok(())
    }

    /// Calls the API chosen by `api` with this session's id.
    pub(crate) async fn call<T: DeserializeOwned>(
        &self,
        api: fn(&Apis) -> &Api,
        method: &'static str,
        params: &[(&str, &str)],
    ) -> Result<T, Error> {
        let client = &self.inner.client;
        client
            .get(api(client.apis()), method, params, Some(&self.inner.sid))
            .await
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();
};
