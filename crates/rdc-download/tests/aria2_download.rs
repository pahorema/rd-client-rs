//! Behaviour of the aria2 engine against a scripted JSON-RPC server.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdc_download::{Aria2Config, Aria2Download, Aria2Request, CANCELLED_MESSAGE, DownloadEngine};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const URL: &str = "https://host/file.mkv";

/// Answer of one RPC method: a result, or a JSON-RPC error when `None`.
type Answer = Arc<Mutex<Option<Value>>>;

struct FakeAria2 {
    server: MockServer,
    status: Answer,
    added: Answer,
    all: Answer,
    removed: Answer,
    _dir: tempfile::TempDir,
    local: PathBuf,
}

fn status(state: &str) -> Value {
    json!({
        "gid": "gid1",
        "status": state,
        "totalLength": "100",
        "completedLength": "40",
        "downloadSpeed": "10",
        "files": [{"path": "/remote/Movie/file.mkv", "uris": [{"uri": URL, "status": "used"}]}]
    })
}

fn answer(value: Value) -> Answer {
    Arc::new(Mutex::new(Some(value)))
}

fn method_of(request: &Request) -> String {
    let body: Value = serde_json::from_slice(&request.body).expect("JSON-RPC body");
    body["method"].as_str().unwrap_or_default().to_owned()
}

impl FakeAria2 {
    async fn start() -> Self {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().expect("temp dir");
        let local = dir.path().join("Movie").join("file.mkv");
        let fake = Self {
            server,
            status: answer(status("active")),
            added: answer(json!("gid1")),
            all: answer(json!([[[]], [[]], [[]]])),
            removed: answer(json!("gid1")),
            _dir: dir,
            local,
        };
        for (rpc, value) in [
            ("aria2.tellStatus", &fake.status),
            ("aria2.addUri", &fake.added),
            ("system.multicall", &fake.all),
            ("aria2.forceRemove", &fake.removed),
            ("aria2.removeDownloadResult", &fake.removed),
            ("aria2.pause", &fake.removed),
            ("aria2.unpause", &fake.removed),
        ] {
            let value = Arc::clone(value);
            Mock::given(move |request: &Request| method_of(request) == rpc)
                .respond_with(move |_: &Request| {
                    let body = match value.lock().expect("answer").clone() {
                        Some(result) => json!({"jsonrpc": "2.0", "id": "x", "result": result}),
                        None => json!({"jsonrpc": "2.0", "id": "x", "error": {"code": 1, "message": "boom"}}),
                    };
                    ResponseTemplate::new(200).set_body_json(body)
                })
                .mount(&fake.server)
                .await;
        }
        fake
    }

    fn config(&self) -> Aria2Config {
        Aria2Config {
            url: format!("{}/jsonrpc", self.server.uri()),
            secret: Some("secret".to_owned()),
            download_path: Some("/remote".to_owned()),
        }
    }

    fn request(&self) -> Aria2Request {
        Aria2Request {
            url: URL.to_owned(),
            file_path: self.local.clone(),
            relative_path: "Movie/file.mkv".to_owned(),
            category: None,
            gid: None,
        }
    }

    fn engine(&self, request: Aria2Request) -> Aria2Download {
        Aria2Download::new(request, self.config()).with_retry_step(Duration::from_millis(1))
    }

    fn set(answer: &Answer, value: Option<Value>) {
        *answer.lock().expect("answer") = value;
    }

    /// Parameters of every call of `rpc`, in order.
    async fn calls(&self, rpc: &str) -> Vec<Value> {
        let requests = self.server.received_requests().await.expect("recording");
        requests
            .iter()
            .filter(|request| method_of(request) == rpc)
            .map(|request| {
                let body: Value = serde_json::from_slice(&request.body).expect("body");
                body["params"].clone()
            })
            .collect()
    }
}

#[tokio::test]
async fn adds_a_new_download_below_the_remote_root() {
    let fake = FakeAria2::start().await;
    let request = Aria2Request {
        category: Some("movies".to_owned()),
        ..fake.request()
    };
    let gid = fake.engine(request).start().await.expect("start");
    assert_eq!(gid, "gid1");
    assert_eq!(
        fake.calls("aria2.addUri").await,
        vec![json!([
            "token:secret",
            [URL],
            {"dir": "/remote/movies/Movie", "out": "file.mkv"}
        ])]
    );
    assert_eq!(
        fake.calls("aria2.tellStatus").await,
        vec![json!(["token:secret", "gid1"])]
    );
}

