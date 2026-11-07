//! Behaviour of the internal HTTP engine against a local scripted server.

pub mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rdc_download::{
    CANCELLED_MESSAGE, DownloadEngine, HttpDownload, InternalConfig, Outcome, Progress,
};
use support::{FileOptions, Framing, Reply, TestServer, file_reply, test_data};
use tokio::sync::watch;

const WAIT_LIMIT: Duration = Duration::from_secs(15);

fn config() -> InternalConfig {
    InternalConfig::new()
}

struct Run {
    download: HttpDownload,
    target: PathBuf,
    _dir: tempfile::TempDir,
}

impl Run {
    async fn start(url: &str, config: watch::Receiver<InternalConfig>) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("file.bin");
        let download = HttpDownload::new(url, &target, config);
        let id = download.start().await.expect("start");
        assert_eq!(id.len(), 36, "remote id {id} is a UUID");
        Self {
            download,
            target,
            _dir: dir,
        }
    }

    async fn finish(&self) -> Outcome {
        tokio::time::timeout(WAIT_LIMIT, self.download.completion().wait())
            .await
            .expect("download finishes in time")
    }

    fn partial(&self) -> PathBuf {
        partial_of(&self.target)
    }

    async fn wait_for_bytes(&self, at_least: u64) {
        let mut progress = self.download.progress();
        tokio::time::timeout(
            WAIT_LIMIT,
            progress.wait_for(|progress| progress.bytes_done >= at_least),
        )
        .await
        .expect("progress in time")
        .expect("progress channel open");
    }
}

fn partial_of(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".download");
    PathBuf::from(name)
}

async fn download(url: &str, config: InternalConfig) -> (Run, Outcome) {
    let (_tx, rx) = watch::channel(config);
    let run = Run::start(url, rx).await;
    let outcome = run.finish().await;
    (run, outcome)
}

fn assert_downloaded(run: &Run, data: &[u8]) {
    let written = std::fs::read(&run.target).expect("target file");
    assert!(
        written == data,
        "content differs: {} vs {} bytes",
        written.len(),
        data.len()
    );
    assert!(!run.partial().exists(), "partial file removed");
}

fn single_chunk() -> InternalConfig {
    InternalConfig {
        chunk_count: 1,
        ..config()
    }
}

