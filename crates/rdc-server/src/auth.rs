//! Signed persistent sessions shared by the web and torrent-client HTTP APIs.
//!
//! Protect routers with `from_fn_with_state(AuthPolicy::new(...), require_auth)`.
//! Successful requests carry [`RequestUser`]. Anonymous endpoints can call
//! [`Sessions::is_authenticated`] and [`AuthPolicy::is_disabled`] themselves.
//! After a credential update, await [`Sessions::invalidate`] before responding.
//! Login and logout return a header to append as `SET_COOKIE`; both must receive
//! the request's [`RequestBasePath`] (or its empty root equivalent).

use std::{
    fmt,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use data_encoding::BASE64URL_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use rdc_store::{LoginOutcome, Store};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::http::RequestBasePath;

/// Persistent lifetime; refresh begins strictly after the midpoint.
const LIFETIME: u64 = 14 * 24 * 60 * 60;
/// Maximum delay for external credential changes; local changes invalidate it.
const STAMP_CACHE_TTL: Duration = Duration::from_secs(30);
/// Version byte, issue time and SHA-256 stamp digest precede the UTF-8 user id.
const PAYLOAD_PREFIX: usize = 1 + 8 + 32;
/// Bound decoding of untrusted cookies before allocation.
const MAX_COOKIE_LENGTH: usize = 4096;

type Signature = Hmac<Sha256>;

/// Failure to initialize sessions or complete a login operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The key file could not be read, locked, created or synchronized.
    #[error("session key file failed: {0}")]
    KeyFile(#[from] io::Error),
    /// The operating system could not supply key material.
    #[error("session key generation failed")]
    Random,
    /// The key-file worker failed.
    #[error("session key worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    /// A login could not access the user store.
    #[error(transparent)]
    Store(#[from] rdc_store::StoreError),
    /// A cookie path cannot be represented as an HTTP header.
    #[error("invalid session cookie path")]
    CookiePath,
}

/// Identity inserted into every request accepted by [`require_auth`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestUser {
    /// A signed cookie whose account security stamp still matches.
    Authenticated {
        /// Stable stored account id, not the user name or security stamp.
        user_id: String,
    },
    /// Authorization was bypassed by the injected configuration switch.
    AuthenticationDisabled,
}

/// Cheaply cloned session signer and short-lived security-stamp cache.
#[derive(Clone)]
pub struct Sessions(Arc<Inner>);

struct Inner {
    store: Store,
    signer: Signature,
    stamp: Mutex<Option<CachedStamp>>,
}

struct CachedStamp {
    user_id: String,
    digest: [u8; 32],
    loaded: Instant,
}

struct ValidSession {
    user_id: String,
    digest: [u8; 32],
    refresh: bool,
}

impl fmt::Debug for Sessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sessions").finish_non_exhaustive()
    }
}

impl Sessions {
    /// Loads or creates the private 32-byte key at `key_path`.
    ///
    /// The parent directory must exist. File locking serializes simultaneous
    /// initializers; newly created Unix files have mode 0600. Key contents and
    /// session credentials are never logged or included in debug output.
    ///
    /// # Errors
    /// Fails on key-file I/O, an invalid key length, unavailable randomness or
    /// failure of the blocking key-file worker. Invalid keys are never replaced.
    pub async fn open(store: Store, key_path: &Path) -> Result<Self, AuthError> {
        let key_path = key_path.to_owned();
        let key = tokio::task::spawn_blocking(move || load_key(&key_path)).await??;
        Ok(Self(Arc::new(Inner {
            store,
            signer: Signature::new_from_slice(&key).expect("HMAC accepts a 32-byte key"),
            stamp: Mutex::new(None),
        })))
    }

