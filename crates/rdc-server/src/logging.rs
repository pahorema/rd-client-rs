//! Reloadable tracing levels, event-wide redaction, and bounded file rotation.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use rdc_model::LogLevel;
use tracing::{Event, Subscriber};
use tracing_subscriber::{
    Registry,
    filter::{LevelFilter, Targets},
    fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter, format::Writer},
    layer::SubscriberExt,
    registry::LookupSpan,
    reload,
    util::SubscriberInitExt,
};

use crate::config::Config;

type LevelHandle = reload::Handle<LevelFilter, Registry>;

/// Cloneable controls shared by settings updates and request logging.
#[derive(Clone)]
pub struct LogControl {
    level: LevelHandle,
    secrets: Arc<RwLock<Vec<String>>>,
}

impl fmt::Debug for LogControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LogControl").finish_non_exhaustive()
    }
}

impl LogControl {
    /// Change verbosity immediately for subsequent events.
    pub fn set_level(&self, level: LogLevel) -> Result<(), reload::Error> {
        self.level.reload(match level {
            LogLevel::Verbose => LevelFilter::TRACE,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Information => LevelFilter::INFO,
            LogLevel::Warning => LevelFilter::WARN,
            LogLevel::Error => LevelFilter::ERROR,
        })
    }

    /// Replace the secret catalogue; values of five characters or fewer are ignored.
    pub fn set_secrets(&self, secrets: impl IntoIterator<Item = String>) {
        let mut secrets: Vec<_> = secrets
            .into_iter()
            .filter(|s| s.chars().count() > 5)
            .flat_map(|secret| {
                let encoded: String =
                    url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
                let json = serde_json::to_string(&secret).expect("strings always serialize");
                let escaped = json[1..json.len() - 1].to_owned();
                [secret, encoded.replace('+', "%20"), encoded, escaped]
            })
            .collect();
        // Longer overlapping values must be replaced before their substrings.
        secrets.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        *self
            .secrets
            .write()
            .expect("secret catalogue lock poisoned") = secrets;
    }

    /// Mask the current secret catalogue in arbitrary text.
    pub fn redact(&self, text: &str) -> String {
        redact(text, &self.secrets)
    }
}

fn redact(text: &str, secrets: &RwLock<Vec<String>>) -> String {
    let secrets = secrets.read().expect("secret catalogue lock poisoned");
    let mut result = text.to_owned();
    for secret in secrets.iter() {
        result = result.replace(secret, "*****");
    }
    result
}

/// Build a subscriber without installing it, allowing isolated embedded/test use.
/// The initial level is Warning. Every formatted event is redacted before writing.
pub fn subscriber<W>(writer: W) -> (impl Subscriber + Send + Sync, LogControl)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let (level, handle) = reload::Layer::new(LevelFilter::WARN);
    let secrets = Arc::new(RwLock::new(Vec::new()));
    let format = RedactedFormat {
        secrets: secrets.clone(),
    };
    // The database driver logs every query at debug; only its warnings (slow queries) are useful.
    let quiet_database = Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_target("sqlx", LevelFilter::WARN);
    let subscriber = Registry::default().with(level).with(quiet_database).with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .event_format(format)
            .with_writer(writer),
    );
    (
        subscriber,
        LogControl {
            level: handle,
            secrets,
        },
    )
}

/// Install console and size-rotated file logging as the process subscriber.
///
/// # Errors
/// Fails when the directory/file cannot be opened or a subscriber is already installed.
pub fn init(config: &Config) -> anyhow::Result<LogControl> {
    let output = Output::open(
        &config.log_path(),
        config.log_max_size,
        config.log_max_files,
    )?;
    let (subscriber, control) = subscriber(OutputWriter(Arc::new(Mutex::new(output))));
    subscriber.try_init()?;
    Ok(control)
}

struct RedactedFormat {
    secrets: Arc<RwLock<Vec<String>>>,
}

impl<S, N> FormatEvent<S, N> for RedactedFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut rendered = String::new();
        tracing_subscriber::fmt::format().format_event(ctx, Writer::new(&mut rendered), event)?;
        writer.write_str(&redact(&rendered, &self.secrets))
    }
}

#[derive(Clone)]
struct OutputWriter(Arc<Mutex<Output>>);

impl<'a> MakeWriter<'a> for OutputWriter {
    type Writer = OutputGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        OutputGuard(self.0.lock().expect("log output lock poisoned"))
    }
}

struct OutputGuard<'a>(std::sync::MutexGuard<'a, Output>);

impl Write for OutputGuard<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write_event(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.file_mut()?.flush()?;
        io::stderr().flush()
    }
}

struct Output {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max_size: u64,
    max_files: u32,
}

impl Output {
    fn open(path: &Path, max_size: u64, max_files: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
            size,
            max_size,
            max_files,
        })
    }

    fn archive_path(&self, index: u32) -> PathBuf {
        let mut path = self.path.as_os_str().to_owned();
        path.push(format!(".{index}"));
        PathBuf::from(path)
    }

    fn file_mut(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is unavailable"))
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file_mut()?.flush()?;
        drop(self.file.take());
        if self.max_files > 1 {
            for index in (1..self.max_files - 1).rev() {
                let source = self.archive_path(index);
                if source.exists() {
                    fs::rename(source, self.archive_path(index + 1))?;
                }
            }
            fs::rename(&self.path, self.archive_path(1))?;
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?,
        );
        self.size = 0;
        Ok(())
    }

    fn write_event(&mut self, bytes: &[u8]) -> io::Result<()> {
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(size) > self.max_size {
            self.rotate()?;
        }
        self.file_mut()?.write_all(bytes)?;
        self.size = self.size.saturating_add(size);
        io::stderr().write_all(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_rotation_retains_the_configured_number_of_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.log");
        let mut output = Output::open(&path, 5, 3).unwrap();
        for event in [b"first".as_slice(), b"second", b"third", b"last"] {
            output.write_event(event).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"last");
        assert_eq!(fs::read(output.archive_path(1)).unwrap(), b"third");
        assert_eq!(fs::read(output.archive_path(2)).unwrap(), b"second");
        assert!(!output.archive_path(3).exists());
    }

    #[test]
    fn single_file_rotation_and_existing_file_sizes_are_honored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.log");
        fs::write(&path, b"existing").unwrap();
        let mut output = Output::open(&path, 5, 1).unwrap();
        output.write_event(b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!output.archive_path(1).exists());
    }
}
