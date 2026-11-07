use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request},
};
use rdc_engine::testing::{ProviderCall, TestEngine};
use rdc_model::{TorrentStatus, settings::Settings};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;

use crate::{http::AppBuilder, logging};

struct Fixture {
    test: TestEngine,
    sessions: Sessions,
    disabled: Arc<AtomicBool>,
}

impl Fixture {
    async fn new(mut settings: Settings) -> Self {
        settings.general.tracker_enrichment_list = None;
        settings.provider.api_key = "test-token".to_owned();
        let test = TestEngine::with_settings(settings).await.unwrap();
        test.store
            .users()
            .create("alice", "Passw0rd!")
            .await
            .unwrap();
        let sessions = Sessions::open(test.store.clone(), &test.tempdir.path().join("key"))
            .await
            .unwrap();
        Self {
            test,
            sessions,
            disabled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn app(&self, base: &str) -> Router {
        let disabled = self.disabled.clone();
        let policy = AuthPolicy::new(
            self.sessions.clone(),
            Arc::new(move || disabled.load(Ordering::Relaxed)),
        );
        let (_, logs) = logging::subscriber(io::sink);
        AppBuilder::new(base, logs)
            .merge(
                router(self.test.engine.clone(), self.sessions.clone(), policy),
                PATHS,
            )
            .build(())
    }

    async fn login(&self, app: &Router, base: &str, prefix: &str) -> String {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("{base}{prefix}/auth/login"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("username=alice&password=Passw0rd%21"))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cookie.contains(&format!("Path={base}/"))
                || base.is_empty() && cookie.contains("Path=/")
        );
        cookie.split(';').next().unwrap().to_owned()
    }
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    content_type: Option<&str>,
    body: impl Into<Body>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(kind) = content_type {
        request = request.header(header::CONTENT_TYPE, kind);
    }
    app.clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap()
}

async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 30_000_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn json_response(response: Response) -> Value {
    serde_json::from_str(&text(response).await).unwrap()
}

const MAGNET1: &str = "magnet:?xt=urn:btih:1111111111111111111111111111111111111111&dn=Movie1";
const MAGNET2: &str = "magnet:?xt=urn:btih:2222222222222222222222222222222222222222&dn=Movie2";
const FILE: &[u8] =
    b"d4:infod6:lengthi1e4:name5:Movie12:piece lengthi1e6:pieces20:abcdefghijklmnopqrstee";

#[tokio::test]
async fn login_cookie_authentication_and_unknown_paths_obey_base_path() {
    let fixture = Fixture::new(Settings::default()).await;
    for base in ["", "/rd"] {
        let app = fixture.app(base);
        assert_eq!(
            text(
                call(
                    &app,
                    Method::GET,
                    &format!("{base}/version/api"),
                    None,
                    None,
                    Body::empty()
                )
                .await
            )
            .await,
            "20"
        );
        for prefix in ["/api/v2", "/qbittorrent/api/v2"] {
            let protected = format!("{base}{prefix}/app/preferences");
            assert_eq!(
                call(&app, Method::GET, &protected, None, None, Body::empty())
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            let invalid = call(
                &app,
                Method::POST,
                &format!("{base}{prefix}/auth/login"),
                None,
                Some("application/x-www-form-urlencoded"),
                "username=alice&password=nope",
            )
            .await;
            assert_eq!(text(invalid).await, "Fails.");
            let cookie = fixture.login(&app, base, prefix).await;
            let response = call(
                &app,
                Method::POST,
                &protected,
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await;
            let preferences = json_response(response).await;
            assert_eq!(preferences["web_ui_username"], "alice");
            assert!(
                preferences["save_path"]
                    .as_str()
                    .unwrap()
                    .ends_with(std::path::MAIN_SEPARATOR)
            );
            assert_eq!(preferences["scan_dirs"], json!({}));
            for (path, expected) in [("app/version", "v4.3.2"), ("app/webapiVersion", "2.7")] {
                assert_eq!(
                    text(
                        call(
                            &app,
                            Method::GET,
                            &format!("{base}{prefix}/{path}"),
                            None,
                            None,
                            Body::empty()
                        )
                        .await
                    )
                    .await,
                    expected
                );
            }
            assert_eq!(
                json_response(
                    call(
                        &app,
                        Method::GET,
                        &format!("{base}{prefix}/app/buildInfo"),
                        None,
                        None,
                        Body::empty()
                    )
                    .await
                )
                .await["bitness"],
                64
            );
            let logout = call(
                &app,
                Method::POST,
                &format!("{base}{prefix}/auth/logout"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await;
            assert_eq!(logout.status(), StatusCode::OK);
            assert!(logout.headers().contains_key(header::SET_COOKIE));
        }
        assert_eq!(
            call(
                &app,
                Method::GET,
                &format!("{base}/qbittorrent/api/v2/nonexistent"),
                None,
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &app,
                Method::GET,
                &format!("{base}/QBITTORRENT/other"),
                None,
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    fixture.disabled.store(true, Ordering::Relaxed);
    let app = fixture.app("");
    let login = call(
        &app,
        Method::POST,
        "/api/v2/auth/login",
        None,
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    assert!(!login.headers().contains_key(header::SET_COOKIE));
    assert_eq!(text(login).await, "Ok.");
    assert_eq!(
        call(
            &app,
            Method::GET,
            "/api/v2/app/preferences",
            None,
            None,
            Body::empty()
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn additions_filters_models_mutations_and_deletion_work_for_both_prefixes() {
    for prefix in ["/api/v2", "/qbittorrent/api/v2"] {
        let mut settings = Settings::default();
        settings.integrations.default.category = Some("default".to_owned());
        let fixture = Fixture::new(settings).await;
        let app = fixture.app("");
        let cookie = fixture.login(&app, "", prefix).await;
        let add = call(
            &app,
            Method::POST,
            &format!("{prefix}/torrents/add"),
            Some(&cookie),
            Some("application/x-www-form-urlencoded"),
            format!(
                "urls={}%0D%0A{}&priority=7",
                url::form_urlencoded::byte_serialize(MAGNET1.as_bytes()).collect::<String>(),
                url::form_urlencoded::byte_serialize(MAGNET2.as_bytes()).collect::<String>()
            ),
        )
        .await;
        assert_eq!(text(add).await, "Ok.");
        let torrents = fixture.test.engine.torrents().await.unwrap();
        assert_eq!(torrents.len(), 2);
        assert!(
            torrents
                .iter()
                .all(|torrent| torrent.category.as_deref() == Some("default")
                    && torrent.priority == Some(7))
        );
        let hash = &torrents[0].hash;
        let filtered = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/info?FILTER=all&CATEGORY=default&HASHES={hash}"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(filtered.as_array().unwrap().len(), 1);
        assert_eq!(filtered[0]["hash"], hash.as_str());
        assert_eq!(filtered[0]["category"], "default");
        assert!(filtered[0]["progress"].is_number());
        let wrong = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/info?category=DEFAULT"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(wrong, json!([]));
        assert_eq!(
            json_response(
                call(
                    &app,
                    Method::GET,
                    &format!("{prefix}/torrents/count?filter=all"),
                    Some(&cookie),
                    None,
                    Body::empty()
                )
                .await
            )
            .await,
            2
        );
        let mut seeded = fixture
            .test
            .engine
            .torrent_by_hash(hash)
            .await
            .unwrap()
            .unwrap();
        seeded.set_files(&[
            rdc_model::RemoteFile {
                id: 7,
                path: "/good.mkv".to_owned(),
                bytes: 100,
                selected: true,
                download_link: None,
            },
            rdc_model::RemoteFile {
                id: 9,
                path: "/skip.exe".to_owned(),
                bytes: 200,
                selected: false,
                download_link: None,
            },
        ]);
        fixture
            .test
            .store
            .torrents()
            .update_rd_data(&seeded)
            .await
            .unwrap();
        let files = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/files?hash={hash}"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(files[0]["index"], 0);
        assert_eq!(files[0]["name"], "/good.mkv");
        assert_eq!(files[0]["priority"], 1);
        assert_eq!(files[1]["priority"], 0);
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/filePrio"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                format!("hash={hash}&id=1|bad&priority=1")
            )
            .await
            .status(),
            StatusCode::OK
        );
        let files = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/files?hash={hash}"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(files[1]["priority"], 1);
        let properties = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/properties?hash={hash}"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(properties["total_downloaded"], 0);
        assert_eq!(properties["created_by"], "rd-client");
        assert_eq!(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/files?hash=unknown"),
                Some(&cookie),
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        for endpoint in ["pause", "resume", "topPrio", "setCategory"] {
            let body = format!("hashes={hash}&category=changed");
            let response = call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/{endpoint}"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                body,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{endpoint}");
        }
        assert_eq!(
            fixture
                .test
                .engine
                .torrent_by_hash(hash)
                .await
                .unwrap()
                .unwrap()
                .category
                .as_deref(),
            Some("changed")
        );
        let categories = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/categories"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert!(categories["changed"]["savePath"].is_string());
        let create = call(
            &app,
            Method::POST,
            &format!("{prefix}/torrents/createCategory"),
            Some(&cookie),
            Some("application/x-www-form-urlencoded"),
            "category=Shows",
        )
        .await;
        assert_eq!(create.status(), StatusCode::OK);
        let categories = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/categories"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(categories["Shows"]["name"], "Shows");
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/removeCategories"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                "categories=Shows"
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_response(
                call(
                    &app,
                    Method::GET,
                    &format!("{prefix}/torrents/tags"),
                    Some(&cookie),
                    None,
                    Body::empty()
                )
                .await
            )
            .await,
            json!([])
        );
        assert!(
            json_response(
                call(
                    &app,
                    Method::GET,
                    &format!("{prefix}/sync/maindata"),
                    Some(&cookie),
                    None,
                    Body::empty()
                )
                .await
            )
            .await["torrents"][hash]
                .is_object()
        );
        let transfer = json_response(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/transfer/info"),
                Some(&cookie),
                None,
                Body::empty(),
            )
            .await,
        )
        .await;
        assert_eq!(
            transfer["dl_info_speed"],
            fixture.test.engine.active_download_speed_sum()
        );
        assert_eq!(
            transfer["dl_info_data"],
            fixture.test.engine.session_downloaded_bytes()
        );
        let local_root = &fixture.test.engine.settings().download_client.download_path;
        let removed = std::path::Path::new(local_root)
            .join("changed")
            .join(torrents[0].rd_name.as_deref().unwrap());
        let kept = std::path::Path::new(local_root)
            .join("default")
            .join(torrents[1].rd_name.as_deref().unwrap());
        std::fs::create_dir_all(&removed).unwrap();
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(removed.join("payload"), b"data").unwrap();
        std::fs::write(kept.join("payload"), b"data").unwrap();
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/delete"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                format!("hashes={hash}&deleteFiles=true")
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(!removed.exists());
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/delete"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                format!("hashes={}&deleteFiles=false", torrents[1].hash)
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(kept.join("payload").exists());
        assert!(
            fixture
                .test
                .engine
                .torrent_by_hash(hash)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .test
                .fake
                .calls()
                .iter()
                .any(|call| matches!(call, ProviderCall::Delete { .. }))
        );
    }
}

#[tokio::test]
async fn uploaded_files_use_priority_and_default_category() {
    let mut settings = Settings::default();
    settings.integrations.default.category = Some("media".to_owned());
    let fixture = Fixture::new(settings).await;
    let app = fixture.app("");
    let cookie = fixture.login(&app, "", "/api/v2").await;
    let mut multipart = b"--boundary\r\nContent-Disposition: form-data; name=\"priority\"\r\n\r\n5\r\n--boundary\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"movie.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\n".to_vec();
    multipart.extend_from_slice(FILE);
    multipart.extend_from_slice(b"\r\n--boundary--\r\n");
    let response = call(
        &app,
        Method::POST,
        "/api/v2/torrents/add",
        Some(&cookie),
        Some("multipart/form-data; boundary=boundary"),
        multipart,
    )
    .await;
    assert_eq!(text(response).await, "");
    let torrent = fixture.test.engine.torrents().await.unwrap().pop().unwrap();
    assert_eq!(torrent.priority, Some(5));
    assert_eq!(torrent.category.as_deref(), Some("media"));
    assert_eq!(torrent.rd_status, Some(TorrentStatus::Queued));
}

#[tokio::test]
async fn queued_add_returns_ok_after_bounded_provider_wait() {
    let fixture = Fixture::new(Settings::default()).await;
    fixture
        .test
        .engine
        .update_setting("Provider:ApiKey", Some(""))
        .await
        .unwrap();
    let app = fixture.app("");
    let cookie = fixture.login(&app, "", "/api/v2").await;
    let response = call(
        &app,
        Method::POST,
        "/api/v2/torrents/add",
        Some(&cookie),
        Some("application/x-www-form-urlencoded"),
        format!(
            "urls={}",
            url::form_urlencoded::byte_serialize(MAGNET1.as_bytes()).collect::<String>()
        ),
    )
    .await;
    assert_eq!(text(response).await, "Ok.");
    let torrent = fixture.test.engine.torrents().await.unwrap().pop().unwrap();
    assert!(torrent.rd_id.is_none());
}

#[tokio::test]
async fn http_torrent_url_uses_query_binding_and_requested_priority() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FILE))
        .mount(&server)
        .await;
    let fixture = Fixture::new(Settings::default()).await;
    let app = fixture.app("");
    let cookie = fixture.login(&app, "", "/api/v2").await;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("urls", &format!("{}/movie", server.uri()))
        .append_pair("priority", "3")
        .finish();
    let response = call(
        &app,
        Method::GET,
        &format!("/api/v2/torrents/add?{query}"),
        Some(&cookie),
        None,
        Body::empty(),
    )
    .await;
    assert_eq!(text(response).await, "Ok.");
    let torrent = fixture.test.engine.torrents().await.unwrap().pop().unwrap();
    assert_eq!(torrent.priority, Some(3));
    assert!(torrent.is_file);
}

#[tokio::test]
async fn auxiliary_endpoints_and_binding_errors_keep_expected_statuses() {
    let fixture = Fixture::new(Settings::default()).await;
    let app = fixture.app("");
    for prefix in ["/api/v2", "/qbittorrent/api/v2"] {
        let cookie = fixture.login(&app, "", prefix).await;
        for suffix in [
            "app/shutdown",
            "app/setPreferences",
            "torrents/setShareLimits",
            "torrents/createTags",
            "torrents/setForcestart",
        ] {
            for method in [Method::GET, Method::POST] {
                assert_eq!(
                    call(
                        &app,
                        method,
                        &format!("{prefix}/{suffix}"),
                        Some(&cookie),
                        None,
                        Body::empty()
                    )
                    .await
                    .status(),
                    StatusCode::OK,
                    "{suffix}"
                );
            }
        }
        assert_eq!(
            text(
                call(
                    &app,
                    Method::GET,
                    &format!("{prefix}/app/defaultSavePath"),
                    Some(&cookie),
                    None,
                    Body::empty()
                )
                .await
            )
            .await,
            models::save_path(&fixture.test.engine.settings())
        );
        let unknown = json_response(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/trackers"),
                Some(&cookie),
                Some("application/x-www-form-urlencoded"),
                "hash=missing",
            )
            .await,
        )
        .await;
        assert_eq!(unknown, json!([]));
        let invalid = [
            ("torrents/pause", "", StatusCode::BAD_REQUEST),
            (
                "torrents/filePrio",
                "hash=missing&id=abc&priority=1",
                StatusCode::BAD_REQUEST,
            ),
            (
                "torrents/createCategory",
                "category=",
                StatusCode::BAD_REQUEST,
            ),
            ("torrents/add", "urls=", StatusCode::BAD_REQUEST),
        ];
        for (suffix, form, status) in invalid {
            assert_eq!(
                call(
                    &app,
                    Method::POST,
                    &format!("{prefix}/{suffix}"),
                    Some(&cookie),
                    Some("application/x-www-form-urlencoded"),
                    form
                )
                .await
                .status(),
                status,
                "{suffix}"
            );
        }
        assert_eq!(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/createCategory?category=ignored"),
                Some(&cookie),
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &app,
                Method::GET,
                &format!("{prefix}/torrents/add"),
                Some(&cookie),
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{prefix}/torrents/add"),
                Some(&cookie),
                None,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

#[tokio::test]
async fn failed_provider_state_returns_fails_after_dequeue() {
    let fixture = Fixture::new(Settings::default()).await;
    let torrent = fixture
        .test
        .engine
        .add_magnet(MAGNET1, TorrentTemplate::default())
        .await
        .unwrap();
    fixture
        .test
        .fake
        .set_status(torrent.rd_id.unwrap(), TorrentStatus::Error);
    let app = fixture.app("");
    let cookie = fixture.login(&app, "", "/api/v2").await;
    let response = call(
        &app,
        Method::POST,
        "/api/v2/torrents/add",
        Some(&cookie),
        Some("application/x-www-form-urlencoded"),
        format!(
            "urls={}",
            url::form_urlencoded::byte_serialize(MAGNET1.as_bytes()).collect::<String>()
        ),
    )
    .await;
    assert_eq!(text(response).await, "Fails.");
}

#[tokio::test]
async fn repeated_failed_logins_lock_the_name_out_even_with_the_right_password() {
    let fixture = Fixture::new(Settings::default()).await;
    let app = fixture.app("");
    let attempt = |password: &'static str| {
        call(
            &app,
            Method::POST,
            "/api/v2/auth/login",
            None,
            Some("application/x-www-form-urlencoded"),
            format!("username=alice&password={password}"),
        )
    };
    for _ in 0..5 {
        assert_eq!(text(attempt("guess").await).await, "Fails.");
    }
    let locked = attempt("Passw0rd%21").await;
    assert_eq!(locked.status(), StatusCode::FORBIDDEN);
    assert!(!locked.headers().contains_key(header::SET_COOKIE));
}
