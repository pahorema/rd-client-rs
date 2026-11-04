//! Command-line and environment configuration for the server process.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Process configuration; explicit command-line arguments override environment values.
#[derive(Clone, Debug, Parser)]
#[command(
    name = "rd-client",
    version,
    about = "Self-hosted debrid torrent client"
)]
pub struct Config {
    /// Listen on every interface; zero or invalid values use 6500.
    #[arg(long, env = "PORT", default_value = "6500", value_parser = parse_port, allow_hyphen_values = true, global = true)]
    pub port: u16,
    /// Also serve the application under this URL prefix, for example /downloads.
    #[arg(long, env = "BASE_PATH", default_value = "", value_parser = parse_base_path, global = true)]
    pub base_path: String,
    /// SQLite database file.
    #[arg(long, env = "DATABASE_PATH", default_value_os_t = default_database(), global = true)]
    pub database: PathBuf,
    /// Log file; defaults to rd-client.log beside the database.
    #[arg(long, env = "LOG_PATH", global = true)]
    pub log_file: Option<PathBuf>,
    /// Maximum log file size in bytes before rotation.
    #[arg(long, env = "LOG_MAX_SIZE", default_value = "5242880", value_parser = clap::value_parser!(u64).range(1..), global = true)]
    pub log_max_size: u64,
    /// Maximum number of log files, including the active file.
    #[arg(long, env = "LOG_MAX_FILES", default_value = "5", value_parser = clap::value_parser!(u32).range(1..), global = true)]
    pub log_max_files: u32,
    /// Operation to execute; omitted means serve.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported process operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum Command {
    /// Run the HTTP server (the default operation).
    Serve,
    /// Check the local HTTP server; exit successfully only for a healthy response.
    Healthcheck,
}

impl Config {
    /// Resolve the log path after the database argument and environment are applied.
    pub fn log_path(&self) -> PathBuf {
        self.log_file.clone().unwrap_or_else(|| {
            self.database
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("rd-client.log")
        })
    }
}

/// Canonicalize a prefix to an empty string or a slash-delimited absolute URL path.
pub fn normalize_base_path(value: &str) -> String {
    let segments: Vec<_> = value
        .trim()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn parse_base_path(value: &str) -> Result<String, String> {
    let path = normalize_base_path(value);
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'?' | b'#'))
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err("base path must be a URL path without whitespace, query, or fragment".into());
    }
    Ok(path)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "clap custom parsers require a Result"
)]
fn parse_port(value: &str) -> Result<u16, String> {
    Ok(value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .unwrap_or(6500))
}

#[cfg(not(windows))]
fn default_database() -> PathBuf {
    PathBuf::from("/data/db/rd-client.db")
}

#[cfg(windows)]
fn default_database() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
        .join("rd-client.db")
}

/// Probe the configured loopback server without requiring an external HTTP utility.
///
/// # Errors
/// Returns a network error, timeout, or non-success HTTP status.
pub async fn healthcheck(config: &Config) -> anyhow::Result<()> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .user_agent(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
        .no_proxy()
        .build()?
        .get(format!(
            "http://127.0.0.1:{}{}/",
            config.port, config.base_path
        ))
        .send()
        .await?;
    response.error_for_status()?;
    Ok(())
}
