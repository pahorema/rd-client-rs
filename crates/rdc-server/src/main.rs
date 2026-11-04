use clap::Parser;
use rdc_server::config::{Command, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    match config.command.unwrap_or(Command::Serve) {
        Command::Healthcheck => rdc_server::config::healthcheck(&config).await,
        Command::Serve => {
            let logs = rdc_server::logging::init(&config)?;
            let app = rdc_server::http::AppBuilder::new(&config.base_path, logs).build(());
            rdc_server::http::serve(config.port, app).await
        }
    }
}
