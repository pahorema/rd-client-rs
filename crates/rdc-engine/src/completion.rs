//! Detached completion programs with single-pass torrent arguments and bounded process lifetimes.

use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use rdc_model::{Torrent, paths};
use tokio::{fs, io::AsyncReadExt, process::Command, task::JoinHandle};
use tracing::instrument::WithSubscriber;

/// Completion programs may run for ten minutes before they are killed.
const COMPLETION_TIMEOUT: Duration = Duration::from_mins(10);

/// Plain settings for a program launched after a torrent completes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionSettings {
    /// Executable path, used directly without a shell; missing or blank disables execution.
    pub program: Option<String>,
    /// Argument template, parsed as shell-style words without executing shell expressions.
    pub arguments: Option<String>,
    /// Local download root, before the optional category component.
    pub download_path: PathBuf,
}

/// Failures while preparing or running a completion program.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CompletionError {
    /// The torrent directory could not be inspected or a process operation failed.
    #[error("completion program I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The argument template contains an unterminated quote or escape.
    #[error("invalid completion program arguments: {0}")]
    Arguments(#[from] shell_words::ParseError),
}

/// Starts a completion program in a detached task without waiting for filesystem or process work.
///
/// Returns `None` for a missing or whitespace-only executable. Otherwise the returned handle
/// may be awaited for shutdown coordination; dropping it does not cancel the task. The task
/// captures the supplied torrent values and download count, logs failures, captures both
/// output streams, and kills the process after ten minutes. It inherits the current tracing
/// subscriber. Aborting the task also kills its child process.
///
/// The executable is never passed to a shell, and shell expressions in arguments remain
/// literal. An explicitly configured shell executable is still allowed.
///
/// # Panics
///
/// Panics if an executable is configured but no Tokio runtime is active.
pub fn run_torrent_complete(
    settings: &CompletionSettings,
    torrent: &Torrent,
    download_count: usize,
) -> Option<JoinHandle<()>> {
    run_with_timeout(settings, torrent, download_count, COMPLETION_TIMEOUT)
}

/// Renders completion tokens once, leaving tokens inside substituted values untouched.
///
/// `%N` (torrent name), `%L` (category), `%F` (single top-level file, otherwise torrent
/// directory), `%R` (download root including category), and `%D` (torrent directory) are
/// wrapped in literal double quotes without escaping their values. `%C` (download count),
/// `%Z` (byte size), and `%I` (hash) are unquoted. Missing
/// names and categories render as empty quoted strings, a missing size renders empty,
/// and unknown tokens remain unchanged. Tokens already supply their own quotes.
///
/// Paths use sanitized category/name components, with `Unknown` for a missing name.
/// Missing torrent directories are allowed. File discovery is non-recursive and is only
/// performed when `%F` occurs. Non-Unicode path text is rendered lossily.
///
/// # Errors
///
/// Returns an I/O error if an existing torrent directory cannot be inspected.
pub async fn render_completion_arguments(
    template: &str,
    download_path: &Path,
    torrent: &Torrent,
    download_count: usize,
) -> Result<String, CompletionError> {
    Values::new(download_path, torrent, download_count)
        .render(template)
        .await
}

struct Values {
    name: String,
    category: String,
    download_path: PathBuf,
    torrent_path: PathBuf,
    download_count: usize,
    size: Option<i64>,
    hash: String,
}

impl Values {
    fn new(download_path: &Path, torrent: &Torrent, download_count: usize) -> Self {
        let category = torrent.category.as_deref().unwrap_or_default();
        let mut download_path = download_path.to_path_buf();
        if !category.trim().is_empty() {
            download_path.push(paths::remove_invalid_file_name_chars(category));
        }
        let torrent_path = download_path.join(paths::remove_invalid_file_name_chars(
            torrent.rd_name.as_deref().unwrap_or("Unknown"),
        ));
        Self {
            name: torrent.rd_name.clone().unwrap_or_default(),
            category: category.to_owned(),
            download_path,
            torrent_path,
            download_count,
            size: torrent.rd_size,
            hash: torrent.hash.clone(),
        }
    }

