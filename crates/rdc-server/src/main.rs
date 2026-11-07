use std::{path::Path, sync::Arc};

use anyhow::Context;
use clap::Parser;
use rdc_engine::EngineOptions;
use rdc_model::AuthenticationType;
use rdc_server::{
    app,
    auth::{AuthPolicy, Sessions},
    config::{Command, Config},
};
use rdc_store::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    match config.command.unwrap_or(Command::Serve) {
        Command::Healthcheck => rdc_server::config::healthcheck(&config).await,
        Command::Serve => serve(&config).await,
    }
}

async fn serve(config: &Config) -> anyhow::Result<()> {
    let logs = rdc_server::logging::init(config).context("initializing logging")?;
    let store = Store::open(&config.database)
        .await
        .context("opening database")?;
    let key_path = config
        .database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("session.key");
    let sessions = Sessions::open(store.clone(), &key_path)
        .await
        .context("opening session key")?;
    let listener_logs = logs.clone();
    let options = EngineOptions {
        settings_listener: Some(Arc::new(move |settings| {
            if let Err(error) = listener_logs.set_level(settings.general.log_level) {
                tracing::error!(%error, "Cannot update log level");
            }
            let secrets = [
                Some(settings.provider.api_key.clone()),
                Some(settings.download_client.aria2c_secret.clone()),
                settings.download_client.download_station_password.clone(),
            ];
            listener_logs.set_secrets(secrets.into_iter().flatten());
        })),
        ..EngineOptions::default()
    };
    let engine = rdc_engine::Engine::start(store.clone(), options)
        .await
        .context("starting torrent engine")?;
    let auth_engine = engine.clone();
    let policy = AuthPolicy::new(
        sessions.clone(),
        Arc::new(move || {
            auth_engine.settings().general.authentication_type == AuthenticationType::None
        }),
    );
    let shutdown = tokio_util::sync::CancellationToken::new();
    let router = app::build(
        app::Backend {
            engine: engine.clone(),
            store,
            sessions,
            policy,
        },
        &config.base_path,
        logs,
        shutdown.clone(),
    );
    let result = rdc_server::http::serve(config.port, router, shutdown)
        .await
        .context("serving HTTP");
    engine.shutdown().await;
    result
}
