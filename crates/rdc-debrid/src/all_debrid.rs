//! Client of the AllDebrid API (version 4.1).

use std::time::Duration;

use jiff::Timestamp;
use rdc_model::{RemoteFile, Torrent};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::endpoint::Endpoint;
use crate::error::DebridError;
use crate::http::ProviderHttp;
use crate::json;
use crate::provider::{DebridUser, ProviderConfig, RemoteTorrent};
use crate::text_rate_limit::classify_add_error;

/// Root of the AllDebrid API.
pub const DEFAULT_BASE_URL: &str = "https://api.alldebrid.com/v4.1/";

/// Application name sent with every request, as AllDebrid requires.
const AGENT: &str = "rd-client";

/// Error code of an unknown magnet.
const MAGNET_INVALID_ID: &str = "MAGNET_INVALID_ID";

/// Extra listings searched for the id of an added magnet AllDebrid did not
/// return, one per [`ID_LOOKUP_INTERVAL`].
const ID_LOOKUP_RETRIES: u32 = 10;

/// Pause between two listings searching an added magnet's id.
const ID_LOOKUP_INTERVAL: Duration = Duration::from_secs(1);

/// AllDebrid account access.
///
/// The torrent listing uses the API's live mode: the first call of a session
/// returns every magnet, later calls only the properties that changed, which
/// the client merges into the state it keeps.
#[derive(Debug)]
pub struct AllDebrid {
    endpoint: Endpoint,
    http: ProviderHttp,
    /// Held across a listing call so the session counter stays in sync.
    live: Mutex<LiveListing>,
}

impl AllDebrid {
    /// Creates a client for the public AllDebrid API.
    pub fn new(config: &ProviderConfig, http: ProviderHttp) -> Self {
        Self::with_base_url(config, http, DEFAULT_BASE_URL)
    }

    /// Creates a client for the API rooted at `base_url`.
    pub fn with_base_url(config: &ProviderConfig, http: ProviderHttp, base_url: &str) -> Self {
        Self {
            endpoint: Endpoint::new("AllDebrid", base_url, config),
            http,
            live: Mutex::new(LiveListing::new()),
        }
    }

    /// Returns the account the API key belongs to; the expiration is only
    /// reported for premium accounts.
    pub async fn user(&self) -> Result<DebridUser, DebridError> {
        let data: UserData = self.call(self.request(Method::GET, "user")?).await?;
        let user = data.user.ok_or_else(|| {
            DebridError::InvalidResponse("AllDebrid did not return the user".to_owned())
        })?;
        Ok(DebridUser {
            username: user.username,
            expiration: epoch(user.premium_until.filter(|seconds| *seconds > 0)),
            premium: false,
        })
    }

    /// Lists every magnet of the account, applying the changes reported
    /// since the previous call.
    ///
    /// Magnets whose status is still unknown are left out. The listing has
    /// no file lists; refreshing a finished torrent loads them.
    pub async fn list_torrents(&self) -> Result<Vec<RemoteTorrent>, DebridError> {
        let mut live = self.live.lock().await;
        let request = self
            .request(Method::POST, "magnet/status")?
            .form(&[("session", live.session), ("counter", live.counter)]);
        let update: LiveStatus = self.call(request).await?;
        live.apply(update);
        Ok(live
            .magnets
            .iter()
            .filter(|magnet| magnet.status.is_some())
            .map(Magnet::to_remote)
            .collect())
    }

    /// Removes the magnet from the account; torrents without provider id
    /// need no request.
    pub async fn delete(&self, torrent: &Torrent) -> Result<(), DebridError> {
        let Some(id) = torrent.rd_id.as_deref() else {
            return Ok(());
        };
        let request = self
            .request(Method::GET, "magnet/delete")?
            .query(&[("id", id)]);
        self.call::<IgnoredAny>(request).await?;
        Ok(())
    }

    /// Turns a restricted link into a direct download URL.
    pub async fn unrestrict(&self, link: &str) -> Result<String, DebridError> {
        let request = self
            .request(Method::GET, "link/unlock")?
            .query(&[("link", link)]);
        let unlocked: Unlocked = self.call(request).await?;
        unlocked
            .link
            .filter(|link| !link.trim().is_empty())
            .ok_or_else(|| {
                DebridError::InvalidResponse("AllDebrid did not return a download link".to_owned())
            })
    }

