//! A logged-in DSM session.

use std::sync::Arc;

use serde::de::IgnoredAny;

use crate::{Client, Error};

/// A logged-in DSM session, created by [`Client::login`].
///
/// Cheap to clone; clones are handles to the same login.
#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    client: Client,
    sid: String,
}

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
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();
};
