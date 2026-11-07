//! Connection to one DSM and the request/response envelope shared by every API.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::api::{Api, ApiDescription, Apis};
use crate::{ApiError, Error, Session};

/// Request timeout used by [`Client::connect`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(100);

/// Connection to one DSM, with the API locations it reported.
///
/// Cheap to clone; clones share the connection pool. Log in with [`Client::login`] to call
/// the Download Station and File Station APIs.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    /// `<dsm url>/webapi`, without a trailing slash.
    base: String,
    apis: Apis,
}

/// Response envelope of every DSM Web API call.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    success: bool,
    data: Option<Value>,
    error: Option<ErrorBody>,
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    errors: Vec<ErrorDetailBody>,
}

#[derive(Deserialize)]
struct LoginData {
    sid: String,
}

#[derive(Deserialize)]
struct ErrorDetailBody {
    #[serde(default)]
    code: i64,
    path: Option<String>,
}

impl Client {
    /// Connects to the DSM at `url` (for example `http://nas:5000`) with
    /// [`DEFAULT_TIMEOUT`] and discovers where its APIs live.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUrl`] for a blank or non-HTTP URL, or any [`Error`] of the
    /// `SYNO.API.Info` query.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        Self::connect_with_timeout(url, DEFAULT_TIMEOUT).await
    }

    /// Like [`Client::connect`], with `timeout` applied to every request of this client.
    ///
    /// # Errors
    ///
    /// Same as [`Client::connect`].
    pub async fn connect_with_timeout(url: &str, timeout: Duration) -> Result<Self, Error> {
        let base = webapi_base(url)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
            .timeout(timeout)
            .build()
            .map_err(Error::ClientBuild)?;
        let descriptions: HashMap<String, ApiDescription> = get(
            &http,
            &base,
            &Apis::info(),
            "query",
            &[("query", "all")],
            None,
        )
        .await?;
        let apis = Apis::discovered(&descriptions);
        tracing::debug!(base, ?apis, "discovered Synology APIs");
        Ok(Self {
            inner: Arc::new(Inner { http, base, apis }),
        })
    }

    /// Logs in with `SYNO.API.Auth` and returns the session. The credentials travel in a
    /// `POST` body, never in the URL.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a blank account or password, [`Error::Api`] when DSM
    /// refuses the login (for example code 400 for wrong credentials or 403 when 2-step
    /// verification is required), or any other [`Error`] of the round trip.
    pub async fn login(&self, account: &str, password: &str) -> Result<Session, Error> {
        if account.trim().is_empty() {
            return Err(Error::InvalidArgument("the Synology account is required"));
        }
        if password.trim().is_empty() {
            return Err(Error::InvalidArgument("the Synology password is required"));
        }
        let api = &self.inner.apis.auth;
        let params = [
            ("account", account),
            ("passwd", password),
            ("format", "sid"),
        ];
        let version = api.version.to_string();
        let form = request_params(api, "login", &version, &params, None);
        let response = self
            .inner
            .http
            .post(endpoint(&self.inner.base, api))
            .form(&form)
            .send()
            .await;
        let login: LoginData = finish(response, api, "login").await?;
        tracing::debug!(account, "logged in to Synology");
        Ok(Session::new(self.clone(), login.sid))
    }

    pub(crate) fn apis(&self) -> &Apis {
        &self.inner.apis
    }

    /// Calls `api`/`method` with `GET`, the common parameters, `params` and the session id.
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        api: &Api,
        method: &'static str,
        params: &[(&str, &str)],
        sid: Option<&str>,
    ) -> Result<T, Error> {
        get(&self.inner.http, &self.inner.base, api, method, params, sid).await
    }
}

async fn get<T: DeserializeOwned>(
    http: &reqwest::Client,
    base: &str,
    api: &Api,
    method: &'static str,
    params: &[(&str, &str)],
    sid: Option<&str>,
) -> Result<T, Error> {
    let version = api.version.to_string();
    let query = request_params(api, method, &version, params, sid);
    let response = http.get(endpoint(base, api)).query(&query).send().await;
    finish(response, api, method).await
}

/// Common parameters in the order DSM documents them, then `params`, then the session.
fn request_params<'a>(
    api: &'a Api,
    method: &'a str,
    version: &'a str,
    params: &[(&'a str, &'a str)],
    sid: Option<&'a str>,
) -> Vec<(&'a str, &'a str)> {
    let mut query = Vec::with_capacity(params.len() + 5);
    query.extend([("api", api.name), ("version", version), ("method", method)]);
    query.extend_from_slice(params);
    if let Some(session) = api.session {
        query.push(("session", session));
    }
    if let Some(sid) = sid {
        query.push(("_sid", sid));
    }
    query
}

fn endpoint(base: &str, api: &Api) -> String {
    format!("{base}/{}", api.path.trim_start_matches('/'))
}

/// Checks the HTTP status, unwraps the envelope and decodes `data` into `T`.
async fn finish<T: DeserializeOwned>(
    response: Result<reqwest::Response, reqwest::Error>,
    api: &Api,
    method: &'static str,
) -> Result<T, Error> {
    let response = response.map_err(transport)?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(Error::UnexpectedStatus(status.as_u16()));
    }
    let body = response.text().await.map_err(transport)?;
    let envelope: Envelope = match serde_json::from_str(&body) {
        Ok(envelope) => envelope,
        Err(source) => return Err(Error::Decode { source, body }),
    };
    if !envelope.success {
        let error = envelope.error.unwrap_or_default();
        let api_error = error.errors.into_iter().fold(
            ApiError::new(api.name, method, api.version, error.code),
            |api_error, detail| api_error.with_detail(detail.code, detail.path),
        );
        return Err(api_error.into());
    }
    // Calls without a result answer with no `data`; treat that as an empty object.
    let data = envelope.data.unwrap_or_else(|| Value::Object(Map::new()));
    serde_json::from_value(data).map_err(|source| Error::Decode { source, body })
}

/// Wraps a request failure without its URL: the query carries the session id (`_sid`) and,
/// for created tasks, the download link, neither of which may reach logs or the user.
fn transport(error: reqwest::Error) -> Error {
    Error::Transport(error.without_url())
}

/// `<url>/webapi` for a DSM URL, rejecting anything that is not an absolute HTTP(S) URL.
fn webapi_base(url: &str) -> Result<String, Error> {
    let trimmed = url.trim();
    let valid = reqwest::Url::parse(trimmed)
        .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https") && parsed.has_host());
    if !valid {
        return Err(Error::InvalidUrl(url.to_owned()));
    }
    Ok(format!("{}/webapi", trimmed.trim_end_matches('/')))
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client>();
    assert_send_sync::<Error>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webapi_base_appends_the_api_root() {
        assert_eq!(
            webapi_base("http://nas:5000").unwrap(),
            "http://nas:5000/webapi"
        );
        assert_eq!(
            webapi_base(" https://nas:5001/ ").unwrap(),
            "https://nas:5001/webapi"
        );
    }

    #[test]
    fn webapi_base_rejects_blank_and_non_http_urls() {
        for url in ["", "   ", "nas:5000", "ftp://nas", "not a url"] {
            assert!(
                matches!(webapi_base(url), Err(Error::InvalidUrl(_))),
                "{url:?}"
            );
        }
    }

    #[test]
    fn endpoint_joins_paths_with_one_slash() {
        let api = Apis::default().task;

        assert_eq!(
            endpoint("http://nas/webapi", &api),
            "http://nas/webapi/DownloadStation/task.cgi"
        );
    }
}
