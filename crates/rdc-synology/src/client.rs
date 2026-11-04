//! Connection to one DSM and the request/response envelope shared by every API.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::Error;
use crate::api::{Api, ApiDescription, Apis};

/// Request timeout used by [`Client::connect`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(100);

/// Connection to one DSM, with the API locations it reported.
///
/// Cheap to clone; clones share the connection pool.
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "read by the authenticated calls")]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
#[expect(dead_code, reason = "read by the authenticated calls")]
struct Inner {
    http: reqwest::Client,
    /// `<dsm url>/webapi`, without a trailing slash.
    base: String,
    apis: Apis,
}

/// Response envelope of every DSM Web API call.
#[derive(Deserialize)]
struct Envelope {
    data: Option<Value>,
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
    finish(response).await
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

/// Unwraps the envelope and decodes `data` into `T`.
async fn finish<T: DeserializeOwned>(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<T, Error> {
    let response = response.map_err(Error::Transport)?;
    let body = response.text().await.map_err(Error::Transport)?;
    let envelope: Envelope = match serde_json::from_str(&body) {
        Ok(envelope) => envelope,
        Err(source) => return Err(Error::Decode { source, body }),
    };
    // Calls without a result answer with no `data`; treat that as an empty object.
    let data = envelope.data.unwrap_or_else(|| Value::Object(Map::new()));
    serde_json::from_value(data).map_err(|source| Error::Decode { source, body })
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