    /// Adds a magnet link and returns the id of the new magnet.
    ///
    /// Errors mentioning `slow_down` or `rate limit exceeded` are reported as
    /// [`DebridError::RateLimited`].
    pub async fn add_magnet(&self, magnet: &str) -> Result<String, DebridError> {
        let request = self
            .request(Method::POST, "magnet/upload")?
            .form(&[("magnets[]", magnet)]);
        self.add(request, |upload| upload.magnets).await
    }

    /// Uploads a `.torrent` file and returns the id of the new magnet; rate
    /// limits are detected like for [`add_magnet`](Self::add_magnet).
    pub async fn add_torrent_file(&self, bytes: &[u8]) -> Result<String, DebridError> {
        let part = Part::bytes(bytes.to_vec())
            .file_name("1.torrent")
            .mime_str("application/x-bittorrent")?;
        let request = self
            .request(Method::POST, "magnet/upload/file")?
            .multipart(Form::new().part("files[]", part));
        self.add(request, |upload| upload.files).await
    }

    async fn add(
        &self,
        request: RequestBuilder,
        entries: fn(Upload) -> Option<Vec<Uploaded>>,
    ) -> Result<String, DebridError> {
        let id = async {
            let upload: Upload = self.call(request).await?;
            let entry = entries(upload)
                .and_then(|entries| entries.into_iter().next())
                .ok_or_else(|| {
                    DebridError::InvalidResponse(
                        "AllDebrid did not return the added magnet".to_owned(),
                    )
                })?;
            if let Some(error) = entry.error.filter(ApiError::has_message) {
                return Err(error.into_error());
            }
            match entry.id.filter(|id| *id != 0) {
                Some(id) => Ok(id),
                None => self.find_id(entry.hash.as_deref()).await,
            }
        }
        .await
        .map_err(classify_add_error)?;
        tracing::debug!(id, "magnet added to AllDebrid");
        Ok(id.to_string())
    }

