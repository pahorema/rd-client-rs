use rdc_synology::ServerConfig;
use serde_json::json;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer};

use crate::common::{SID, logged_in, ok, ok_without_data};

fn info_call(info_method: &str) -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path("/webapi/DownloadStation/info.cgi"))
        .and(query_param("api", "SYNO.DownloadStation.Info"))
        .and(query_param("version", "2"))
        .and(query_param("method", info_method))
        .and(query_param("session", "DownloadStation"))
        .and(query_param("_sid", SID))
}

#[tokio::test]
async fn server_config_parses_the_settings() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    info_call("getconfig")
        .respond_with(ok(&json!({
            "bt_max_download": 0,
            "bt_max_upload": 100,
            "emule_enabled": false,
            "emul_max_download": 0,
            "emul_max_upload": 20,
            "ftp_max_download": 0,
            "http_max_download": 0,
            "nzb_max_download": 0,
            "unzip_service_enabled": true,
            "default_destination": "Media/Downloads",
            "emul_default_destination": null,
            "xunlei_enabled": false
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = session.server_config().await.unwrap();

    assert_eq!(
        config,
        ServerConfig {
            bt_max_download: Some(0),
            bt_max_upload: Some(100),
            emule_enabled: Some(false),
            emul_max_download: Some(0),
            emul_max_upload: Some(20),
            ftp_max_download: Some(0),
            http_max_download: Some(0),
            nzb_max_download: Some(0),
            unzip_service_enabled: Some(true),
            default_destination: Some("Media/Downloads".to_owned()),
            emul_default_destination: None,
        }
    );
}

#[tokio::test]
async fn server_config_without_destination_reports_none() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    info_call("getconfig")
        .respond_with(ok(
            &json!({"bt_max_download": 0, "default_destination": ""}),
        ))
        .mount(&server)
        .await;

    let config = session.server_config().await.unwrap();

    assert_eq!(config.default_destination.as_deref(), Some(""));
    assert_eq!(config.bt_max_upload, None);
}

#[tokio::test]
async fn set_server_config_resends_reported_settings_with_the_new_destination() {
    let server = MockServer::start().await;
    let session = logged_in(&server).await;
    info_call("setserverconfig")
        .and(query_param("bt_max_download", "0"))
        .and(query_param("bt_max_upload", "100"))
        .and(query_param("emule_enabled", "false"))
        .and(query_param("unzip_service_enabled", "true"))
        .and(query_param("default_destination", "Media/Downloads"))
        .and(query_param_is_missing("emul_default_destination"))
        .and(query_param_is_missing("http_max_download"))
        .respond_with(ok_without_data())
        .expect(1)
        .mount(&server)
        .await;
    let config = ServerConfig {
        bt_max_download: Some(0),
        bt_max_upload: Some(100),
        emule_enabled: Some(false),
        unzip_service_enabled: Some(true),
        default_destination: Some("Media/Downloads".to_owned()),
        ..ServerConfig::default()
    };

    session.set_server_config(&config).await.unwrap();
}
