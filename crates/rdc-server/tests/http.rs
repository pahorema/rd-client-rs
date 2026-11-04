use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Request, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use rdc_server::{
    config::{Config, healthcheck},
    http::{ApiError, AppBuilder, RequestBasePath},
    logging,
};
use tower::ServiceExt;

fn builder<S: Clone + Send + Sync + 'static>(base: &str) -> AppBuilder<S> {
    let (_, logs) = logging::subscriber(std::io::sink);
    AppBuilder::new(base, logs)
}

async fn text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("test response body is readable")
            .to_vec(),
    )
    .expect("test response is UTF-8")
}

async fn request(app: Router, path: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("valid test request"),
    )
    .await
    .expect("router is infallible")
}

#[tokio::test]
async fn root_and_normalized_base_prefix_share_handlers_and_error_bodies() {
    let app = builder("//downloads///client/")
        .route(
            "/Api/Example",
            get(|| async { ApiError::new(StatusCode::BAD_REQUEST, "details are preserved") }),
        )
        .route(
            "/api/mount",
            get(|axum::Extension(base): axum::Extension<RequestBasePath>| async move { base.0 }),
        )
        .build(());
    for path in ["/api/example", "/downloads/client/API/EXAMPLE"] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(text(response).await, "details are preserved");
    }
    assert_eq!(text(request(app.clone(), "/api/mount").await).await, "");
    assert_eq!(
        text(request(app.clone(), "/downloads/client/api/mount").await).await,
        "/downloads/client"
    );
    let response = request(app.clone(), "/downloads/client?value=Case%2B").await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/downloads/client/?value=Case%2B"
    );
    assert_eq!(
        request(app, "/downloads/clientx/api/example")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn api_paths_ignore_case_but_parameters_and_queries_keep_case() {
    let app = builder("")
        .route(
            "/Api/Items/{name}",
            get(
                |Path(name): Path<String>, uri: axum::http::Uri| async move {
                    format!("{name}|{}", uri.query().unwrap_or(""))
                },
            ),
        )
        .route(
            "/qbittorrent/api/v2/Example/{id}",
            get(|Path(id): Path<String>| async move { id }),
        )
        .route("/api/items/Fixed", get(|| async { "static route" }))
        .build(());
    assert_eq!(
        text(request(app.clone(), "/API/ITEMS/MiXeD?token=AbC%2FDef").await).await,
        "MiXeD|token=AbC%2FDef"
    );
    assert_eq!(
        text(request(app.clone(), "/QBITTORRENT/API/V2/EXAMPLE/CaseSensitive").await).await,
        "CaseSensitive"
    );
    assert_eq!(
        text(request(app, "/API/ITEMS/FIXED").await).await,
        "static route"
    );
}

#[tokio::test]
async fn unauthorized_api_responses_become_forbidden_without_losing_text() {
    let app = builder("/base")
        .route(
            "/api/private",
            get(|| async { (StatusCode::UNAUTHORIZED, "not authenticated") }),
        )
        .route("/public", get(|| async { StatusCode::UNAUTHORIZED }))
        .route("/api/setup", get(|| async { StatusCode::PAYMENT_REQUIRED }))
        .build(());
    for path in ["/api/private", "/base/api/private"] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(text(response).await, "not authenticated");
    }
    assert_eq!(
        request(app.clone(), "/public").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(app, "/api/setup").await.status(),
        StatusCode::PAYMENT_REQUIRED
    );
}

#[tokio::test]
async fn unknown_api_paths_are_empty_not_found_and_wrong_methods_are_405() {
    let app = builder("/base")
        .route("/api/known", get(|| async {}))
        .build(());
    for path in [
        "/api/nope",
        "/API/nope",
        "/qbittorrent/api/v2/nope",
        "/QBITTORRENT/nope",
        "/base/api/nope",
    ] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(text(response).await, "");
    }
    let response = app
        .oneshot(Request::post("/API/KNOWN").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn index_and_ui_missing_page_are_healthy_and_not_cached() {
    let app = builder("/base").build(());
    for path in ["/", "/index.html", "/base/", "/base/index.html"] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let body = text(response).await;
        assert!(body.contains("UI is missing") || body.contains("<html"));
    }
    assert_eq!(
        request(app, "/missing.js").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn magnet_redirect_preserves_the_encoded_query_under_both_mounts() {
    let app = builder("/base").build(());
    for path in [
        "/add?magnet=magnet%3A%3Fxt%3Durn%3Abtih%3AAbC%26dn%3Dx%2By",
        "/base/add?magnet=magnet%3A%3Fxt%3Durn%3Abtih%3AAbC%26dn%3Dx%2By",
    ] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "./#/add?magnet=magnet%3A%3Fxt%3Durn%3Abtih%3AAbC%26dn%3Dx%2By"
        );
    }
}

#[tokio::test]
async fn handler_errors_and_panics_return_plain_text_500() {
    let app = builder("/base")
        .route(
            "/api/error",
            get(|| async { Err::<(), _>(ApiError::from(anyhow::anyhow!("handler error"))) }),
        )
        .route(
            "/api/panic",
            get(|| async {
                panic!("handler panic");
                #[expect(
                    unreachable_code,
                    reason = "give the panicking test handler a concrete response type"
                )]
                ().into_response()
            }),
        )
        .build(());
    for (path, expected) in [
        ("/base/api/error", "handler error"),
        ("/api/panic", "handler panic"),
    ] {
        let response = request(app.clone(), path).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(text(response).await, expected);
    }
}

#[tokio::test]
async fn routers_can_merge_and_receive_arbitrary_shared_state() {
    let routes =
        Router::new().route(
            "/api/state/{id}",
            get(
                |State(state): State<String>, Path(id): Path<String>| async move {
                    format!("{state}/{id}")
                },
            ),
        );
    let app = builder("")
        .merge(routes, &["/api/state/{id}"])
        .build("shared".to_owned());
    assert_eq!(
        text(request(app, "/API/STATE/AbC").await).await,
        "shared/AbC"
    );
}

#[tokio::test]
async fn healthcheck_probes_a_running_server_and_rejects_connection_failures() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = builder("/health").build(());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = Config::parse_from([
        "rd-client",
        "--port",
        &port.to_string(),
        "--base-path",
        "/health",
    ]);
    healthcheck(&config).await.unwrap();
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rd-client"))
        .args([
            "healthcheck",
            "--port",
            &port.to_string(),
            "--base-path",
            "/health",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    task.abort();
    let _ = task.await;
    assert!(healthcheck(&config).await.is_err());
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rd-client"))
        .args(["healthcheck", "--port", &port.to_string()])
        .output()
        .await
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
}
