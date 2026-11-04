use std::{
    io,
    sync::{Arc, Mutex},
};

use clap::{CommandFactory, FromArgMatches, Parser};
use rdc_model::LogLevel;
use rdc_server::{
    config::{Config, normalize_base_path},
    logging,
};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Debug, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("capture lock poisoned").clone())
            .expect("logs are UTF-8")
    }
}

impl io::Write for Capture {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("capture lock poisoned")
            .extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

fn parse(args: &[&str]) -> Config {
    let command = Config::command().mut_args(|arg| arg.env(None::<&str>));
    Config::from_arg_matches(
        &command
            .try_get_matches_from(args)
            .expect("valid test arguments"),
    )
    .expect("valid test configuration")
}

#[test]
fn defaults_and_cli_configuration_are_resolved() {
    let config = parse(&["rd-client"]);
    assert_eq!(config.port, 6500);
    assert_eq!(config.base_path, "");
    assert_eq!(config.log_max_size, 5 * 1024 * 1024);
    assert_eq!(config.log_max_files, 5);
    assert_eq!(config.log_path().file_name().unwrap(), "rd-client.log");
    #[cfg(not(windows))]
    assert_eq!(
        config.database,
        std::path::Path::new("/data/db/rd-client.db")
    );
    let config = parse(&[
        "rd-client",
        "--database",
        "/tmp/db.sqlite",
        "--base-path",
        "//x/y//",
        "--port",
        "9999",
        "serve",
    ]);
    assert_eq!(config.port, 9999);
    assert_eq!(config.base_path, "/x/y");
    assert_eq!(
        config.log_path(),
        std::path::Path::new("/tmp/rd-client.log")
    );
    assert!(matches!(
        config.command,
        Some(rdc_server::config::Command::Serve)
    ));
    assert!(
        Config::command()
            .render_long_help()
            .to_string()
            .contains("DATABASE_PATH")
    );
}

#[test]
fn invalid_and_zero_ports_use_the_default() {
    for value in ["0", "invalid", "65536", "-1"] {
        assert_eq!(parse(&["rd-client", "--port", value]).port, 6500);
    }
}

#[test]
fn base_paths_are_normalized_and_invalid_urls_rejected() {
    for (input, expected) in [
        ("", ""),
        (" / ", ""),
        ("x/y", "/x/y"),
        ("//x///y//", "/x/y"),
    ] {
        assert_eq!(normalize_base_path(input), expected);
    }
    for value in ["/x?y", "/x#y", "/x/../y", "/x y"] {
        assert!(Config::try_parse_from(["rd-client", "--base-path", value]).is_err());
    }
}

#[test]
fn environment_and_cli_precedence_are_isolated() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "environment_probe", "--nocapture"])
        .env("RDC_CONFIG_PROBE", "1")
        .env("PORT", "1234")
        .env("BASE_PATH", "//env/path/")
        .env("DATABASE_PATH", "/tmp/env/db.sqlite")
        .env("LOG_PATH", "/tmp/env/custom.log")
        .env("LOG_MAX_SIZE", "12345")
        .env("LOG_MAX_FILES", "3")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
fn environment_probe() {
    if std::env::var_os("RDC_CONFIG_PROBE").is_none() {
        return;
    }
    let config = Config::parse_from(["rd-client"]);
    assert_eq!(config.port, 1234);
    assert_eq!(config.base_path, "/env/path");
    assert_eq!(config.database, std::path::Path::new("/tmp/env/db.sqlite"));
    assert_eq!(
        config.log_path(),
        std::path::Path::new("/tmp/env/custom.log")
    );
    assert_eq!(config.log_max_size, 12345);
    assert_eq!(config.log_max_files, 3);
    let config = Config::parse_from([
        "rd-client",
        "--port",
        "4321",
        "--base-path",
        "cli",
        "--log-file",
        "/tmp/cli.log",
    ]);
    assert_eq!(config.port, 4321);
    assert_eq!(config.base_path, "/cli");
    assert_eq!(config.log_path(), std::path::Path::new("/tmp/cli.log"));
}

#[test]
fn every_formatted_event_is_redacted_and_levels_reload() {
    let capture = Capture::default();
    let (subscriber, logs) = logging::subscriber(capture.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("hidden initially");
        tracing::warn!("visible warning");
        logs.set_level(LogLevel::Verbose).unwrap();
        logs.set_secrets([
            "long-secret".to_owned(),
            "short".to_owned(),
            "long-secret-extension".to_owned(),
        ]);
        let span = tracing::info_span!("credentials", key = "long-secret");
        let entered = span.enter();
        tracing::trace!(token = "long-secret-extension", "message long-secret short");
        logs.set_level(LogLevel::Error).unwrap();
        tracing::warn!("hidden after reload");
        tracing::error!("visible error");
        drop(entered);
        drop(span);
        logs.set_secrets(["next-secret".to_owned()]);
        tracing::error!("next-secret is hidden");
    });
    let output = capture.text();
    assert!(!output.contains("hidden initially"));
    assert!(!output.contains("hidden after reload"));
    assert!(!output.contains("long-secret"));
    assert!(!output.contains("next-secret"));
    assert!(output.contains("short"));
    assert!(output.contains("visible warning"));
    assert!(output.contains("visible error"));
    assert!(output.contains("*****"));
}