    /// Verifies credentials and returns a persistent `SID` Set-Cookie header.
    ///
    /// Blank user names and empty passwords fail without accessing the store.
    /// Unknown credentials and locked accounts return `None`. This does not
    /// honor the disabled switch: anonymous login endpoints decide that policy.
    ///
    /// # Errors
    /// Fails on user-store errors or an invalid base path.
    pub async fn login(
        &self,
        user_name: &str,
        password: &str,
        base_path: &RequestBasePath,
    ) -> Result<Option<HeaderValue>, AuthError> {
        if user_name.trim().is_empty() || password.is_empty() {
            return Ok(None);
        }
        let LoginOutcome::Success(user) = self
            .0
            .store
            .users()
            .verify_login(user_name, password)
            .await?
        else {
            return Ok(None);
        };
        let Some(stamp) = user.security_stamp else {
            return Ok(None);
        };
        self.invalidate(&user.id).await;
        self.issue(
            &user.id,
            &Sha256::digest(stamp.as_bytes()).into(),
            now(),
            base_path,
        )
        .map(Some)
    }

    /// Returns the requesting account's name for torrent-client preference responses;
    /// with authentication disabled, the name of the only account.
    ///
    /// # Errors
    /// Returns store access or row decoding errors.
    pub async fn user_name(&self, user: &RequestUser) -> Result<Option<String>, AuthError> {
        let users = self.0.store.users();
        let account = match user {
            RequestUser::Authenticated { user_id } => users.get(user_id).await?,
            RequestUser::AuthenticationDisabled => users.first().await?,
        };
        Ok(account.and_then(|user| user.user_name))
    }

    /// Returns an expiring `SID` Set-Cookie header at this request's mount path.
    ///
    /// Logout removes the browser cookie, not other sessions of the account.
    /// # Errors
    /// Fails if the base path cannot be represented safely in a cookie.
    pub fn logout(&self, base_path: &RequestBasePath) -> Result<HeaderValue, AuthError> {
        cookie_header("", 0, base_path)
    }

    /// Checks the cookie only; malformed cookies and store failures fail closed.
    ///
    /// This ignores the authentication-disabled switch and does not refresh the
    /// response. Use [`require_auth`] for protected routes and sliding renewal.
    pub async fn is_authenticated(&self, headers: &HeaderMap) -> bool {
        self.authenticate(headers, now()).await.is_some()
    }

    /// Clears a cached stamp after a successful credential change.
    ///
    /// Await this after `Store::users().update_credentials(...)` and before
    /// responding. Clones share this cache. In-flight stamp reads cannot restore
    /// an invalidated entry because loading and invalidation share one lock.
    pub async fn invalidate(&self, user_id: &str) {
        let mut stamp = self.0.stamp.lock().await;
        if stamp
            .as_ref()
            .is_some_and(|cached| cached.user_id == user_id)
        {
            *stamp = None;
        }
    }

    async fn authenticate(&self, headers: &HeaderMap, now: u64) -> Option<ValidSession> {
        let value = sid(headers)?;
        if value.len() > MAX_COOKIE_LENGTH {
            return None;
        }
        let (payload, signature) = value.split_once('.')?;
        let signature = BASE64URL_NOPAD.decode(signature.as_bytes()).ok()?;
        let mut signer = self.0.signer.clone();
        signer.update(payload.as_bytes());
        signer.verify_slice(&signature).ok()?;
        let payload = BASE64URL_NOPAD.decode(payload.as_bytes()).ok()?;
        if payload.len() <= PAYLOAD_PREFIX || payload[0] != 1 {
            return None;
        }
        let issued = u64::from_be_bytes(payload[1..9].try_into().ok()?);
        let age = now.checked_sub(issued)?;
        if age >= LIFETIME {
            return None;
        }
        let user_id = str::from_utf8(&payload[PAYLOAD_PREFIX..]).ok()?;
        let digest = self.stamp_digest(user_id).await?;
        if !bool::from(digest.ct_eq(&payload[9..PAYLOAD_PREFIX])) {
            return None;
        }
        Some(ValidSession {
            user_id: user_id.to_owned(),
            digest,
            refresh: age > LIFETIME / 2,
        })
    }

    async fn stamp_digest(&self, user_id: &str) -> Option<[u8; 32]> {
        let mut cache = self.0.stamp.lock().await;
        if let Some(stamp) = cache.as_ref()
            && stamp.user_id == user_id
            && stamp.loaded.elapsed() < STAMP_CACHE_TTL
        {
            return Some(stamp.digest);
        }
        let stamp = self.0.store.users().security_stamp(user_id).await.ok()??;
        let digest = Sha256::digest(stamp.as_bytes()).into();
        *cache = Some(CachedStamp {
            user_id: user_id.to_owned(),
            digest,
            loaded: Instant::now(),
        });
        Some(digest)
    }