#[tokio::test]
async fn without_a_remote_root_aria2_saves_to_the_local_path() {
    let fake = FakeAria2::start().await;
    let config = Aria2Config {
        download_path: Some("  ".to_owned()),
        ..fake.config()
    };
    Aria2Download::new(fake.request(), config)
        .start()
        .await
        .expect("start");
    let dir = fake.local.parent().expect("parent").display().to_string();
    assert_eq!(
        fake.calls("aria2.addUri").await[0][2],
        json!({"dir": dir, "out": "file.mkv"})
    );
}

#[tokio::test]
async fn a_link_aria2_already_downloads_is_refused() {
    let fake = FakeAria2::start().await;
    FakeAria2::set(&fake.all, Some(json!([[[status("complete")]], [[]], [[]]])));
    let engine = fake.engine(fake.request());
    let error = engine.start().await.expect_err("already added");
    let message = format!("The download link {URL} has already been added to Aria2");
    assert_eq!(error.message(), message);
    assert_eq!(engine.completion().outcome(), Some(Err(message)));
    assert_eq!(fake.calls("aria2.addUri").await, Vec::<Value>::new());
}

#[tokio::test]
async fn a_known_gid_is_adopted_without_adding() {
    let fake = FakeAria2::start().await;
    let request = Aria2Request {
        gid: Some("gid9".to_owned()),
        ..fake.request()
    };
    let gid = fake.engine(request).start().await.expect("start");
    assert_eq!(gid, "gid9");
    assert_eq!(fake.calls("aria2.addUri").await, Vec::<Value>::new());
    assert_eq!(fake.calls("system.multicall").await, Vec::<Value>::new());
}

#[tokio::test]
async fn a_forgotten_gid_is_added_again() {
    let fake = FakeAria2::start().await;
    FakeAria2::set(&fake.status, None);
    let request = Aria2Request {
        gid: Some("gid9".to_owned()),
        ..fake.request()
    };
    let engine = fake.engine(request);
    let start = tokio::spawn(async move { engine.start().await });
    // The re-added download only becomes known on the second verification.
    while fake.calls("aria2.addUri").await.is_empty() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    FakeAria2::set(&fake.status, Some(status("active")));
    let gid = start.await.expect("task").expect("start");
    assert_eq!(gid, "gid1");
}

#[tokio::test]
async fn adding_gives_up_after_six_attempts() {
    let fake = FakeAria2::start().await;
    FakeAria2::set(&fake.added, None);
    let engine = fake.engine(fake.request());
    let error = engine.start().await.expect_err("aria2 refuses");
    assert_eq!(error.message(), "aria2 error 1: boom");
    assert_eq!(fake.calls("aria2.addUri").await.len(), 6);
    assert_eq!(
        engine.completion().outcome(),
        Some(Err("aria2 error 1: boom".to_owned()))
    );
}

#[tokio::test]
async fn an_rpc_url_without_jsonrpc_fails_to_start() {
    let fake = FakeAria2::start().await;
    let config = Aria2Config {
        url: fake.server.uri(),
        ..fake.config()
    };
    let error = Aria2Download::new(fake.request(), config)
        .start()
        .await
        .expect_err("invalid url");
    assert_eq!(error.message(), "The URL must end with /jsonrpc");
}

#[tokio::test]
async fn pause_and_resume_reach_aria2() {
    let fake = FakeAria2::start().await;
    let engine = fake.engine(fake.request());
    engine.pause().await;
    engine.start().await.expect("start");
    engine.pause().await;
    engine.resume().await;
    assert_eq!(
        fake.calls("aria2.pause").await,
        vec![json!(["token:secret", "gid1"])]
    );
    assert_eq!(
        fake.calls("aria2.unpause").await,
        vec![json!(["token:secret", "gid1"])]
    );
}

#[tokio::test]
async fn cancel_removes_the_download_even_when_aria2_fails() {
    let fake = FakeAria2::start().await;
    let engine = fake.engine(fake.request());
    engine.start().await.expect("start");
    FakeAria2::set(&fake.removed, None);
    engine.cancel().await;
    engine.cancel().await;
    assert_eq!(
        engine.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    assert_eq!(fake.calls("aria2.forceRemove").await.len(), 2);
    assert_eq!(fake.calls("aria2.removeDownloadResult").await.len(), 2);
}

#[tokio::test]
async fn cancel_before_start_completes_without_aria2() {
    let fake = FakeAria2::start().await;
    let engine = fake.engine(fake.request());
    engine.cancel().await;
    assert_eq!(
        engine.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    let error = engine.start().await.expect_err("cancelled");
    assert_eq!(error.message(), CANCELLED_MESSAGE);
    assert!(
        fake.server
            .received_requests()
            .await
            .expect("recording")
            .is_empty()
    );
}
