//! A scripted DSM serving the Web API calls of the Download Station engine.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

pub const URL: &str = "https://fake.url/file.mkv";
pub const GID: &str = "123456";

/// One scripted DSM answer: `data` of a success, or an error code.
#[derive(Debug, Clone)]
pub enum Reply {
    Data(Value),
    Fail(i64),
}

/// Answers of one DSM method: queued replies first, then the default.
#[derive(Debug, Clone)]
pub struct Script(Arc<Mutex<(VecDeque<Reply>, Reply)>>);

impl Script {
    pub fn new(default: Reply) -> Self {
        Self(Arc::new(Mutex::new((VecDeque::new(), default))))
    }

    pub fn then(&self, reply: Reply) -> &Self {
        self.0.lock().expect("script").0.push_back(reply);
        self
    }

    pub fn always(&self, reply: Reply) {
        let mut script = self.0.lock().expect("script");
        script.0.clear();
        script.1 = reply;
    }

    pub fn next(&self) -> ResponseTemplate {
        let mut script = self.0.lock().expect("script");
        let reply = script.0.pop_front().unwrap_or_else(|| script.1.clone());
        let body = match reply {
            Reply::Data(data) => json!({"success": true, "data": data}),
            Reply::Fail(code) => json!({"success": false, "error": {"code": code}}),
        };
        ResponseTemplate::new(200).set_body_json(body)
    }
}

pub fn task(status: i64, error_detail: &str) -> Value {
    json!({
        "id": GID,
        "type": "https",
        "title": "file.mkv",
        "size": 1000,
        "status": status,
        "status_extra": {"error_detail": error_detail},
        "additional": {
            "detail": {"uri": URL, "destination": "Media/Downloads"},
            "transfer": {"size_downloaded": 250, "speed_download": 50}
        }
    })
}

pub fn tasks(list: &[Value]) -> Reply {
    Reply::Data(json!({"offset": 0, "total": list.len(), "task": list}))
}

#[derive(Debug)]
pub struct FakeDsm {
    pub server: MockServer,
    pub get: Script,
    pub list: Script,
    pub create: Script,
    pub folder: Script,
    pub config: Script,
    _dir: tempfile::TempDir,
    pub local: PathBuf,
}

impl FakeDsm {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().expect("temp dir");
        let local = dir.path().join("file.mkv");
        let fake = Self {
            server,
            get: Script::new(tasks(&[task(2, "")])),
            list: Script::new(tasks(&[])),
            create: Script::new(Reply::Data(json!({"task_id": [GID]}))),
            folder: Script::new(Reply::Data(json!({"folders": []}))),
            config: Script::new(Reply::Data(json!({"default_destination": "Media"}))),
            _dir: dir,
            local,
        };
        Mock::given(method("GET"))
            .and(path("/webapi/query.cgi"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "SYNO.API.Auth": {"path": "entry.cgi"},
                    "SYNO.DownloadStation2.Task": {"path": "DownloadStation/task.cgi"},
                    "SYNO.DownloadStation.Info": {"path": "DownloadStation/info.cgi"},
                    "SYNO.FileStation.CreateFolder": {"path": "entry.cgi"}
                }
            })))
            .mount(&fake.server)
            .await;
        Mock::given(method("POST"))
            .and(path("/webapi/entry.cgi"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"success": true, "data": {"sid": "sid"}})),
            )
            .mount(&fake.server)
            .await;
        let ok = Script::new(Reply::Data(json!({})));
        for (cgi, name, script) in [
            ("DownloadStation/task.cgi", "get", &fake.get),
            ("DownloadStation/task.cgi", "list", &fake.list),
            ("DownloadStation/task.cgi", "create", &fake.create),
            ("DownloadStation/task.cgi", "delete", &ok),
            ("DownloadStation/task.cgi", "pause", &ok),
            ("DownloadStation/task.cgi", "resume", &ok),
            ("entry.cgi", "create", &fake.folder),
            ("DownloadStation/info.cgi", "getconfig", &fake.config),
            ("DownloadStation/info.cgi", "setserverconfig", &ok),
        ] {
            let script = script.clone();
            Mock::given(method("GET"))
                .and(path(format!("/webapi/{cgi}")))
                .and(query_param("method", name))
                .respond_with(move |_: &Request| script.next())
                .mount(&fake.server)
                .await;
        }
        fake
    }
    /// Received requests as `<cgi> <method>` (`login` for sign-ins), with their queries.
    pub async fn calls(&self) -> Vec<(String, Vec<(String, String)>)> {
        let requests = self.server.received_requests().await.expect("recording");
        requests
            .iter()
            .filter(|request| request.url.path() != "/webapi/query.cgi")
            .map(|request| {
                let query: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
                let name = query
                    .iter()
                    .find(|(key, _)| key == "method")
                    .map_or("login".to_owned(), |(_, value)| value.clone());
                let cgi = request.url.path().trim_start_matches("/webapi/").to_owned();
                (format!("{cgi} {name}"), query)
            })
            .collect()
    }

    pub async fn count(&self, call: &str) -> usize {
        self.calls()
            .await
            .iter()
            .filter(|(name, _)| name == call)
            .count()
    }

    pub async fn task_calls(&self) -> Vec<String> {
        self.calls()
            .await
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name.starts_with("DownloadStation/task.cgi"))
            .collect()
    }
}

pub const TASK_GET: &str = "DownloadStation/task.cgi get";
pub const TASK_LIST: &str = "DownloadStation/task.cgi list";
pub const TASK_CREATE: &str = "DownloadStation/task.cgi create";
pub const FOLDER_CREATE: &str = "entry.cgi create";