fn slow_options() -> FileOptions {
    FileOptions {
        write_size: 2048,
        write_delay: Duration::from_millis(20),
        ..FileOptions::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_a_file_and_reports_its_size() {
    let data = test_data(64 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(
        *run.download.progress().borrow(),
        Progress {
            bytes_done: 65_536,
            bytes_total: 65_536,
            speed: 0
        }
    );
}

#[tokio::test]
async fn invalid_links_fail_to_start() {
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new("not a url", "/tmp/never", rx);
    let error = download.start().await.expect_err("invalid link");
    assert_eq!(error.message(), "Invalid download link not a url");
    assert_eq!(
        download.completion().outcome(),
        Some(Err("Invalid download link not a url".to_owned()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_errors_fail_with_the_status_and_remove_the_stale_partial() {
    let server = TestServer::start(|_| Reply::status(404)).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("file.bin");
    std::fs::write(partial_of(&target), b"stale").expect("stale partial");
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new(server.url("/missing"), &target, rx);
    download.start().await.expect("start");
    let outcome = tokio::time::timeout(WAIT_LIMIT, download.completion().wait())
        .await
        .expect("finishes");
    assert_eq!(
        outcome,
        Err("Response status code does not indicate success: 404 (Not Found).".to_owned())
    );
    assert!(!partial_of(&target).exists());
    assert!(!target.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_file_is_replaced_and_stale_partial_removed() {
    let data = test_data(4096);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("file.bin");
    std::fs::write(&target, b"old content").expect("old file");
    std::fs::write(partial_of(&target), vec![7; 100_000]).expect("stale partial");
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new(server.url("/file.bin"), &target, rx);
    download.start().await.expect("start");
    assert_eq!(download.completion().wait().await, Ok(()));
    assert_eq!(std::fs::read(&target).expect("target"), *data);
    assert!(!partial_of(&target).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_identify_the_client_and_ask_for_identity_encoding() {
    let data = test_data(2048);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (_run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    for request in server.requests() {
        assert_eq!(
            request.header("user-agent"),
            Some(concat!("rd-client/", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(request.header("accept"), Some("*/*"));
        assert_eq!(request.header("accept-encoding"), Some("identity"));
    }
}

#[tokio::test]
async fn cancel_before_start_completes_and_start_then_fails() {
    let (_tx, rx) = watch::channel(config());
    let download = HttpDownload::new("http://127.0.0.1:9/file", "/nonexistent/file", rx);
    download.cancel().await;
    assert_eq!(
        download.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    let error = download.start().await.expect_err("cancelled");
    assert_eq!(error.message(), CANCELLED_MESSAGE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_the_download_and_removes_the_partial_file() {
    let data = test_data(128 * 1024);
    let server = TestServer::file(Arc::clone(&data), slow_options()).await;
    let (_tx, rx) = watch::channel(config());
    let run = Run::start(&server.url("/file.bin"), rx).await;
    run.wait_for_bytes(1).await;
    tokio::time::timeout(WAIT_LIMIT, run.download.cancel())
        .await
        .expect("cancel returns");
    assert_eq!(
        run.download.completion().outcome(),
        Some(Err(CANCELLED_MESSAGE.to_owned()))
    );
    assert!(!run.partial().exists());
    assert!(!run.target.exists());
    run.download.cancel().await;
    assert_eq!(run.finish().await, Err(CANCELLED_MESSAGE.to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_reads_fail_with_a_timeout() {
    let data = test_data(32 * 1024);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| {
            let options = FileOptions {
                ranges: false,
                ..FileOptions::default()
            };
            let mut reply = file_reply(&data, request, &options);
            reply.stall_after = Some((4096, Duration::from_secs(5)));
            reply
        })
        .await
    };
    let fast_timeout = InternalConfig {
        timeout_ms: 150,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), fast_timeout).await;
    assert_eq!(outcome, Err("No data was received for 150 ms.".to_owned()));
    assert!(!run.target.exists());
    assert!(!run.partial().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probes_with_a_first_byte_range_request() {
    let data = test_data(4096);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (_run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    let first = server.requests().first().cloned().expect("probe");
    assert_eq!(first.range(), Some((0, Some(0))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn servers_without_range_support_get_one_plain_request() {
    let data = test_data(20_000);
    let options = FileOptions {
        ranges: false,
        ..FileOptions::default()
    };
    let server = TestServer::file(Arc::clone(&data), options).await;
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let transfers = server.transfer_requests();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].header("range"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_size_is_finalised_to_the_received_bytes() {
    let data = test_data(50_000);
    let options = FileOptions {
        ranges: false,
        framing: Framing::CloseDelimited,
        ..FileOptions::default()
    };
    let server = TestServer::file(Arc::clone(&data), options).await;
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(
        *run.download.progress().borrow(),
        Progress {
            bytes_done: 50_000,
            bytes_total: 50_000,
            speed: 0
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_retries_passing_server_errors() {
    let data = test_data(4096);
    let probes = Arc::new(AtomicUsize::new(0));
    let server = {
        let data = Arc::clone(&data);
        let probes = Arc::clone(&probes);
        TestServer::start(move |request| {
            if probes.fetch_add(1, Ordering::SeqCst) < 2 {
                return Reply::status(503);
            }
            file_reply(&data, request, &FileOptions::default())
        })
        .await
    };
    let fast = InternalConfig {
        timeout_ms: 100,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), fast).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_retries_dropped_connections() {
    let data = test_data(4096);
    let probes = Arc::new(AtomicUsize::new(0));
    let server = {
        let data = Arc::clone(&data);
        let probes = Arc::clone(&probes);
        TestServer::start(move |request| {
            if probes.fetch_add(1, Ordering::SeqCst) == 0 {
                return Reply::hang_up();
            }
            file_reply(&data, request, &FileOptions::default())
        })
        .await
    };
    let fast = InternalConfig {
        timeout_ms: 100,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), fast).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let first_two: Vec<_> = server
        .requests()
        .iter()
        .take(2)
        .map(support::Request::range)
        .collect();
    assert_eq!(
        first_two,
        [Some((0, Some(0))); 2],
        "the probe was sent again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn range_refusing_servers_are_probed_again_without_range() {
    let data = test_data(4096);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| match request.range() {
            Some(_) => Reply::status(403),
            None => file_reply(
                &data,
                request,
                &FileOptions {
                    ranges: false,
                    ..FileOptions::default()
                },
            ),
        })
        .await
    };
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redirects_are_followed() {
    let data = test_data(8192);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| {
            if request.path() == "/old" {
                return Reply::status(302).header("Location", "/file.bin");
            }
            file_reply(&data, request, &FileOptions::default())
        })
        .await
    };
    let (run, outcome) = download(&server.url("/old"), config()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert!(
        server
            .transfer_requests()
            .iter()
            .all(|request| request.path() == "/file.bin" || request.path() == "/old")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failures_without_range_support_are_final_and_clean_up() {
    let data = test_data(64 * 1024);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| {
            let options = FileOptions {
                ranges: false,
                ..FileOptions::default()
            };
            let mut reply = file_reply(&data, request, &options);
            if request.range().is_none() {
                reply.abort_after = Some(1000);
            }
            reply
        })
        .await
    };
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert!(outcome.is_err(), "outcome {outcome:?}");
    assert!(!run.target.exists());
    assert!(!run.partial().exists());
    assert_eq!(server.transfer_requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn responses_ending_early_fail_as_incomplete() {
    let data = test_data(8000);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| {
            let options = FileOptions {
                ranges: false,
                ..FileOptions::default()
            };
            let mut reply = file_reply(&data, request, &options);
            if request.range().is_none() {
                reply.framing = Framing::CloseDelimited;
                reply.body.truncate(5000);
            }
            reply
        })
        .await
    };
    let (run, outcome) = download(&server.url("/file.bin"), config()).await;
    assert_eq!(
        outcome,
        Err("The download ended prematurely: received 5000 of 8000 bytes of a chunk.".to_owned())
    );
    assert!(!run.partial().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn small_files_are_fetched_with_one_plain_request() {
    let data = test_data(512);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (run, outcome) = download(
        &server.url("/file.bin"),
        InternalConfig {
            chunk_count: 8,
            ..config()
        },
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let transfers = server.transfer_requests();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].header("range"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_a_file_in_ranged_chunks() {
    let data = test_data(64 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let chunked = InternalConfig {
        chunk_count: 4,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), chunked).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let ranges: Vec<_> = server
        .transfer_requests()
        .iter()
        .filter_map(support::Request::range)
        .collect();
    assert_eq!(
        ranges,
        vec![
            (0, Some(16_383)),
            (16_384, Some(32_767)),
            (32_768, Some(49_151)),
            (49_152, Some(65_535))
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn downloads_a_file_over_parallel_ranged_chunks() {
    let data = test_data(64 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (run, outcome) = download(
        &server.url("/file.bin"),
        InternalConfig {
            chunk_count: 4,
            parallel_connections: 4,
            ..config()
        },
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let mut ranges: Vec<_> = server
        .transfer_requests()
        .iter()
        .filter_map(support::Request::range)
        .collect();
    ranges.sort_unstable();
    assert_eq!(
        ranges,
        vec![
            (0, Some(16_383)),
            (16_384, Some(32_767)),
            (32_768, Some(49_151)),
            (49_152, Some(65_535))
        ]
    );
    assert_eq!(
        *run.download.progress().borrow(),
        Progress {
            bytes_done: 65_536,
            bytes_total: 65_536,
            speed: 0
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_connections_never_exceed_the_limit() {
    let data = test_data(64 * 1024);
    let options = FileOptions {
        write_size: 2048,
        write_delay: Duration::from_millis(10),
        ..FileOptions::default()
    };
    let server = TestServer::file(Arc::clone(&data), options).await;
    let (run, outcome) = download(
        &server.url("/file.bin"),
        InternalConfig {
            chunk_count: 8,
            parallel_connections: 2,
            ..config()
        },
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(server.transfer_requests().len(), 8);
    assert_eq!(server.max_concurrent(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_parallel_connections_use_a_single_connection() {
    let data = test_data(32 * 1024);
    let options = FileOptions {
        write_size: 4096,
        write_delay: Duration::from_millis(5),
        ..FileOptions::default()
    };
    let server = TestServer::file(Arc::clone(&data), options).await;
    let (run, outcome) = download(
        &server.url("/file.bin"),
        InternalConfig {
            chunk_count: 4,
            parallel_connections: 0,
            ..config()
        },
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(server.transfer_requests().len(), 4);
    assert_eq!(server.max_concurrent(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_reports_the_current_speed() {
    let data = test_data(256 * 1024);
    let server = TestServer::file(Arc::clone(&data), slow_options()).await;
    let (_tx, rx) = watch::channel(config());
    let run = Run::start(&server.url("/file.bin"), rx).await;
    let mut progress = run.download.progress();
    let moving = tokio::time::timeout(
        WAIT_LIMIT,
        progress.wait_for(|progress| progress.speed > 0 && progress.bytes_total == 256 * 1024),
    )
    .await
    .expect("speed reported in time")
    .expect("progress channel open");
    assert!(moving.bytes_done < 256 * 1024);
    drop(moving);
    run.download.cancel().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speed_limit_is_shared_by_all_connections() {
    let data = test_data(192 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let limited = InternalConfig {
        max_speed: 256 * 1024,
        chunk_count: 4,
        parallel_connections: 4,
        ..config()
    };
    let started = Instant::now();
    let (run, outcome) = download(&server.url("/file.bin"), limited).await;
    let elapsed = started.elapsed();
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    // 192 KiB at 256 KiB/s, minus a 250 ms burst, takes at least half a second.
    assert!(elapsed >= Duration::from_millis(450), "took {elapsed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn speed_limit_changes_apply_to_a_running_download() {
    let data = test_data(256 * 1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (tx, rx) = watch::channel(InternalConfig {
        max_speed: 16 * 1024,
        chunk_count: 2,
        parallel_connections: 2,
        ..config()
    });
    let started = Instant::now();
    let run = Run::start(&server.url("/file.bin"), rx).await;
    run.wait_for_bytes(1).await;
    tx.send_modify(|config| config.max_speed = 0);
    assert_eq!(run.finish().await, Ok(()));
    assert_downloaded(&run, &data);
    // At the initial limit the download would take 16 s.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_connection_resumes_from_the_received_offset() {
    let data = test_data(64 * 1024);
    let transfers = Arc::new(AtomicUsize::new(0));
    let server = {
        let data = Arc::clone(&data);
        let transfers = Arc::clone(&transfers);
        TestServer::start(move |request| {
            let mut reply = file_reply(&data, request, &FileOptions::default());
            if request.range() != Some((0, Some(0)))
                && transfers.fetch_add(1, Ordering::SeqCst) == 0
            {
                reply.abort_after = Some(10_000);
            }
            reply
        })
        .await
    };
    let (run, outcome) = download(&server.url("/file.bin"), single_chunk()).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let requests = server.transfer_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("range"), None);
    let (start, end) = requests[1].range().expect("resumed with a range");
    assert!(start > 0 && start <= 10_000, "resumed at {start}");
    assert_eq!(end, Some(65_535));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_reads_time_out_and_retry() {
    let data = test_data(32 * 1024);
    let transfers = Arc::new(AtomicUsize::new(0));
    let server = {
        let data = Arc::clone(&data);
        let transfers = Arc::clone(&transfers);
        TestServer::start(move |request| {
            let mut reply = file_reply(&data, request, &FileOptions::default());
            if request.range() != Some((0, Some(0)))
                && transfers.fetch_add(1, Ordering::SeqCst) == 0
            {
                reply.stall_after = Some((4096, Duration::from_secs(5)));
            }
            reply
        })
        .await
    };
    let fast_timeout = InternalConfig {
        timeout_ms: 150,
        ..single_chunk()
    };
    let started = Instant::now();
    let (run, outcome) = download(&server.url("/file.bin"), fast_timeout).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(server.transfer_requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_size_downloads_resume_with_an_open_range() {
    let data = test_data(32 * 1024);
    let transfers = Arc::new(AtomicUsize::new(0));
    let server = {
        let data = Arc::clone(&data);
        let transfers = Arc::clone(&transfers);
        TestServer::start(move |request| {
            if request.range() == Some((0, Some(0))) {
                let mut probe = Reply::status(206).header("Content-Range", "bytes 0-0/*");
                probe.body = data[..1].to_vec();
                return probe;
            }
            let options = FileOptions {
                framing: Framing::CloseDelimited,
                ..FileOptions::default()
            };
            let mut reply = file_reply(&data, request, &options);
            if transfers.fetch_add(1, Ordering::SeqCst) == 0 {
                reply.stall_after = Some((8192, Duration::from_secs(5)));
            }
            reply
        })
        .await
    };
    let fast_timeout = InternalConfig {
        timeout_ms: 150,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), fast_timeout).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let requests = server.transfer_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("range"), None);
    let (start, end) = requests[1].range().expect("resumed with a range");
    assert!(start > 0, "resumed at {start}");
    assert_eq!(end, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_multi_connection_download_falls_back_to_one_connection() {
    let data = test_data(16 * 1024);
    let server = {
        let data = Arc::clone(&data);
        TestServer::start(move |request| match request.range() {
            Some((0, Some(0))) | None => file_reply(&data, request, &FileOptions::default()),
            Some(_) => Reply::status(503),
        })
        .await
    };
    let fast = InternalConfig {
        timeout_ms: 100,
        chunk_count: 4,
        parallel_connections: 4,
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), fast).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    let plain = server
        .transfer_requests()
        .iter()
        .filter(|request| request.range().is_none())
        .count();
    assert_eq!(plain, 1);
    // The failing chunk sent its first request and five retries before the fallback.
    assert!(server.transfer_requests().len() > 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_stops_reading_and_resume_reconnects_at_the_offset() {
    let data = test_data(64 * 1024);
    let server = TestServer::file(Arc::clone(&data), slow_options()).await;
    let (_tx, rx) = watch::channel(single_chunk());
    let run = Run::start(&server.url("/file.bin"), rx).await;
    run.wait_for_bytes(4096).await;
    run.download.pause().await;
    run.download.pause().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let paused_at = run.download.progress().borrow().bytes_done;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(run.download.progress().borrow().bytes_done, paused_at);
    assert!(paused_at < 64 * 1024);
    run.download.resume().await;
    assert_eq!(run.finish().await, Ok(()));
    assert_downloaded(&run, &data);
    let resumed = server
        .transfer_requests()
        .iter()
        .filter_map(support::Request::range)
        .any(|(start, _)| start > 0);
    assert!(resumed, "a request continued at the paused offset");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_keeps_the_connection_when_ranges_are_unsupported() {
    let data = test_data(32 * 1024);
    let options = FileOptions {
        ranges: false,
        ..slow_options()
    };
    let server = TestServer::file(Arc::clone(&data), options).await;
    let (_tx, rx) = watch::channel(InternalConfig {
        timeout_ms: 2000,
        ..config()
    });
    let run = Run::start(&server.url("/file.bin"), rx).await;
    run.wait_for_bytes(2048).await;
    run.download.pause().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    run.download.resume().await;
    assert_eq!(run.finish().await, Ok(()));
    assert_downloaded(&run, &data);
    assert_eq!(server.transfer_requests().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_go_through_the_configured_proxy() {
    let data = test_data(4096);
    let proxy = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let proxied = InternalConfig {
        proxy: Some(proxy.url("")),
        ..config()
    };
    let (run, outcome) = download("http://download.invalid/file.bin", proxied).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
    assert!(
        proxy
            .requests()
            .iter()
            .all(|request| request.target.starts_with("http://download.invalid/"))
    );
}

#[tokio::test]
async fn invalid_proxy_fails_the_download() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bad_proxy = InternalConfig {
        proxy: Some("::not a proxy::".to_owned()),
        ..config()
    };
    let (_tx, rx) = watch::channel(bad_proxy);
    let download = HttpDownload::new("http://127.0.0.1:9/file", dir.path().join("file"), rx);
    download.start().await.expect("start");
    let outcome = download.completion().wait().await;
    let message = outcome.expect_err("invalid proxy");
    assert!(
        message.starts_with("Invalid proxy server ::not a proxy::"),
        "{message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_can_be_bound_to_a_local_address() {
    let data = test_data(4096);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let bound = InternalConfig {
        bind_address: Some("127.0.0.1".to_owned()),
        ..config()
    };
    let (run, outcome) = download(&server.url("/file.bin"), bound).await;
    assert_eq!(outcome, Ok(()));
    assert_downloaded(&run, &data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bind_address_that_is_not_an_ip_fails_the_download() {
    let data = test_data(4096);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let typo = InternalConfig {
        bind_address: Some("192.168.1.x".to_owned()),
        ..config()
    };
    let (_run, outcome) = download(&server.url("/file.bin"), typo).await;
    assert_eq!(
        outcome,
        Err(r#"Invalid bind address "192.168.1.x": it is not an IP address."#.to_owned())
    );
    assert!(server.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_download_starts_only_once() {
    let data = test_data(1024);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let (_tx, rx) = watch::channel(config());
    let run = Run::start(&server.url("/file.bin"), rx).await;
    let error = run.download.start().await.expect_err("second start");
    assert_eq!(error.message(), "The download was already started");
    assert_eq!(run.finish().await, Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engines_work_as_trait_objects() {
    let data = test_data(2048);
    let server = TestServer::file(Arc::clone(&data), FileOptions::default()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let (_tx, rx) = watch::channel(config());
    let engine: Box<dyn DownloadEngine> = Box::new(HttpDownload::new(
        server.url("/file.bin"),
        dir.path().join("f"),
        rx,
    ));
    engine.start().await.expect("start");
    assert_eq!(engine.completion().wait().await, Ok(()));
}
