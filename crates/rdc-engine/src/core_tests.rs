//! Settings, construction and profile integration tests.

use crate::{
    EngineOptions,
    testing::{FakeDebridProvider, TestEngine},
};
use jiff::Timestamp;
use rdc_debrid::{DebridProvider, DebridUser};
use rdc_model::{Provider, settings::Settings};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn settings_listener_receives_startup_and_live_log_level_and_secrets() {
    let fake = FakeDebridProvider::default();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let notifications = observed.clone();
    let options = EngineOptions {
        settings_listener: Some(Arc::new(move |settings| {
            notifications.lock().expect("notifications").push(settings);
        })),
        ..fake.options()
    };
    let test = TestEngine::with_options(Settings::default(), fake, options)
        .await
        .expect("engine");
    let old = test.engine.settings();
    test.engine
        .update_settings(vec![
            ("General:LogLevel".to_owned(), Some("1".to_owned())),
            (
                "Provider:ApiKey".to_owned(),
                Some("secret-changed".to_owned()),
            ),
        ])
        .await
        .expect("settings update");
    {
        let observed = observed.lock().expect("notifications");
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[1].general.log_level.to_string(), "Debug");
        assert_eq!(observed[1].provider.api_key, "secret-changed");
        assert!(
            old.provider.api_key.is_empty(),
            "previous snapshot remains immutable"
        );
    }
    test.engine.shutdown().await;
}

#[tokio::test]
async fn provider_rebuild_uses_new_config_only_for_provider_changes() {
    let fake = FakeDebridProvider::default();
    let builds = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let count = builds.clone();
    let configs = observed.clone();
    let provider = fake.clone();
    let options = EngineOptions {
        provider_factory: Arc::new(move |kind, config, _http| {
            count.fetch_add(1, Ordering::Relaxed);
            configs.lock().expect("configs").push(config.clone());
            Ok(Arc::new(provider.with_kind(kind)) as Arc<dyn DebridProvider>)
        }),
        ..fake.options()
    };
    let test = TestEngine::with_options(Settings::default(), fake, options)
        .await
        .expect("engine");
    test.engine
        .update_setting("General:DownloadLimit", Some("5"))
        .await
        .expect("nonprovider setting");
    assert_eq!(builds.load(Ordering::Relaxed), 1);
    test.engine
        .update_settings(vec![
            ("Provider:Provider".to_owned(), Some("3".to_owned())),
            ("Provider:ApiKey".to_owned(), Some("changed".to_owned())),
            ("Provider:Timeout".to_owned(), Some("27".to_owned())),
        ])
        .await
        .expect("provider settings");
    assert_eq!(builds.load(Ordering::Relaxed), 2);
    assert_eq!(test.engine.provider().kind(), Provider::TorBox);
    {
        let configs = observed.lock().expect("configs");
        assert_eq!(configs[1].api_key, "changed");
        assert_eq!(configs[1].timeout.as_secs(), 27);
    }
    test.engine.shutdown().await;
}

#[tokio::test]
async fn settings_invalid_values_keep_cache_unknown_keys_ignore_and_first_duplicate_wins() {
    let test = TestEngine::new().await.expect("engine");
    test.engine
        .update_setting("General:DownloadLimit", Some("9"))
        .await
        .expect("valid");
    test.engine
        .update_settings(vec![
            (
                "General:DownloadLimit".to_owned(),
                Some("invalid".to_owned()),
            ),
            ("Unknown:Setting".to_owned(), Some("oops".to_owned())),
            ("Watch:Interval".to_owned(), Some("4".to_owned())),
            ("Watch:Interval".to_owned(), Some("70".to_owned())),
        ])
        .await
        .expect("invalid stored value tolerated");
    assert_eq!(test.engine.settings().general.download_limit, 9);
    assert_eq!(test.engine.settings().watch.interval, 4);
    test.engine
        .update_setting("General:DownloadLimit", None)
        .await
        .expect("null");
    assert_eq!(test.engine.settings().general.download_limit, 0);
    test.engine.shutdown().await;
}

#[tokio::test]
async fn internal_settings_watch_applies_normalized_speed_and_raw_connection_controls() {
    let test = TestEngine::new().await.expect("engine");
    let mut config = test.engine.internal_download_config();
    test.engine
        .update_settings(vec![
            ("DownloadClient:MaxSpeed".to_owned(), Some("3".to_owned())),
            (
                "DownloadClient:ParallelCount".to_owned(),
                Some("0".to_owned()),
            ),
            (
                "DownloadClient:BufferSize".to_owned(),
                Some("8192".to_owned()),
            ),
            (
                "DownloadClient:BindToSpecificIp".to_owned(),
                Some("True".to_owned()),
            ),
            (
                "DownloadClient:BindIpAddress".to_owned(),
                Some("127.0.0.1".to_owned()),
            ),
        ])
        .await
        .expect("live config");
    config.changed().await.expect("notification");
    assert_eq!(config.borrow().max_speed, 3 * 1024 * 1024);
    assert_eq!(config.borrow().parallel_connections, 0);
    assert_eq!(config.borrow().buffer_size, 8192);
    assert_eq!(config.borrow().bind_address.as_deref(), Some("127.0.0.1"));
    test.engine.shutdown().await;
}

#[tokio::test]
async fn profile_combines_account_expiration_with_premium_flag() {
    let test = TestEngine::new().await.expect("engine");
    test.fake.set_user(DebridUser {
        username: Some("account".to_owned()),
        expiration: Some(Timestamp::now()),
        premium: false,
    });
    let profile = test.engine.profile().await.expect("profile");
    assert!(profile.premium);
    assert_eq!(profile.user_name.as_deref(), Some("account"));
    assert_eq!(
        profile.current_version,
        concat!("v", env!("CARGO_PKG_VERSION"))
    );
    assert!(!profile.is_insecure);
    test.engine.shutdown().await;
}