    fn issue(
        &self,
        user_id: &str,
        digest: &[u8; 32],
        issued: u64,
        base_path: &RequestBasePath,
    ) -> Result<HeaderValue, AuthError> {
        let mut payload = Vec::with_capacity(PAYLOAD_PREFIX + user_id.len());
        payload.push(1);
        payload.extend_from_slice(&issued.to_be_bytes());
        payload.extend_from_slice(digest);
        payload.extend_from_slice(user_id.as_bytes());
        let payload = BASE64URL_NOPAD.encode(&payload);
        let mut signer = self.0.signer.clone();
        signer.update(payload.as_bytes());
        let signature = BASE64URL_NOPAD.encode(&signer.finalize().into_bytes());
        cookie_header(&format!("{payload}.{signature}"), LIFETIME, base_path)
    }
}

/// Session validation combined with a live authentication-disabled setting.
#[derive(Clone)]
pub struct AuthPolicy {
    sessions: Sessions,
    disabled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl fmt::Debug for AuthPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthPolicy").finish_non_exhaustive()
    }
}

impl AuthPolicy {
    /// Builds the state passed to `axum::middleware::from_fn_with_state`.
    pub fn new(sessions: Sessions, disabled: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self { sessions, disabled }
    }

    /// Reads the current injected setting, including changes after startup.
    pub fn is_disabled(&self) -> bool {
        (self.disabled)()
    }
}

/// Rejects unauthorized requests with an empty 403, otherwise inserts [`RequestUser`].
///
/// Mount this inside [`crate::http::AppBuilder`] so [`RequestBasePath`] is available.
/// Sliding renewal never overwrites an explicit downstream SID login/logout.
pub async fn require_auth(
    State(policy): State<AuthPolicy>,
    mut request: Request,
    next: Next,
) -> Response {
    if policy.is_disabled() {
        request
            .extensions_mut()
            .insert(RequestUser::AuthenticationDisabled);
        return next.run(request).await;
    }
    let Some(session) = policy.sessions.authenticate(request.headers(), now()).await else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let refresh = if session.refresh {
        let root = RequestBasePath(String::new());
        let base = request
            .extensions()
            .get::<RequestBasePath>()
            .unwrap_or(&root);
        policy
            .sessions
            .issue(&session.user_id, &session.digest, now(), base)
            .ok()
    } else {
        None
    };
    request.extensions_mut().insert(RequestUser::Authenticated {
        user_id: session.user_id,
    });
    let mut response = next.run(request).await;
    if let Some(refresh) = refresh
        && !response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().is_ok_and(|value| value.starts_with("SID=")))
    {
        response.headers_mut().append(header::SET_COOKIE, refresh);
    }
    response
}

fn sid(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| part.trim().strip_prefix("SID="))
}

fn cookie_header(
    value: &str,
    max_age: u64,
    base: &RequestBasePath,
) -> Result<HeaderValue, AuthError> {
    if base.0.contains([';', '\r', '\n']) || (!base.0.is_empty() && !base.0.starts_with('/')) {
        return Err(AuthError::CookiePath);
    }
    let path = format!("{}/", base.0.trim_end_matches('/'));
    HeaderValue::from_str(&format!(
        "SID={value}; HttpOnly; SameSite=Lax; Path={path}; Max-Age={max_age}"
    ))
    .map_err(|_| AuthError::CookiePath)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_key(path: &Path) -> Result<[u8; 32], AuthError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.lock()?;
    let mut key = [0; 32];
    match file.metadata()?.len() {
        0 => {
            getrandom::fill(&mut key).map_err(|_| AuthError::Random)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&key)?;
            file.sync_all()?;
        }
        32 => file.read_exact(&mut key)?,
        _ => {
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "session key must be 32 bytes").into(),
            );
        }
    }
    Ok(key)
}

#[cfg(test)]
mod tests;