    /// Searches the magnet list for the id of the magnet with `hash`.
    async fn find_id(&self, hash: Option<&str>) -> Result<i64, DebridError> {
        for attempt in 0..=ID_LOOKUP_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(ID_LOOKUP_INTERVAL).await;
            }
            let list: MagnetList = self
                .call(self.request(Method::GET, "magnet/status")?)
                .await?;
            let found = list.magnets.unwrap_or_default().into_iter().find(|magnet| {
                hash.is_some_and(|hash| {
                    magnet
                        .hash
                        .as_deref()
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(hash))
                })
            });
            if let Some(id) = found.and_then(|magnet| magnet.id) {
                return Ok(id);
            }
        }
        Err(DebridError::Provider(
            "Unable to find the id of the added magnet".to_owned(),
        ))
    }

    /// An authenticated request carrying the application name.
    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, DebridError> {
        Ok(self
            .endpoint
            .request(&self.http, method, path)?
            .query(&[("agent", AGENT)]))
    }

    /// Sends `request` and decodes the `data` of its `{status, data, error}`
    /// envelope (the default when there is none).
    async fn call<T: DeserializeOwned + Default>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, DebridError> {
        let response = self.http.send(request).await?;
        let status = response.status();
        let text = response.text().await?;
        if status == StatusCode::NO_CONTENT || (status.is_success() && text.trim().is_empty()) {
            return Ok(T::default());
        }
        let envelope: Envelope = serde_json::from_str(&text).map_err(|_| {
            if status.is_success() {
                DebridError::InvalidResponse(format!(
                    "Unable to read the AllDebrid response. Response was: {text}"
                ))
            } else {
                DebridError::Provider(format!(
                    "AllDebrid answered with HTTP status {status}. Response was: {text}"
                ))
            }
        })?;
        if envelope.status.as_deref() != Some("success") {
            return Err(match envelope.error {
                Some(error) if error.has_message() => error.into_error(),
                _ => DebridError::Provider(format!("Unknown error. Response was: {text}")),
            });
        }
        match envelope.data {
            None | Some(Value::Null) => Ok(T::default()),
            Some(data) => serde_json::from_value(data).map_err(|error| {
                DebridError::InvalidResponse(format!(
                    "Unable to read the AllDebrid response ({error}). Response was: {text}"
                ))
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    status: Option<String>,
    data: Option<Value>,
    error: Option<ApiError>,
}

/// An error reported by AllDebrid, for the whole call or for one item.
#[derive(Debug, Default, Deserialize)]
struct ApiError {
    code: Option<Value>,
    message: Option<String>,
}

impl ApiError {
    fn has_message(&self) -> bool {
        self.message
            .as_deref()
            .is_some_and(|message| !message.trim().is_empty())
    }

    /// The error as `"<message> (<code>)"`; unknown magnets are
    /// [`DebridError::NotFound`].
    fn into_error(self) -> DebridError {
        let code = match self.code {
            Some(Value::String(code)) => code,
            Some(Value::Null) | None => String::new(),
            Some(code) => code.to_string(),
        };
        let message = format!("{} ({code})", self.message.unwrap_or_default());
        if code == MAGNET_INVALID_ID {
            DebridError::NotFound(message)
        } else {
            DebridError::Provider(message)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct UserData {
    user: Option<User>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    username: Option<String>,
    /// End of the premium time in Unix seconds, 0 for free accounts.
    #[serde(default, deserialize_with = "json::optional_i64")]
    premium_until: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct Unlocked {
    link: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Upload {
    #[serde(default, deserialize_with = "json::one_or_many")]
    magnets: Option<Vec<Uploaded>>,
    #[serde(default, deserialize_with = "json::one_or_many")]
    files: Option<Vec<Uploaded>>,
}

/// One uploaded magnet or file; `error` is set when it was rejected.
#[derive(Debug, Deserialize)]
struct Uploaded {
    #[serde(default, deserialize_with = "json::optional_i64")]
    id: Option<i64>,
    hash: Option<String>,
    error: Option<ApiError>,
}

#[derive(Debug, Default, Deserialize)]
struct MagnetList {
    #[serde(default, deserialize_with = "json::one_or_many")]
    magnets: Option<Vec<Magnet>>,
}

/// The magnet state of the current live-mode session.
#[derive(Debug)]
struct LiveListing {
    /// Random session id, fixed for the lifetime of the client.
    session: i64,
    /// Counter to send with the next call.
    counter: i64,
    /// Every magnet known in this session, in the order first reported.
    magnets: Vec<Magnet>,
}

impl LiveListing {
    fn new() -> Self {
        Self {
            session: fastrand::i64(1..i64::from(i32::MAX)),
            counter: 0,
            magnets: Vec::new(),
        }
    }

    /// Replaces the state with a full sync, or merges the changed
    /// properties of each reported magnet into the known one.
    fn apply(&mut self, update: LiveStatus) {
        self.counter = update.counter.unwrap_or_default();
        let magnets = update.magnets.unwrap_or_default();
        if update.fullsync == Some(true) {
            self.magnets = magnets;
            return;
        }
        for change in magnets {
            let Some(id) = change.id else {
                continue;
            };
            match self.magnets.iter_mut().find(|magnet| magnet.id == Some(id)) {
                Some(magnet) => magnet.merge(change),
                None => self.magnets.push(change),
            }
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct LiveStatus {
    #[serde(default, deserialize_with = "json::one_or_many")]
    magnets: Option<Vec<Magnet>>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    counter: Option<i64>,
    fullsync: Option<bool>,
}

/// A magnet as reported by `magnet/status`; in live-mode changes only the
/// `id` and the changed properties are present.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Magnet {
    #[serde(default, deserialize_with = "json::optional_i64")]
    id: Option<i64>,
    filename: Option<String>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    size: Option<i64>,
    hash: Option<String>,
    /// Status in plain English.
    status: Option<String>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    status_code: Option<i64>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    downloaded: Option<i64>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    seeders: Option<i64>,
    #[serde(default, deserialize_with = "json::optional_i64")]
    download_speed: Option<i64>,
    /// Unix seconds.
    #[serde(default, deserialize_with = "json::optional_i64")]
    upload_date: Option<i64>,
    /// Unix seconds, 0 while unfinished.
    #[serde(default, deserialize_with = "json::optional_i64")]
    completion_date: Option<i64>,
    #[serde(default, deserialize_with = "json::one_or_many")]
    files: Option<Vec<FileNode>>,
}

/// A node of a magnet's file tree: a file (with size and link), a folder
/// (with children) or both.
#[derive(Debug, Clone, Default, Deserialize)]
struct FileNode {
    #[serde(rename = "n", default)]
    name: String,
    #[serde(rename = "s", default, deserialize_with = "json::optional_i64")]
    size: Option<i64>,
    #[serde(rename = "e", default, deserialize_with = "json::one_or_many")]
    children: Option<Vec<FileNode>>,
    #[serde(rename = "l")]
    link: Option<String>,
}

impl Magnet {
    /// Takes every property present in `change`.
    fn merge(&mut self, change: Self) {
        fn update<T>(target: &mut Option<T>, value: Option<T>) {
            if value.is_some() {
                *target = value;
            }
        }
        update(&mut self.filename, change.filename);
        update(&mut self.size, change.size);
        update(&mut self.hash, change.hash);
        update(&mut self.status, change.status);
        update(&mut self.status_code, change.status_code);
        update(&mut self.downloaded, change.downloaded);
        update(&mut self.seeders, change.seeders);
        update(&mut self.download_speed, change.download_speed);
        update(&mut self.upload_date, change.upload_date);
        update(&mut self.completion_date, change.completion_date);
        update(&mut self.files, change.files);
    }

    /// The magnet as provider-neutral torrent. Missing dates are the Unix
    /// epoch, so an unfinished magnet ends at 1970-01-01.
    fn to_remote(&self) -> RemoteTorrent {
        RemoteTorrent {
            id: self.id.map(|id| id.to_string()).unwrap_or_default(),
            filename: self.filename.clone().unwrap_or_default(),
            original_filename: self.filename.clone(),
            hash: self.hash.clone().unwrap_or_default(),
            bytes: self.size.unwrap_or_default(),
            original_bytes: self.size.unwrap_or_default(),
            host: None,
            split: 0,
            progress: progress(self.downloaded.unwrap_or_default(), self.size.unwrap_or(1)),
            status: self.status.clone(),
            message: None,
            status_code: self.status_code.unwrap_or_default(),
            added: epoch(Some(self.upload_date.unwrap_or_default())),
            files: self.files.as_deref().map(flatten),
            links: Some(Vec::new()),
            ended: epoch(Some(self.completion_date.unwrap_or_default())),
            speed: self.download_speed,
            seeders: self.seeders,
        }
    }
}

/// `downloaded` as a percentage of `size`, rounded half to even; 0 for an
/// empty size.
fn progress(downloaded: i64, size: i64) -> i64 {
    if size <= 0 {
        return 0;
    }
    let scaled = i128::from(downloaded) * 100;
    let size = i128::from(size);
    let (quotient, remainder) = (scaled.div_euclid(size), scaled.rem_euclid(size));
    let rounded = match (2 * remainder).cmp(&size) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => quotient + (quotient & 1),
    };
    i64::try_from(rounded).unwrap_or(i64::MAX)
}

/// The files of a file tree, depth first, with `/`-separated paths. File
/// ids are 0 and no file is marked selected.
fn flatten(nodes: &[FileNode]) -> Vec<RemoteFile> {
    fn walk(nodes: &[FileNode], parent: &str, files: &mut Vec<RemoteFile>) {
        for node in nodes {
            let path = if parent.is_empty() {
                node.name.clone()
            } else {
                format!("{parent}/{}", node.name)
            };
            if let Some(bytes) = node.size {
                files.push(RemoteFile {
                    id: 0,
                    path: path.clone(),
                    bytes,
                    selected: false,
                    download_link: node.link.clone(),
                });
            }
            if let Some(children) = &node.children {
                walk(children, &path, files);
            }
        }
    }
    let mut files = Vec::new();
    walk(nodes, "", &mut files);
    files
}

/// The instant `seconds` after the Unix epoch.
fn epoch(seconds: Option<i64>) -> Option<Timestamp> {
    Timestamp::from_second(seconds?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_rounded_half_to_even() {
        assert_eq!(progress(50, 100), 50);
        assert_eq!(progress(100, 400), 25);
        assert_eq!(progress(1, 8), 12, "12.5 rounds to even");
        assert_eq!(progress(3, 8), 38, "37.5 rounds to even");
        assert_eq!(progress(2, 3), 67);
        assert_eq!(progress(5, 0), 0);
    }

    #[test]
    fn file_trees_flatten_depth_first() {
        let tree: Vec<FileNode> = serde_json::from_str(
            r#"[{"n": "Folder", "e": [{"n": "a.mkv", "s": 10, "l": "https://ad/f/a"},
                                      {"n": "Sub", "e": {"n": "b.txt", "s": 2}}]},
                {"n": "c.nfo", "s": 1, "l": "https://ad/f/c"}]"#,
        )
        .expect("tree");

        let files = flatten(&tree);

        let paths: Vec<(&str, i64)> = files
            .iter()
            .map(|file| (file.path.as_str(), file.bytes))
            .collect();
        assert_eq!(
            paths,
            [("Folder/a.mkv", 10), ("Folder/Sub/b.txt", 2), ("c.nfo", 1)]
        );
        assert_eq!(files[0].download_link.as_deref(), Some("https://ad/f/a"));
        assert!(files.iter().all(|file| file.id == 0 && !file.selected));
    }
}