    async fn render(&self, template: &str) -> Result<String, CompletionError> {
        let file_path = if template.contains("%F") {
            single_file_or_directory(&self.torrent_path).await?
        } else {
            self.torrent_path.clone()
        };
        let mut rendered = String::with_capacity(template.len());
        let mut chars = template.chars();
        while let Some(character) = chars.next() {
            if character != '%' {
                rendered.push(character);
                continue;
            }
            match chars.next() {
                Some('N') => push_quoted(&mut rendered, &self.name),
                Some('L') => push_quoted(&mut rendered, &self.category),
                Some('F') => push_quoted(&mut rendered, &file_path.to_string_lossy()),
                Some('R') => push_quoted(&mut rendered, &self.download_path.to_string_lossy()),
                Some('D') => push_quoted(&mut rendered, &self.torrent_path.to_string_lossy()),
                Some('C') => rendered.push_str(&self.download_count.to_string()),
                Some('Z') => {
                    if let Some(size) = self.size {
                        rendered.push_str(&size.to_string());
                    }
                }
                Some('I') => rendered.push_str(&self.hash),
                Some(other) => {
                    rendered.push('%');
                    rendered.push(other);
                }
                None => rendered.push('%'),
            }
        }
        Ok(rendered)
    }
}

fn push_quoted(destination: &mut String, value: &str) {
    destination.push('"');
    destination.push_str(value);
    destination.push('"');
}

async fn single_file_or_directory(directory: &Path) -> io::Result<PathBuf> {
    let mut entries = match fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(directory.to_path_buf()),
        Err(error) => return Err(error),
    };
    let mut file = None;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if fs::metadata(&path).await?.is_file() {
            if file.is_some() {
                return Ok(directory.to_path_buf());
            }
            file = Some(path);
        }
    }
    Ok(file.unwrap_or_else(|| directory.to_path_buf()))
}

fn run_with_timeout(
    settings: &CompletionSettings,
    torrent: &Torrent,
    download_count: usize,
    timeout: Duration,
) -> Option<JoinHandle<()>> {
    let program = settings
        .program
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    let program = program.to_owned();
    let template = settings.arguments.clone().unwrap_or_default();
    let values = Values::new(&settings.download_path, torrent, download_count);
    Some(tokio::spawn(
        async move {
            let result = async {
                let arguments = values.render(&template).await?;
                execute(&program, &arguments, timeout).await
            }
            .await;
            if let Err(error) = result {
                tracing::error!(%error, %program, "Unable to run completion program");
            }
        }
        .with_current_subscriber(),
    ))
}

async fn execute(program: &str, arguments: &str, timeout: Duration) -> Result<(), CompletionError> {
    let arguments = shell_words::split(arguments)?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout is configured as piped");
    let mut stderr = child.stderr.take().expect("stderr is configured as piped");
    let mut output = Vec::new();
    let mut errors = Vec::new();
    // Read both pipes while waiting, so neither stream can fill its pipe and deadlock.
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut errors),
        )
    })
    .await;
    let timed_out = if let Ok(result) = result {
        result?;
        false
    } else {
        child.kill().await?;
        true
    };
    let output = trimmed_lines(&output);
    let errors = trimmed_lines(&errors);
    if !errors.is_empty() {
        tracing::debug!("External application exited with errors: {errors}");
    }
    if !output.is_empty() {
        tracing::debug!("External application exited with output: {output}");
    }
    if timed_out {
        tracing::debug!(
            timeout_seconds = timeout.as_secs_f64(),
            "External application killed after timeout"
        );
    }
    Ok(())
}

fn trimmed_lines(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        result.push_str(line.trim());
        result.push('\n');
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
    };

    use rdc_model::Download;
    use tempfile::TempDir;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("test log buffer lock is not poisoned")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for Logs {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .expect("test log buffer lock is not poisoned")
                    .clone(),
            )
            .expect("tracing writes UTF-8 log text")
        }
    }

    fn torrent() -> Torrent {
        Torrent {
            rd_name: Some("TestTorrent".to_owned()),
            category: Some("Movies".to_owned()),
            hash: "123ABC".to_owned(),
            rd_size: Some(100),
            downloads: vec![Download {
                file_name: Some("file.txt".to_owned()),
                ..Download::default()
            }],
            ..Torrent::default()
        }
    }

    fn start_logged(
        settings: &CompletionSettings,
        torrent: &Torrent,
        timeout: Duration,
    ) -> (JoinHandle<()>, Logs) {
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let task = tracing::subscriber::with_default(subscriber, || {
            run_with_timeout(settings, torrent, torrent.downloads.len(), timeout)
                .expect("test completion program is configured")
        });
        (task, logs)
    }

    async fn script(body: &str) -> (TempDir, CompletionSettings) {
        let directory = tempfile::tempdir().expect("create temporary script directory");
        let program = directory.path().join("completion script");
        fs::write(&program, format!("#!/bin/sh\n{body}\n"))
            .await
            .expect("write completion script");
        fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .await
            .expect("make completion script executable");
        let settings = CompletionSettings {
            program: Some(
                program
                    .to_str()
                    .expect("temporary script path is UTF-8")
                    .to_owned(),
            ),
            download_path: directory.path().join("downloads"),
            ..CompletionSettings::default()
        };
        (directory, settings)
    }

    #[tokio::test]
    async fn configured_echo_receives_all_torrent_values_and_single_file() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = torrent();
        let torrent_path = directory.path().join("Movies/TestTorrent");
        fs::create_dir_all(&torrent_path)
            .await
            .expect("create torrent directory");
        let file_path = torrent_path.join("file.txt");
        fs::write(&file_path, b"data")
            .await
            .expect("write torrent file");
        let template = "%N %L %F %R %D %C %Z %I";
        let download_path = directory.path().join("Movies");
        let expected = format!(
            "\"TestTorrent\" \"Movies\" \"{}\" \"{}\" \"{}\" 1 100 123ABC",
            file_path.display(),
            download_path.display(),
            torrent_path.display(),
        );
        assert_eq!(
            render_completion_arguments(
                template,
                directory.path(),
                &torrent,
                torrent.downloads.len()
            )
            .await
            .expect("render completion arguments"),
            expected
        );
        let settings = CompletionSettings {
            program: Some("/bin/echo".to_owned()),
            arguments: Some(template.to_owned()),
            download_path: directory.path().to_path_buf(),
        };
        let (task, logs) = start_logged(&settings, &torrent, COMPLETION_TIMEOUT);
        task.await.expect("completion task finishes without panic");
        let expected_output = format!(
            "TestTorrent Movies {} {} {} 1 100 123ABC",
            file_path.display(),
            download_path.display(),
            torrent_path.display(),
        );
        assert!(logs.text().contains(&expected_output), "{}", logs.text());
    }

    #[test]
    fn missing_and_blank_programs_do_not_spawn_or_require_a_runtime() {
        for program in [None, Some(String::new()), Some(" \t\n".to_owned())] {
            let settings = CompletionSettings {
                program,
                arguments: Some("'unterminated %F".to_owned()),
                download_path: PathBuf::from("/missing/completion/directory"),
            };
            assert!(run_torrent_complete(&settings, &torrent(), 1).is_none());
        }
    }

    #[tokio::test]
    async fn stdout_lines_are_trimmed_and_logged_together_once() {
        let (_directory, settings) = script("printf '  first  \\n second\\r\\nthird  '").await;
        let (task, logs) = start_logged(&settings, &torrent(), COMPLETION_TIMEOUT);
        task.await.expect("completion task finishes without panic");
        let text = logs.text();
        assert_eq!(text.matches("exited with output").count(), 1, "{text}");
        assert!(text.contains("first\nsecond\nthird\n"), "{text}");
        assert!(!text.contains("exited with errors"), "{text}");
    }

    #[tokio::test]
    async fn stderr_lines_are_trimmed_and_logged_together_once() {
        let (_directory, settings) = script("printf '  first  \\n second\\r\\nthird  ' >&2").await;
        let (task, logs) = start_logged(&settings, &torrent(), COMPLETION_TIMEOUT);
        task.await.expect("completion task finishes without panic");
        let text = logs.text();
        assert_eq!(text.matches("exited with errors").count(), 1, "{text}");
        assert!(text.contains("first\nsecond\nthird\n"), "{text}");
        assert!(!text.contains("exited with output"), "{text}");
    }

    #[tokio::test]
    async fn replacement_values_are_not_expanded_again() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = Torrent {
            rd_name: Some("Name %L %I".to_owned()),
            category: Some("Category %N %Z".to_owned()),
            hash: "%C".to_owned(),
            rd_size: Some(123),
            ..Torrent::default()
        };
        let arguments =
            render_completion_arguments("%N %L %I %C %Z %Q %", directory.path(), &torrent, 7)
                .await
                .expect("render completion arguments");
        assert_eq!(arguments, "\"Name %L %I\" \"Category %N %Z\" %C 7 123 %Q %");
    }

    #[tokio::test]
    async fn missing_directory_uses_torrent_path_and_unknown_name_without_error() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = Torrent::default();
        let rendered =
            render_completion_arguments("%N|%L|%F|%D|%C|%Z|%I", directory.path(), &torrent, 0)
                .await
                .expect("render arguments for missing torrent directory");
        let path = directory.path().join("Unknown");
        assert_eq!(
            rendered,
            format!(
                "\"\"|\"\"|\"{}\"|\"{}\"|0||",
                path.display(),
                path.display()
            )
        );
        assert!(
            !path.exists(),
            "rendering must not create a torrent directory"
        );
    }

    #[tokio::test]
    async fn file_discovery_ignores_nested_files_and_uses_directory_for_multiple_files() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = torrent();
        let path = directory.path().join("Movies/TestTorrent");
        fs::create_dir_all(path.join("nested"))
            .await
            .expect("create nested torrent directory");
        fs::write(path.join("nested/inside.txt"), b"nested")
            .await
            .expect("write nested torrent file");
        assert_eq!(
            render_completion_arguments("%F", directory.path(), &torrent, 1)
                .await
                .expect("render file token for nested-only files"),
            format!("\"{}\"", path.display())
        );
        fs::write(path.join("one.txt"), b"one")
            .await
            .expect("write first top-level file");
        assert_eq!(
            render_completion_arguments("%F", directory.path(), &torrent, 1)
                .await
                .expect("render file token for single top-level file"),
            format!("\"{}\"", path.join("one.txt").display())
        );
        fs::write(path.join("two.txt"), b"two")
            .await
            .expect("write second top-level file");
        assert_eq!(
            render_completion_arguments("%F", directory.path(), &torrent, 1)
                .await
                .expect("render file token for multiple top-level files"),
            format!("\"{}\"", path.display())
        );
    }

    #[tokio::test]
    async fn ordinary_quoted_values_remain_single_literal_arguments() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = Torrent {
            rd_name: Some("a torrent name $(touch ignored)".to_owned()),
            category: Some("a 'category'".to_owned()),
            ..Torrent::default()
        };
        let rendered = render_completion_arguments("%N %L", directory.path(), &torrent, 0)
            .await
            .expect("render completion arguments");
        assert_eq!(
            shell_words::split(&rendered)
                .expect("rendered ordinary arguments have balanced quotes"),
            vec![
                torrent.rd_name.expect("test torrent has a name"),
                torrent.category.expect("test torrent has a category")
            ]
        );
    }

    #[tokio::test]
    async fn quoting_wraps_values_without_escaping_quotes_or_backslashes() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let torrent = Torrent {
            rd_name: Some("a \"quoted\" \\ name".to_owned()),
            ..Torrent::default()
        };
        let rendered = render_completion_arguments("%N", directory.path(), &torrent, 0)
            .await
            .expect("render completion arguments");
        assert_eq!(rendered, "\"a \"quoted\" \\ name\"");
    }

    #[tokio::test]
    async fn shell_expressions_in_arguments_are_not_executed() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let marker = directory.path().join("must-not-exist");
        let settings = CompletionSettings {
            program: Some("/bin/echo".to_owned()),
            arguments: Some(format!("$(touch {}) ; literal | text", marker.display())),
            download_path: directory.path().to_path_buf(),
        };
        let (task, logs) = start_logged(&settings, &torrent(), COMPLETION_TIMEOUT);
        task.await.expect("completion task finishes without panic");
        assert!(!marker.exists());
        assert!(logs.text().contains("; literal | text"), "{}", logs.text());
    }

    #[tokio::test]
    async fn missing_directory_does_not_prevent_program_execution() {
        let directory = tempfile::tempdir().expect("create temporary download directory");
        let settings = CompletionSettings {
            program: Some("/bin/echo".to_owned()),
            arguments: Some("%F".to_owned()),
            download_path: directory.path().join("missing"),
        };
        let (task, logs) = start_logged(&settings, &torrent(), COMPLETION_TIMEOUT);
        task.await.expect("completion task finishes without panic");
        assert!(
            logs.text().contains("missing/Movies/TestTorrent"),
            "{}",
            logs.text()
        );
    }

    #[tokio::test]
    async fn detached_program_returns_before_exit_and_timeout_kills_the_process() {
        let (directory, mut settings) =
            script("printf '%s' \"$$\" > \"$1\"\nexec /bin/sleep 30").await;
        let pid_path = directory.path().join("child.pid");
        settings.arguments = Some(
            shell_words::quote(pid_path.to_str().expect("temporary PID path is UTF-8"))
                .into_owned(),
        );
        let (task, logs) = start_logged(&settings, &torrent(), Duration::from_millis(500));
        assert!(
            !task.is_finished(),
            "the caller must not wait for process completion"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if fs::read_to_string(&pid_path)
                    .await
                    .is_ok_and(|pid| !pid.is_empty())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completion script writes its PID before deadline");
        let pid = fs::read_to_string(&pid_path)
            .await
            .expect("read completion process PID");
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("completion process timeout finishes before deadline")
            .expect("completion task finishes without panic");
        let status = Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .await
            .expect("inspect timed-out process existence");
        assert!(!status.success(), "timed-out process {pid} is still alive");
        assert!(
            logs.text().contains("killed after timeout"),
            "{}",
            logs.text()
        );
    }
}
